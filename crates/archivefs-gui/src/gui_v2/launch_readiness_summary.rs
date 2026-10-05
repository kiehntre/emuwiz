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
        LaunchReadinessInput::IdentityUnknown => identity_summary(false),
        LaunchReadinessInput::IdentityConflicting => identity_summary(true),
        LaunchReadinessInput::Plan {
            plan,
            retroarch_scanned,
            standalone_scans_complete,
            ..
        } => project_plan(plan, *retroarch_scanned, *standalone_scans_complete),
    };
    summary.freshness = freshness;
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

/// Adds the full list of blockers to a not-ready summary and points the
/// primary action at the most actionable one.
fn attach_findings(summary: &mut GameReadinessSummary, input: &LaunchReadinessInput) {
    use diagnosis::{Cause, findings_for};
    let findings = match input {
        LaunchReadinessInput::IdentityUnknown | LaunchReadinessInput::IdentityConflicting => {
            diagnosis::single(Cause::IdentityNotConfirmed)
        }
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
    if findings.is_empty()
        || matches!(
            summary.status,
            ReadinessPresentationState::Ready | ReadinessPresentationState::ReadyWithWarnings
        )
    {
        return;
    }
    if let Some(fix) = findings[0].fix {
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

fn selected_candidate(plan: &LaunchPlan) -> Option<&LaunchCandidate> {
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
            crate::ui::components::status_badge(
                ui,
                status_label(summary.status),
                status_tone(summary.status),
            );
            ui.strong(&summary.headline);
        });
        // The action sits directly under the state so it stays on screen in a
        // short window; the explanation and evidence chips follow it.
        ui.horizontal_wrapped(|ui| {
            if let Some(primary) = summary.primary_action {
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new(action_label(primary)).strong())
                            .fill(crate::ui::theme::PRIMARY_ACTION)
                            .min_size(egui::vec2(180.0, 42.0)),
                    )
                    .clicked()
                {
                    action = Some(primary);
                }
            }
            // Play is always on screen so the page never changes shape: when
            // it cannot run, it is disabled and the headline above says why.
            if summary.primary_action != Some(ReadinessAction::Play) {
                ui.add_enabled(
                    false,
                    egui::Button::new(egui::RichText::new("Play").strong())
                        .min_size(egui::vec2(120.0, 42.0)),
                )
                .on_disabled_hover_text(summary.headline.as_str());
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
            ui.colored_label(crate::ui::theme::WARNING, warning);
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
            amiga_whdload: None,
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
                        action = show(ui, summary);
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
}
