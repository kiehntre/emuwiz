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
    /// The existing checks are still running for this game.
    Checking,
    /// No game selected, or no answer for it. Never a diagnosis.
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
pub(super) const NO_SELECTION_WHY: &str =
    "Choose a game below, or open one in Browse & Play, to see its launch check here.";

pub(super) const NO_SELECTION_LINE: &str = "Game launch check: no game selected.";

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
            verdict: Verdict::Checking,
            reason: "Checking this game".into(),
            explanation: "The launch check is running, so nothing is reported yet.".into(),
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
        Verdict::Checking => "Checking",
        Verdict::NotChecked => "Not checked",
    }
}

fn verdict_tone(verdict: Verdict) -> StatusTone {
    match verdict {
        Verdict::Ready => StatusTone::Success,
        Verdict::NeedsAttention => StatusTone::Warning,
        Verdict::Unsupported => StatusTone::Blocked,
        Verdict::Checking => StatusTone::Active,
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
    query: &mut String,
) -> Option<ReadinessAction> {
    if title.is_none() && diagnosis.reason == NO_SELECTION {
        // Nothing selected: one line, so setup problems below stay in view.
        ui.horizontal_wrapped(|ui| {
            status_badge(
                ui,
                verdict_label(diagnosis.verdict),
                verdict_tone(diagnosis.verdict),
            );
            ui.label(NO_SELECTION_LINE);
            search_box(ui, query);
        });
        return None;
    }
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
        ui.horizontal_wrapped(|ui| {
            ui.label("Check another game:");
            search_box(ui, query);
        });
    });
    pressed
}

fn search_box(ui: &mut egui::Ui, query: &mut String) {
    let search = ui.add(
        egui::TextEdit::singleline(query)
            .hint_text("Find a game to check")
            .desired_width(220.0),
    );
    crate::gui_v2::shortcuts::register_search(&search);
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

/// The Doctor's own game choice. `None` keeps the default: the game last
/// opened in Browse & Play.
#[derive(Default)]
pub(super) struct Picker {
    pub(super) game: Option<i64>,
    pub(super) query: String,
    /// Picked this frame: its check has not been started yet.
    pub(super) fresh: bool,
}

impl Picker {
    pub(super) fn pick(&mut self, id: i64) {
        self.game = Some(id);
        self.fresh = true;
        self.query.clear();
    }

    /// The game to check here, given the game Blue's readiness checks
    /// currently belong to. Opening a different game elsewhere (Browse &
    /// Play, Game Details) returns the card to following that game.
    pub(super) fn current(&mut self, on_screen: Option<i64>) -> Option<i64> {
        let picked = self.game?;
        if std::mem::take(&mut self.fresh) || on_screen == Some(picked) {
            Some(picked)
        } else {
            self.game = None;
            None
        }
    }
}

/// A chosen game whose own answer is not available yet.
pub(super) fn checking() -> Diagnosis {
    diagnose(Some(&super::launch_readiness_summary::not_started_summary()))
}

pub(super) const PICKER_LIMIT: usize = 8;

/// Library games whose title or platform contains every word of the query,
/// in library order, at most [`PICKER_LIMIT`]. An empty query lists none.
pub(super) fn matches<'a>(
    games: impl IntoIterator<Item = &'a super::library::Game>,
    query: &str,
) -> Vec<&'a super::library::Game> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    games
        .into_iter()
        .filter(|game| {
            let hay = format!("{} {}", game.title, game.platform).to_lowercase();
            words.iter().all(|word| hay.contains(word.as_str()))
        })
        .take(PICKER_LIMIT)
        .collect()
}

/// The games matching the search box, as buttons. Picking runs nothing by
/// itself; the caller starts the existing readiness check for that game.
pub(super) fn show_matches(
    ui: &mut egui::Ui,
    query: &str,
    games: &[super::library::Game],
) -> Option<i64> {
    let mut choice = None;
    let found = matches(games, query);
    if !query.trim().is_empty() && found.is_empty() {
        ui.label("No game in your library matches.");
    }
    for game in found {
        if ui
            .button(format!("{} ({})", game.title, game.platform))
            .clicked()
        {
            choice = Some(game.archive.id);
        }
    }
    choice
}

#[cfg(test)]
#[path = "doctor_launch_check/tests.rs"]
mod tests;
