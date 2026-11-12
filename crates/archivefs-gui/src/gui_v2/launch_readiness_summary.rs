//! Compact, read-only launch-readiness presentation for GUI-v2 Game Details.
//!
//! The projector consumes the existing launch input and never performs
//! discovery, identity resolution, firmware inspection, or launch planning.

use crate::launch_readiness_page::{IdentityUnknownReason, LaunchReadinessInput};
use archivefs_core::launch::{
    CandidatePreference, FirmwareReadiness, LaunchBlockerKind, LaunchCandidate, LaunchPlan,
    LaunchReadiness, LaunchTarget, LaunchWarningKind,
};
use eframe::egui;

mod diagnosis;
pub(crate) mod route_truth;
mod vita3k_package;
pub(crate) use diagnosis::{Attempt, AttemptTracker, Finding};
pub(crate) use vita3k_package::note_vita3k_package;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReadinessPresentationState {
    Checking,
    Ready,
    ReadyWithWarnings,
    NeedsEmulator,
    NeedsFirmware,
    NeedsIdentityReview,
    /// No trusted system is on record for this game.
    NeedsSystem,
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
    /// Identity unresolved though the system is known: open the evidence.
    VerifyIdentity,
    /// No trusted system on record: open the existing Choose system flow.
    ChooseSystem,
    /// The system came from the folder name only: open the Launch page,
    /// which offers a one-launch "Try anyway" (nothing saved) beside the
    /// permanent Change system.
    TryAnyway,
    CheckGamesFolder,
    ReviewProblem,
    ChooseEmulator,
    RecheckReadiness,
    ReviewDiscSet,
    OpenActivity,
    /// Game Details only: bring "Look up online" into view. Never reads a
    /// file or contacts Hasheous by itself.
    LookUpOnline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IdentitySummary {
    StrongLocal,
    LaunchableWithWarning,
    NeedsReview,
    /// The saved catalogue holds an exact authoritative match, but launch
    /// still needs the file's own identity evidence.
    CatalogueVerified,
    /// The game's identity is verified by current, trusted evidence while
    /// launch readiness is still being checked. Says nothing about launch.
    Verified,
    /// A DAT match is on record, but this game's current proof is still
    /// being checked. Says nothing about launch.
    VerificationPending,
    /// The check finished without establishing current proof either way
    /// (setup gap, unreadable evidence, unavailable file). Not a disproof.
    VerificationNotConfirmed,
    /// This game's fresh check established its earlier proof is no longer
    /// current.
    RecheckNeeded,
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
    /// Play is ready and at least one other emulator could also be chosen
    /// for this game (the shared launch-choice gate, so a blocked or
    /// unavailable one never counts). The card then also offers "Choose
    /// emulator", which opens the Launch page's per-emulator choice controls;
    /// without it a planner's sole pick left no way to set a default.
    pub(crate) other_emulator_choosable: bool,
}

pub(crate) fn project(
    input: &LaunchReadinessInput,
    freshness: ReadinessFreshness,
) -> GameReadinessSummary {
    let mut summary = match input {
        LaunchReadinessInput::EvidenceNotLoaded | LaunchReadinessInput::RetroArchNotScanned => {
            checking_summary()
        }
        // One fully preflighted Hatari profile launches through the isolated
        // disposable session, so it projects like any other ready plan (with
        // its visible identity-unverified warning and a Play action).
        LaunchReadinessInput::AtariSt(snapshot) if snapshot.launch_request().is_some() => {
            let mut summary = project_plan(&snapshot.plan(), true, true);
            summary.details.extend(snapshot.details());
            summary
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
        LaunchReadinessInput::IdentityUnknown(reason) => identity_unknown_summary(*reason),
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
        LaunchReadinessInput::IdentityConflicting => diagnosis::single(Cause::IdentityNotConfirmed),
        // Each reason already carries its own wording and next action.
        LaunchReadinessInput::IdentityUnknown(_) => Vec::new(),
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
                // An option blocked only by a core tie can be chosen, so it
                // speaks for the game before an option that cannot open the
                // file at all (Stella refusing a zipped 2600 cartridge must
                // not turn the action into "Check games folder").
                selected_candidate(plan)
                    .or_else(|| {
                        plan.candidates.iter().find(|c| {
                            !c.blockers.is_empty()
                                && c.blockers
                                    .iter()
                                    .all(|b| b.kind == LaunchBlockerKind::AmbiguousCore)
                        })
                    })
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

fn identity_unknown_summary(reason: IdentityUnknownReason) -> GameReadinessSummary {
    match reason {
        IdentityUnknownReason::SystemNotChosen => base(
            ReadinessPresentationState::NeedsSystem,
            "Choose a system for this game",
            "EmuWiz has no trusted system for this file, so it cannot pick an emulator. Choose system lets you set it after reviewing its confirmation. Optional catalogue (DAT) verification is separate and is not what is stopping Play.",
            IdentitySummary::NeedsReview,
            SourceSummary::RecheckNeeded,
            FirmwareSummary::Unknown,
            None,
            Some(ReadinessAction::ChooseSystem),
        ),
        IdentityUnknownReason::UnsupportedArchive => base(
            ReadinessPresentationState::Blocked,
            "This archive format isn't supported for launch identity",
            "EmuWiz cannot read a game identity from this kind of archive and will not guess one. Extract the game from the archive, or use a supported format, then recheck readiness. Your file is untouched. Optional catalogue (DAT) verification is separate and is not what is stopping Play.",
            IdentitySummary::NeedsReview,
            SourceSummary::RecheckNeeded,
            FirmwareSummary::Unknown,
            None,
            Some(ReadinessAction::RecheckReadiness),
        ),
        // No identity route exists for these, so there is nothing for a
        // "Verify identity" button to open: only a safe recheck is offered.
        reason @ (IdentityUnknownReason::NoIdentityCheck(_)
        | IdentityUnknownReason::UnsupportedFormat(_)
        | IdentityUnknownReason::AmigaHardDisk
        | IdentityUnknownReason::AmigaIpf
        | IdentityUnknownReason::NoLaunchRoute(_)) => base(
            ReadinessPresentationState::Blocked,
            &match reason {
                IdentityUnknownReason::NoIdentityCheck(system) => {
                    format!("EmuWiz has no identity check for {system} yet")
                }
                IdentityUnknownReason::NoLaunchRoute(system) => {
                    format!("This is a {system} game; no installed emulator starts it")
                }
                IdentityUnknownReason::UnsupportedFormat(_) => {
                    "EmuWiz does not read this file format yet".to_string()
                }
                _ => reason.banner_title().to_string(),
            },
            &reason.no_action_note().unwrap_or_default(),
            IdentitySummary::NeedsReview,
            SourceSummary::RecheckNeeded,
            FirmwareSummary::Unknown,
            None,
            Some(ReadinessAction::RecheckReadiness),
        ),
        IdentityUnknownReason::FolderOnly(system) => base(
            ReadinessPresentationState::NeedsSystem,
            &format!("The {system} system is only a guess from the folder"),
            &format!(
                "EmuWiz only knows the system from the folder this file is in, and the file format is shared with other systems, so Play is not offered on that guess alone. Try anyway starts it once as {system} (or another compatible system) with a warning and saves nothing. To make it lasting, change the system permanently on Review identity. Catalogue (DAT) verification is not required."
            ),
            IdentitySummary::NeedsReview,
            SourceSummary::RecheckNeeded,
            FirmwareSummary::Unknown,
            None,
            Some(ReadinessAction::TryAnyway),
        ),
        IdentityUnknownReason::SystemUnknown => base(
            ReadinessPresentationState::NeedsSystem,
            "The system for this game is not known",
            "No system is recorded for this game and the file does not name one, so Play is not offered on a guess. Try anyway starts it once as a compatible system you pick (only systems that use this file format and have an installed emulator), with a warning, and saves nothing unless you tick Remember this launch choice. Neither sets the game's system; choose it permanently on Review identity. Catalogue (DAT) verification is not required.",
            IdentitySummary::NeedsReview,
            SourceSummary::RecheckNeeded,
            FirmwareSummary::Unknown,
            None,
            Some(ReadinessAction::TryAnyway),
        ),
        IdentityUnknownReason::MediaConflict { recorded, content } => base(
            ReadinessPresentationState::Blocked,
            &format!("This file is a {content} game, not {recorded}"),
            &format!(
                "The file's own header says {content}, but the game is recorded as {recorded}. EmuWiz will not start it with a {recorded} emulator. Review identity lets you change the system."
            ),
            IdentitySummary::NeedsReview,
            SourceSummary::RecheckNeeded,
            FirmwareSummary::Unknown,
            None,
            Some(ReadinessAction::ReviewIdentity),
        ),
        IdentityUnknownReason::NotConfirmed => base(
            ReadinessPresentationState::NeedsIdentityReview,
            "EmuWiz cannot tell which game this file is yet",
            "Play needs the file's own identity evidence to choose the right emulator settings. Optional catalogue (DAT) verification is a separate check and is not what is stopping Play. Verify identity shows what EmuWiz found.",
            IdentitySummary::NeedsReview,
            SourceSummary::RecheckNeeded,
            FirmwareSummary::Unknown,
            None,
            Some(ReadinessAction::VerifyIdentity),
        ),
    }
}

fn identity_summary(conflicting: bool) -> GameReadinessSummary {
    base(
        ReadinessPresentationState::NeedsIdentityReview,
        if conflicting {
            "This file's evidence points to different games"
        } else {
            "EmuWiz cannot tell which game this file is yet"
        },
        if conflicting {
            "Evidence for this game conflicts, so EmuWiz will not guess which game to start. Review identity to choose the right match."
        } else {
            "Play needs the file's own identity evidence to choose the right emulator settings. Optional catalogue (DAT) verification is a separate check and is not what is stopping Play. Review identity shows what EmuWiz thinks it is."
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

    // One option's own content refusal (a zipped cartridge on a standalone
    // emulator that cannot open ZIPs) says nothing about the game when another
    // option can start it, or is blocked for its own reason (a core tie): the
    // card then follows those options, exactly as the Launch page does.
    let no_usable_option = plan
        .candidates
        .iter()
        .all(|candidate| candidate.readiness == LaunchReadiness::Blocked);
    let content_unresolved = |candidate: &LaunchCandidate| {
        candidate
            .blockers
            .iter()
            .any(|blocker| blocker.kind == LaunchBlockerKind::ContentNotResolved)
    };
    if no_usable_option
        && plan.candidates.iter().all(content_unresolved)
        && let Some(candidate) = plan.candidates.first()
    {
        // A file confirmed missing gets its own message (`note_file_missing`);
        // here only "no route can open it as stored" is known.
        return with_details(
            base(
                ReadinessPresentationState::SourceUnavailable,
                "No emulator here can open this game file",
                "None of the installed emulators can start this file as it is stored. A game kept in an archive (such as a ZIP of a disc image) may need extracting, and a moved or disconnected folder must be reconnected.",
                IdentitySummary::StrongLocal,
                SourceSummary::Unavailable,
                firmware_for(candidate),
                emulator_for(candidate),
                Some(ReadinessAction::CheckGamesFolder),
            ),
            details,
        );
    }

    let media_refused = |candidate: &LaunchCandidate| {
        candidate.blockers.iter().any(|blocker| {
            matches!(
                blocker.kind,
                LaunchBlockerKind::MediaTopologyBlocked
                    | LaunchBlockerKind::MediaTopologyReviewRequired
                    | LaunchBlockerKind::ContentNotResolved
            )
        })
    };
    // Only when every option refuses the media itself; an option blocked for
    // its own reason (a core tie) is reported by the branches below.
    if no_usable_option
        && plan.candidates.iter().all(media_refused)
        && let Some(candidate) = plan.candidates.first()
    {
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
    // A saved emulator choice that can no longer be used never falls back to
    // another emulator (launch_choice), so say which one is missing and
    // offer the choice again instead of a generic "choose one".
    // `hold_for_new_choice` gives every remaining option this warning, so its
    // presence alone means the choice is held: a sole remaining option must
    // not fall through to the ready path (re-review of 17615e6c).
    if let Some(warning) = plan.candidates.iter().find_map(|candidate| {
        candidate.warnings.iter().find(|warning| {
            warning.kind
                == archivefs_core::launch::LaunchWarningKind::PreferredLaunchChoiceUnavailable
        })
    }) {
        return with_details(
            base(
                ReadinessPresentationState::NeedsEmulator,
                "Your saved emulator choice cannot be used",
                &format!(
                    "{} Use Choose emulator to pick another one for this launch, or save a new default.",
                    warning.detail
                ),
                IdentitySummary::StrongLocal,
                SourceSummary::Available,
                FirmwareSummary::Unknown,
                None,
                Some(ReadinessAction::ChooseEmulator),
            ),
            details,
        );
    }
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
    // One clear statement: the candidate's own (file-naming) warning wins; the
    // generic line is only a fallback for candidates that carry none.
    if candidate.firmware == FirmwareReadiness::PresentUnverified
        && !candidate.warnings.iter().any(|warning| {
            warning.kind == archivefs_core::launch::LaunchWarningKind::FirmwarePresentUnverified
        })
    {
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
    summary.other_emulator_choosable = plan.candidates.iter().any(|other| {
        !std::ptr::eq(other, candidate)
            && archivefs_core::launch::launch_choice::is_choosable(plan, other)
    });
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
        other_emulator_choosable: false,
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
        | ReadinessPresentationState::NeedsSystem
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
        | ReadinessPresentationState::NeedsIdentityReview
        | ReadinessPresentationState::NeedsSystem => crate::ui::components::StatusTone::Warning,
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
        ReadinessPresentationState::NeedsSystem => "System needed",
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
        ReadinessAction::VerifyIdentity => "Verify identity",
        ReadinessAction::ChooseSystem => "Choose system",
        ReadinessAction::TryAnyway => "Try anyway",
        ReadinessAction::CheckGamesFolder => "Check games folder",
        ReadinessAction::ReviewProblem => "Review problem",
        ReadinessAction::ChooseEmulator => "Choose emulator",
        ReadinessAction::RecheckReadiness => "Recheck readiness",
        ReadinessAction::ReviewDiscSet => "Review multi-disc set",
        ReadinessAction::OpenActivity => "Open Activity",
        ReadinessAction::LookUpOnline => super::hasheous_check::SHORTCUT_LABEL,
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
        let offers_choice = summary.primary_action == Some(ReadinessAction::Play)
            && summary.other_emulator_choosable;
        let action_count = 1 + usize::from(
            (summary.primary_action.is_some()
                && summary.primary_action != Some(ReadinessAction::Play))
                || offers_choice,
        );
        let available = ui.available_width();
        let gap = ui.spacing().item_spacing.x;
        // Two buttons share the row when each keeps a usable width; otherwise
        // they stack. `add_sized` does not wrap in a wrapping row, so never
        // rely on that: a narrow card used to push Play past its edge.
        let side_by_side = action_count == 2 && available >= 240.0;
        let slot = if side_by_side {
            ((available - gap) / 2.0).min(260.0)
        } else {
            available.min(260.0)
        };
        let button_layout = if action_count == 2 && !side_by_side {
            egui::Layout::top_down(egui::Align::Min)
        } else {
            egui::Layout::left_to_right(egui::Align::Min).with_main_wrap(true)
        };
        ui.allocate_ui_with_layout(egui::vec2(available, 0.0), button_layout, |ui| {
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
            if offers_choice
                && ui
                    .add_sized(
                        [slot, 64.0],
                        egui::Button::new(action_label(ReadinessAction::ChooseEmulator)).wrap(),
                    )
                    .on_hover_text("Other emulators can also start this game. Choose one for this launch, or save it as the default for this game or system.")
                    .clicked()
            {
                action = Some(ReadinessAction::ChooseEmulator);
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
            if ui
                .small_button(action_label(ReadinessAction::LookUpOnline))
                .on_hover_text(super::hasheous_check::SHORTCUT_HINT)
                .clicked()
            {
                action = Some(ReadinessAction::LookUpOnline);
            }
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
    "Play checks that this game can start, then starts it. Nothing starts unless the check passes.";

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
        Some(finding) => format!("{}. {}", finding.title, finding.why),
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
            // A launchable-with-warning game states the warning itself (for
            // example which firmware files are unverified), not only a
            // generic note, while Play stays enabled.
            let reason = if status == State::ReadyWithWarnings && ready {
                summary.warnings.first().cloned().unwrap_or(reason)
            } else {
                reason
            };
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
    let reason_response = ui
        .add(egui::Label::new(reason).wrap().selectable(false))
        .on_hover_text(view.reason.as_str());
    // Reserve the full reason block so a longer answer arriving after
    // "Checking" does not push the buttons down.
    let row_height = ui.fonts_mut(|fonts| {
        fonts.row_height(&egui::FontId::proportional(crate::ui::theme::METADATA_SIZE))
    });
    let reserved = row_height * reason_rows as f32;
    if reason_response.rect.height() < reserved {
        ui.add_space(reserved - reason_response.rect.height());
    }
    let gap = ui.spacing().item_spacing.x;
    let fill = crate::ui::theme::primary_action_fill(ui);
    let fix_is_primary = view.fix.is_some() && !view.pending;
    let count = 1 + usize::from(view.fix.is_some());
    let inline_trailing = trailing.is_some() && width >= 3.0 * 144.0 + gap * 2.0;
    let slots = count + usize::from(inline_trailing);
    // Decided for the widest answer (a fix plus Play), so the arrangement does
    // not change when readiness resolves.
    let stacked = width < 2.0 * 144.0 + gap;
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
            if fix_is_primary {
                if let Some(clicked) = fix_button(ui, true) {
                    choice = Some(clicked);
                }
            } else if stacked {
                // Stacked: keep the fix button's slot so Play and the
                // trailing button stay put when a fix appears.
                ui.allocate_space(egui::vec2(slot, 64.0));
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

/// A file already found missing is never offered for Play and never left
/// reading as "Checking". DOS review offers Play without looking at the file.
pub(crate) fn note_file_missing(summary: &mut GameReadinessSummary) {
    if summary.primary_action == Some(ReadinessAction::Play)
        || summary.status == ReadinessPresentationState::Checking
    {
        summary.status = ReadinessPresentationState::SourceUnavailable;
        summary.headline = "The game file is unavailable".into();
        summary.explanation =
            "Reconnect its drive or restore the file, then open this game again.".into();
        summary.source = SourceSummary::Unavailable;
        summary.primary_action = Some(ReadinessAction::CheckGamesFolder);
    }
}

/// Game Details' DAT identity answer, on the identity line only: status,
/// headline and actions keep their answer. Never upgrades to Verified.
pub(crate) fn note_dat_identity(summary: &mut GameReadinessSummary, identity: IdentitySummary) {
    debug_assert!(matches!(
        identity,
        IdentitySummary::VerificationPending
            | IdentitySummary::VerificationNotConfirmed
            | IdentitySummary::RecheckNeeded
    ));
    if summary.identity == IdentitySummary::NeedsReview {
        summary.identity = identity;
    }
}

/// Callers pass only a current, strictly verified identity. While readiness
/// is still checking, only the identity line changes: the headline, status
/// and primary action keep saying launch has not been decided.
pub(crate) fn note_catalogue_verified(summary: &mut GameReadinessSummary) {
    if summary.status == ReadinessPresentationState::Checking
        && summary.identity == IdentitySummary::NeedsReview
    {
        summary.identity = IdentitySummary::Verified;
    }
    if summary.status == ReadinessPresentationState::NeedsIdentityReview
        && summary.identity == IdentitySummary::NeedsReview
    {
        summary.identity = IdentitySummary::CatalogueVerified;
        summary.headline = "Verified. Launch also needs the file's own identity".into();
        summary.explanation = "This game matched trusted identification data exactly. Before it plays, EmuWiz also reads the file's own identity data, and that could not be confirmed yet. The verification itself is fine; Verify identity shows what is known.".into();
    }
}

fn identity_label(identity: IdentitySummary) -> &'static str {
    match identity {
        IdentitySummary::StrongLocal => "Strong local identity",
        IdentitySummary::LaunchableWithWarning => "Launchable with warning",
        IdentitySummary::NeedsReview => "Needs review",
        IdentitySummary::CatalogueVerified => "Verified · file identity still needed to launch",
        IdentitySummary::Verified => "Verified",
        IdentitySummary::VerificationPending => "Checking verification",
        IdentitySummary::VerificationNotConfirmed => "Verification not yet confirmed",
        IdentitySummary::RecheckNeeded => "Re-check needed",
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
            azahar: None,
            vita3k: None,
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

    /// Census 2026-11-10 (NES/Game Gear/... zips): the Launch page offered
    /// Play through RetroArch while Game Details said "The game file is
    /// unavailable", taken from the standalone option's own ZIP refusal.
    #[test]
    fn a_zipped_cartridge_refused_by_one_option_stays_playable_through_another() {
        let mesen = candidate(
            LaunchReadiness::Blocked,
            FirmwareReadiness::NotRequired,
            vec![LaunchBlocker::new(
                LaunchBlockerKind::ContentNotResolved,
                "a zipped cartridge launches through RetroArch only",
            )],
            vec![],
            CandidatePreference::Undetermined,
            Some("/mnt/nes/Game.zip"),
        );
        let retroarch = candidate(
            LaunchReadiness::Ready,
            FirmwareReadiness::NotRequired,
            vec![],
            vec![],
            CandidatePreference::SoleEligible,
            Some("/mnt/nes/Game.zip"),
        );
        let summary = project(
            &plan(vec![mesen.clone(), retroarch]),
            ReadinessFreshness::Current,
        );
        assert_ne!(
            summary.status,
            ReadinessPresentationState::SourceUnavailable
        );
        assert_eq!(summary.primary_action, Some(ReadinessAction::Play));
        // Atari 2600 zip: the other options are blocked by a core tie, not by
        // the file, so the card must not say the file cannot be opened.
        let tied = candidate(
            LaunchReadiness::Blocked,
            FirmwareReadiness::NotRequired,
            vec![LaunchBlocker::new(
                LaunchBlockerKind::AmbiguousCore,
                "more than one installed RetroArch core resolves to Atari2600",
            )],
            vec![],
            CandidatePreference::Undetermined,
            Some("/mnt/a2600/Game.zip"),
        );
        let summary = project(
            &plan(vec![mesen.clone(), tied]),
            ReadinessFreshness::Current,
        );
        assert_ne!(
            summary.status,
            ReadinessPresentationState::SourceUnavailable
        );
        // Census 2026-11-10: the media branch said "could not be read" here.
        assert_ne!(summary.status, ReadinessPresentationState::MediaUnreadable);
        // Census 2026-11-10 (059ae25a): the action followed the option that
        // cannot open ZIPs ("Check games folder"); the tie can be chosen.
        assert_eq!(
            summary.primary_action,
            Some(ReadinessAction::ChooseEmulator)
        );
        // With nothing else, the refusal is still reported - without claiming
        // the folder is unreachable (the file may simply be an archive).
        let alone = project(&plan(vec![mesen]), ReadinessFreshness::Current);
        assert_eq!(alone.status, ReadinessPresentationState::SourceUnavailable);
        assert!(!alone.explanation.contains("cannot be reached"));
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

    /// The remembered emulator choice is part of the plan the worker must
    /// share with the foreground: it turns "choose one" into Play.
    #[test]
    fn a_remembered_choice_among_safe_candidates_is_ready_not_a_choice() {
        let summary = project(
            &plan(vec![
                candidate(
                    LaunchReadiness::Ready,
                    FirmwareReadiness::NotRequired,
                    vec![],
                    vec![],
                    CandidatePreference::Remembered,
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
        assert_eq!(summary.status, ReadinessPresentationState::Ready);
        assert_eq!(summary.primary_action, Some(ReadinessAction::Play));
    }

    /// A remembered emulator that is blocked must not let a clean alternative
    /// make the game Ready: without the memory the alternative looks clean.
    #[test]
    fn a_blocked_remembered_candidate_beside_a_clean_one_is_not_ready() {
        let summary = project(
            &plan(vec![
                candidate(
                    LaunchReadiness::Blocked,
                    FirmwareReadiness::NotRequired,
                    vec![LaunchBlocker {
                        kind: LaunchBlockerKind::NoInstallationCandidate,
                        detail: "remembered profile is gone".into(),
                    }],
                    vec![],
                    CandidatePreference::Remembered,
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
        assert_ne!(
            summary.status,
            ReadinessPresentationState::Ready,
            "{summary:?}"
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
    fn verified_identity_while_checking_changes_only_the_identity_line() {
        let mut checking = project(
            &LaunchReadinessInput::EvidenceNotLoaded,
            ReadinessFreshness::Current,
        );
        let before = checking.clone();
        assert_eq!(before.identity, IdentitySummary::NeedsReview);
        note_catalogue_verified(&mut checking);
        assert_eq!(checking.identity, IdentitySummary::Verified);
        assert_eq!(identity_label(checking.identity), "Verified");
        // Identity is not launch readiness: still checking, no Play.
        assert_eq!(checking.status, ReadinessPresentationState::Checking);
        assert_eq!(checking.headline, before.headline);
        assert_eq!(checking.explanation, before.explanation);
        assert_eq!(checking.primary_action, before.primary_action);
        assert_ne!(checking.primary_action, Some(ReadinessAction::Play));
        assert_eq!(checking.source, before.source);
        // Without verified identity nothing changes.
        let unverified = project(
            &LaunchReadinessInput::EvidenceNotLoaded,
            ReadinessFreshness::Current,
        );
        assert_eq!(unverified.identity, IdentitySummary::NeedsReview);
    }

    #[test]
    fn identity_inputs_are_never_called_verified_by_the_summary() {
        let unknown = project(
            &LaunchReadinessInput::IdentityUnknown(
                crate::launch_readiness_page::IdentityUnknownReason::NotConfirmed,
            ),
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

    /// Owner 2026-11-11: PS1 showed one Play for the planner's sole pick
    /// (a reviewed SwanStation core) while standalone DuckStation was also
    /// ready, so "Default for PSX" was unreachable. Another choosable
    /// emulator now adds "Choose emulator" beside Play; a blocked or
    /// unavailable one never does.
    #[test]
    fn a_ready_sole_pick_offers_choose_emulator_only_when_another_is_choosable() {
        let sole = candidate(
            LaunchReadiness::Ready,
            FirmwareReadiness::NotRequired,
            vec![],
            vec![],
            CandidatePreference::SoleEligible,
            Some("/game.chd"),
        );
        let other_ready = candidate(
            LaunchReadiness::Ready,
            FirmwareReadiness::Verified,
            vec![],
            vec![],
            CandidatePreference::Undetermined,
            Some("/game.chd"),
        );
        let summary = project(
            &plan(vec![sole.clone(), other_ready]),
            ReadinessFreshness::Current,
        );
        assert_eq!(summary.primary_action, Some(ReadinessAction::Play));
        assert!(summary.other_emulator_choosable);
        let (labels, action) = click_card_button(&summary, "Choose emulator");
        assert_eq!(action, Some(ReadinessAction::ChooseEmulator), "{labels:?}");
        let (labels, action) = click_card_button(&summary, "Play");
        assert_eq!(labels.iter().filter(|label| *label == "Play").count(), 1);
        assert_eq!(action, Some(ReadinessAction::Play));

        for unavailable in [
            LaunchBlocker::new(
                LaunchBlockerKind::NoInstallationCandidate,
                "the chosen DuckStation is no longer installed",
            ),
            LaunchBlocker::new(LaunchBlockerKind::RequiredFirmwareMissing, "no BIOS"),
        ] {
            let blocked = candidate(
                LaunchReadiness::Blocked,
                FirmwareReadiness::Missing,
                vec![unavailable],
                vec![],
                CandidatePreference::Undetermined,
                Some("/game.chd"),
            );
            let summary = project(
                &plan(vec![sole.clone(), blocked]),
                ReadinessFreshness::Current,
            );
            assert_eq!(summary.primary_action, Some(ReadinessAction::Play));
            assert!(!summary.other_emulator_choosable);
            let (labels, action) = click_card_button(&summary, "Choose emulator");
            assert_eq!(action, None);
            assert!(!labels.iter().any(|label| label == "Choose emulator"));
        }

        let alone = project(&plan(vec![sole]), ReadinessFreshness::Current);
        assert!(!alone.other_emulator_choosable);
    }

    /// Owner 2026-11-11: a missing saved PSX default showed only "More than
    /// one safe emulator choice is available". The card now names the
    /// missing selection, routes to Choose emulator and never offers Play,
    /// even when just one other emulator remains.
    #[test]
    fn a_missing_saved_choice_is_named_and_offers_choose_emulator_not_play() {
        let held = |preference| {
            candidate(
                LaunchReadiness::ReadyWithWarnings,
                FirmwareReadiness::NotRequired,
                vec![],
                vec![LaunchWarning::new(
                    archivefs_core::launch::LaunchWarningKind::PreferredLaunchChoiceUnavailable,
                    "The emulator in your saved emulator default for this system (duckstation at /apps/DuckStationRemoved) is no longer available. No other emulator is used instead; choose one to continue.",
                )],
                preference,
                Some("/game.chd"),
            )
        };
        for candidates in [
            vec![held(CandidatePreference::Undetermined)],
            vec![
                held(CandidatePreference::Undetermined),
                held(CandidatePreference::Undetermined),
            ],
        ] {
            let summary = project(&plan(candidates), ReadinessFreshness::Current);
            assert_eq!(summary.status, ReadinessPresentationState::NeedsEmulator);
            assert_eq!(
                summary.primary_action,
                Some(ReadinessAction::ChooseEmulator)
            );
            assert!(summary.headline.contains("saved emulator choice"));
            assert!(
                summary
                    .explanation
                    .contains("duckstation at /apps/DuckStationRemoved")
            );
            assert!(summary.explanation.contains("Choose emulator"));
            let (_, action) = click_card_button(&summary, "Play");
            assert_eq!(action, None, "Play stays inert");
        }
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
    fn compact_card_names_the_unverified_firmware_once_and_keeps_play() {
        let warning = LaunchWarning::new(
            LaunchWarningKind::FirmwarePresentUnverified,
            "Firmware found but not verified: kick34005.A500 is present but its contents were not checked.",
        );
        let summary = project(
            &plan(vec![candidate(
                LaunchReadiness::ReadyWithWarnings,
                FirmwareReadiness::PresentUnverified,
                vec![],
                vec![warning],
                CandidatePreference::SoleEligible,
                Some("/game.adf"),
            )]),
            ReadinessFreshness::Current,
        );
        let unverified = summary
            .warnings
            .iter()
            .filter(|text| text.to_lowercase().contains("not verified"))
            .count();
        assert_eq!(unverified, 1, "{:?}", summary.warnings);
        let view = compact(&summary, false);
        assert!(view.reason.contains("kick34005.A500"), "{}", view.reason);
        assert!(view.reason.contains("Firmware found but not verified"));
        assert!(view.play_enabled);
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
            &LaunchReadinessInput::IdentityUnknown(
                crate::launch_readiness_page::IdentityUnknownReason::NotConfirmed,
            ),
            ReadinessFreshness::Current,
        );
        assert_eq!(
            summary.status,
            ReadinessPresentationState::NeedsIdentityReview
        );
        let deferred = compact(&summary, true);
        assert_eq!(deferred.badge, "Not fully checked");
        assert!(deferred.play_enabled && deferred.pending);
        assert_eq!(deferred.fix, Some(ReadinessAction::VerifyIdentity));
        // Once the checksum has run, the same answer is a current refusal.
        let current = compact(&summary, false);
        assert!(!current.play_enabled && !current.pending);
        assert_eq!(current.fix, Some(ReadinessAction::VerifyIdentity));
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

#[cfg(test)]
mod identity_reason_wording_tests {
    use super::*;

    fn summary(reason: IdentityUnknownReason) -> GameReadinessSummary {
        project(
            &LaunchReadinessInput::IdentityUnknown(reason),
            ReadinessFreshness::Current,
        )
    }

    #[test]
    fn system_not_chosen_offers_choose_system_not_identity_review() {
        let s = summary(IdentityUnknownReason::SystemNotChosen);
        assert_eq!(s.status, ReadinessPresentationState::NeedsSystem);
        assert_eq!(status_label(s.status), "System needed");
        assert_eq!(s.primary_action, Some(ReadinessAction::ChooseSystem));
        assert_eq!(action_label(ReadinessAction::ChooseSystem), "Choose system");
        assert!(s.headline.contains("Choose a system"));
        assert!(s.explanation.contains("separate"));
        assert_ne!(status_label(s.status), "Identity needs review");
    }

    #[test]
    fn unsupported_archive_says_so_and_never_identity_review() {
        let s = summary(IdentityUnknownReason::UnsupportedArchive);
        assert_ne!(status_label(s.status), "Identity needs review");
        assert!(s.headline.contains("archive format isn't supported"));
        assert!(s.explanation.contains("Extract"));
        assert!(s.explanation.contains("not what is stopping Play"));
        assert_eq!(s.primary_action, Some(ReadinessAction::RecheckReadiness));
        assert!(s.findings.is_empty());
        assert!(!s.explanation.contains("Review identity"));
    }

    #[test]
    fn unconfirmed_identity_offers_verify_identity() {
        let s = summary(IdentityUnknownReason::NotConfirmed);
        assert_eq!(s.status, ReadinessPresentationState::NeedsIdentityReview);
        assert_eq!(s.primary_action, Some(ReadinessAction::VerifyIdentity));
        assert_eq!(
            action_label(ReadinessAction::VerifyIdentity),
            "Verify identity"
        );
        assert!(s.explanation.contains("not what is stopping Play"));
    }

    #[test]
    fn conflicting_identity_keeps_review_identity() {
        let s = project(
            &LaunchReadinessInput::IdentityConflicting,
            ReadinessFreshness::Current,
        );
        assert_eq!(s.primary_action, Some(ReadinessAction::ReviewIdentity));
        assert_eq!(status_label(s.status), "Identity needs review");
    }

    #[test]
    fn chooser_and_verify_actions_route_to_the_identity_screen() {
        use crate::gui_v2::native_workflows::route_for_action;
        for action in [
            ReadinessAction::ChooseSystem,
            ReadinessAction::VerifyIdentity,
        ] {
            assert_eq!(
                route_for_action(action, 7),
                Some(crate::gui_v2::routes::Route::ReviewIdentity(7))
            );
        }
    }

    #[test]
    fn launch_page_blockers_differ_per_reason() {
        use IdentityUnknownReason as R;
        let texts: Vec<_> = [R::SystemNotChosen, R::UnsupportedArchive, R::NotConfirmed]
            .iter()
            .map(|r| (r.blocker_text(), r.banner_title()))
            .collect();
        assert_ne!(texts[0], texts[1]);
        assert_ne!(texts[1], texts[2]);
        assert!(
            R::UnsupportedArchive
                .blocker_text()
                .contains("not supported")
        );
        for r in [R::SystemNotChosen, R::UnsupportedArchive, R::NotConfirmed] {
            assert!(
                !r.blocker_text()
                    .to_lowercase()
                    .contains("dat verification is required")
            );
        }
    }

    #[test]
    fn no_route_reasons_offer_only_a_recheck_never_a_dead_end_verify_identity_action() {
        use IdentityUnknownReason as R;
        for r in [
            R::NoIdentityCheck("Sharp X68000"),
            R::UnsupportedFormat("Atari 2600"),
            R::AmigaHardDisk,
            R::AmigaIpf,
        ] {
            let s = summary(r);
            assert_eq!(
                s.primary_action,
                Some(ReadinessAction::RecheckReadiness),
                "{r:?}"
            );
            assert!(!r.has_identity_screen());
            let text = format!("{} {}", s.headline, s.explanation);
            assert!(!text.contains("cannot tell which game"), "{text}");
            assert!(!text.contains("Extract the game"), "{text}");
            assert!(!text.contains("Verify identity"), "{text}");
            assert!(!text.contains("Nothing about your file"), "{text}");
            assert!(!text.contains("Choose system"), "{text}");
            assert!(
                text.contains("not caused by missing DAT verification"),
                "{text}"
            );
        }
        let s = summary(R::NoIdentityCheck("Sharp X68000"));
        assert!(s.headline.contains("no identity check for Sharp X68000"));
        assert!(
            s.explanation
                .contains("not caused by missing DAT verification")
        );
    }

    #[test]
    fn choose_system_and_verify_identity_actions_are_kept() {
        use IdentityUnknownReason as R;
        assert_eq!(
            summary(R::SystemNotChosen).primary_action,
            Some(ReadinessAction::ChooseSystem)
        );
        assert_eq!(
            summary(R::NotConfirmed).primary_action,
            Some(ReadinessAction::VerifyIdentity)
        );
        assert!(R::SystemNotChosen.has_identity_screen() && R::NotConfirmed.has_identity_screen());
    }

    #[test]
    fn amiga_reasons_reuse_the_amiga_readiness_wording() {
        use IdentityUnknownReason as R;
        assert!(
            R::AmigaHardDisk
                .blocker_text()
                .contains("private working copy")
        );
        assert!(R::AmigaIpf.blocker_text().contains("CAPS/SPS"));
    }

    #[test]
    fn new_reasons_never_enable_play_or_report_ready_like_the_old_ones() {
        use IdentityUnknownReason as R;
        for r in [
            R::SystemNotChosen,
            R::UnsupportedArchive,
            R::NotConfirmed,
            R::NoIdentityCheck("MSX"),
            R::UnsupportedFormat("Atari 2600"),
            R::AmigaHardDisk,
            R::AmigaIpf,
        ] {
            let s = summary(r);
            assert!(!compact(&s, false).play_enabled, "{r:?}");
            assert_ne!(s.primary_action, Some(ReadinessAction::Play), "{r:?}");
            assert!(
                !matches!(
                    s.status,
                    ReadinessPresentationState::Ready
                        | ReadinessPresentationState::ReadyWithWarnings
                ),
                "{r:?}"
            );
        }
    }
}
