//! Setup & Doctor's "Selected game" launch check.
//!
//! A read-only projection over the launch-readiness answer the readiness card
//! already holds for the game selected in Browse & Play. It runs no check of
//! its own, never launches, and never repairs, downloads, moves or renames
//! anything. Each verdict comes from the existing findings; when no current
//! answer exists the game is reported as not checked, never as ready.

use super::launch_readiness_summary::{
    Cause, GameReadinessSummary, ReadinessAction, ReadinessFreshness, ReadinessPresentationState,
    action_label,
};
use crate::ui::components::{StatusTone, card, status_badge};
use eframe::egui;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Verdict {
    /// The existing launch checks passed for the current answer.
    Ready,
    /// Something the person can act on is in the way.
    NeedsAttention,
    /// Every finding says EmuWiz does not launch this format or system yet.
    Unsupported,
    /// No selected game, or its check has not finished. Never a diagnosis.
    NotChecked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Diagnosis {
    pub(super) verdict: Verdict,
    pub(super) reason: String,
    pub(super) explanation: String,
    /// One next step, routed to an existing workflow. Never Play.
    pub(super) action: Option<ReadinessAction>,
}

pub(super) const NO_SELECTION: &str = "No game selected";
pub(super) const NO_SELECTION_WHY: &str = "Open a game in Browse & Play and its launch check appears here. Setup & Doctor runs no game checks of its own.";

fn unsupported(cause: Cause) -> bool {
    matches!(cause, Cause::FormatUnsupported | Cause::PlatformUnsupported)
}

pub(super) fn diagnose(summary: Option<&GameReadinessSummary>) -> Diagnosis {
    let Some(summary) = summary else {
        return Diagnosis {
            verdict: Verdict::NotChecked,
            reason: NO_SELECTION.into(),
            explanation: NO_SELECTION_WHY.into(),
            action: None,
        };
    };
    use ReadinessPresentationState as S;
    if summary.status == S::Checking {
        return Diagnosis {
            verdict: Verdict::NotChecked,
            reason: "Still checking this game".into(),
            explanation: "The launch check has not finished, so nothing is reported yet. Open Game Details to follow it.".into(),
            action: None,
        };
    }
    let current = summary.freshness == ReadinessFreshness::Current;
    if current
        && matches!(summary.status, S::Ready | S::ReadyWithWarnings)
        && summary.findings.is_empty()
    {
        let explanation = match summary.warnings.first() {
            Some(warning) => format!("Launch checks passed, with a warning: {warning}"),
            None => "Launch checks passed. This is not proof the game plays; EmuWiz keeps no record of gameplay.".into(),
        };
        return Diagnosis {
            verdict: Verdict::Ready,
            reason: summary.headline.clone(),
            explanation,
            action: None,
        };
    }
    // Findings are ordered most actionable first.
    if current
        && !summary.findings.is_empty()
        && summary.findings.iter().all(|f| unsupported(f.cause))
    {
        let finding = &summary.findings[0];
        return Diagnosis {
            verdict: Verdict::Unsupported,
            reason: finding.title.into(),
            explanation: finding.why.into(),
            action: finding.fix,
        };
    }
    if current && let Some(finding) = summary.findings.iter().find(|f| !unsupported(f.cause)) {
        return Diagnosis {
            verdict: Verdict::NeedsAttention,
            reason: finding.title.into(),
            explanation: finding.why.into(),
            action: finding.fix,
        };
    }
    // A stale answer, or a blocked one that lists no finding: the card's own
    // reason and action, unchanged.
    Diagnosis {
        verdict: Verdict::NeedsAttention,
        reason: summary.headline.clone(),
        explanation: summary.explanation.clone(),
        action: summary.primary_action,
    }
}

pub(super) fn verdict_label(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Ready => "Ready",
        Verdict::NeedsAttention => "Needs attention",
        Verdict::Unsupported => "Unsupported",
        Verdict::NotChecked => "Not checked",
    }
}

fn verdict_tone(verdict: Verdict) -> StatusTone {
    match verdict {
        Verdict::Ready => StatusTone::Success,
        Verdict::NeedsAttention => StatusTone::Warning,
        Verdict::Unsupported => StatusTone::Blocked,
        Verdict::NotChecked => StatusTone::Pending,
    }
}

/// The button text Doctor shows for an action. Play and the in-place checks
/// are worded for where they actually go.
pub(super) fn doctor_action_label(action: ReadinessAction) -> &'static str {
    match action {
        ReadinessAction::Play | ReadinessAction::TryAnyway => "Open Launch page",
        ReadinessAction::RecheckReadiness | ReadinessAction::LookUpOnline => "Open Game Details",
        other => action_label(other),
    }
}

/// Render the card. Returns the pressed action, if any; the caller routes it.
pub(super) fn show(
    ui: &mut egui::Ui,
    title: Option<&str>,
    diagnosis: &Diagnosis,
) -> Option<ReadinessAction> {
    let mut pressed = None;
    card(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.heading("Selected game launch check");
        if let Some(title) = title {
            ui.label(title);
        }
        ui.horizontal_wrapped(|ui| {
            status_badge(
                ui,
                verdict_label(diagnosis.verdict),
                verdict_tone(diagnosis.verdict),
            );
            ui.strong(&diagnosis.reason);
        });
        ui.label(&diagnosis.explanation);
        if let Some(action) = diagnosis.action
            && ui
                .button(doctor_action_label(action))
                .on_hover_text("Opens the existing workflow. Nothing is changed.")
                .clicked()
        {
            pressed = Some(action);
        }
    });
    pressed
}

/// Where a Doctor action goes: always an existing screen, never a launch or
/// a repair. Play and "Try anyway" open the Launch page, whose own preflight
/// and confirmations still decide; in-place checks open Game Details.
pub(super) fn route(action: ReadinessAction, game_id: i64) -> Option<super::routes::Route> {
    match action {
        ReadinessAction::RecheckReadiness | ReadinessAction::LookUpOnline => {
            Some(super::routes::Route::Game(game_id))
        }
        other => super::native_workflows::route_for_action(other, game_id),
    }
}

#[cfg(test)]
#[path = "doctor_launch_check/tests.rs"]
mod tests;
