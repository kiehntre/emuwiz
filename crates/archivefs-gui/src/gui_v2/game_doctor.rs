//! Game Doctor, Part 1: one read-only explanation of what EmuWiz already
//! knows about the selected game's identity and launch readiness.
//!
//! This is a projection only. It reuses Game Details' identity answer
//! (`detail_identity::decide`, which already applies the DAT current-proof
//! rule), the canonical identity review, the saved catalogue (DAT) state and
//! the exact readiness summary the readiness card last displayed for this
//! game (launch plan blockers, firmware state, warnings and the last launch
//! attempt). It performs no I/O, probing, hashing, planning or launching,
//! writes nothing, and offers no action: the readiness card keeps the single
//! primary action. Recognition and passing launch checks are never presented
//! as proof that the game plays; EmuWiz keeps no record of gameplay.

use super::detail_identity::{DetailIdentity, basis_text, identity_wording, likely_match};
use super::identity_review::{Review, ReviewState};
use super::launch_readiness_summary::{
    Cause, FirmwareSummary, GameReadinessSummary, ReadinessFreshness, ReadinessPresentationState,
    status_label,
};
use super::library::Game;
use crate::ui::theme;
use eframe::egui::{self, RichText};

/// How strongly the existing evidence identifies this exact game: the
/// confidence of Game Details' one identity wording.
pub(super) use super::detail_identity::Confidence as IdentityTier;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct IdentityDiagnosis {
    pub tier: IdentityTier,
    /// Why this tier, in plain words, from the evidence that produced it.
    pub basis: String,
    /// Other names the saved evidence suggests (ambiguity only).
    pub candidates: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LineKind {
    Missing,
    Warning,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DoctorLine {
    pub kind: LineKind,
    pub title: String,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct GameDoctorReport {
    pub identity: IdentityDiagnosis,
    /// `None` while launch readiness has not been answered for this game.
    pub readiness: Option<ReadinessView>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ReadinessView {
    pub status: &'static str,
    pub headline: String,
    pub explanation: String,
    pub checks_passed: bool,
    /// Missing requirements, then warnings, as the card reports them.
    pub lines: Vec<DoctorLine>,
}

pub(super) const NOT_PROOF_OF_PLAY: &str = "EmuWiz keeps no record of whether this game actually plays. Recognising it, and passing the launch checks, do not prove that it will run or play correctly.";
pub(super) const READINESS_PENDING: &str = "Launch readiness for this game has not been answered yet. The readiness card above shows it once the check finishes.";
const LAST_RUN_NOT_EVIDENCE: &str = "This describes the last time Play was pressed in this session only. It is not a record of gameplay.";

/// Classify the existing identity answer. `identity` is Game Details' answer
/// (`App::detail_identity`); `review` the canonical review it was built from.
pub(super) fn identity_diagnosis(
    identity: Option<&DetailIdentity>,
    game: &Game,
    review: Option<&Review>,
) -> IdentityDiagnosis {
    let mut notes = Vec::new();
    let mut candidates = Vec::new();
    let guess = likely_match(game, review);
    // The tier is the page's own identity confidence, never a second mapping.
    let wording = identity_wording(identity, game, review);
    let basis = match identity {
        None => "No identity answer is available for this game right now.".to_string(),
        Some(DetailIdentity::Checking) => "A catalogue (DAT) match is on record; this game's current proof is still being re-read. It is not treated as verified until that finishes.".into(),
        Some(DetailIdentity::NotConfirmed) => "A catalogue (DAT) match is on record, but its current proof could not be confirmed. Not verified.".into(),
        Some(DetailIdentity::RecheckNeeded) => {
            notes.push("This game's earlier catalogue verification no longer holds; it needs a re-check.".into());
            match review.map(|review| &review.state) {
                Some(ReviewState::Ambiguous { candidates: found }) => {
                    candidates = found.iter().take(5).cloned().collect();
                    "The saved evidence matches more than one game.".into()
                }
                Some(ReviewState::Conflict { .. }) => {
                    "The identity evidence disagrees with itself.".into()
                }
                _ => basis_text(guess.basis).into(),
            }
        }
        Some(DetailIdentity::Known(known)) => match &known.state {
            ReviewState::Verified(_) => "Current evidence (the file's own identity evidence, or a catalogue match this game has re-confirmed) verifies this exact game.".into(),
            ReviewState::ReferenceMatched => {
                "Matched to arcade reference data by a set audit. Not a verified identity.".into()
            }
            ReviewState::Ambiguous { candidates: found } => {
                candidates = found.iter().take(5).cloned().collect();
                "The saved evidence matches more than one game.".into()
            }
            ReviewState::Conflict { detail } => {
                if !detail.is_empty() {
                    notes.push(detail.clone());
                }
                "The identity evidence disagrees with itself.".into()
            }
            ReviewState::NoSystem => "No trusted system is on record for this file.".into(),
            ReviewState::NotCompared | ReviewState::NoMatch(_) | ReviewState::NoData { .. } => {
                if let Some(note) = wording.note {
                    notes.push(note.into());
                }
                basis_text(guess.basis).into()
            }
        },
    };
    if wording.from_saved_comparison {
        notes.push("A saved catalogue (DAT) comparison found more than one possible match.".into());
    }
    let tier = wording.confidence;
    IdentityDiagnosis {
        tier,
        basis,
        candidates,
        notes,
    }
}

/// Project the readiness card's own summary. `None`, or a summary still
/// checking, is reported as not yet answered, never as ready or missing.
pub(super) fn readiness_view(summary: Option<&GameReadinessSummary>) -> Option<ReadinessView> {
    let summary = summary.filter(|s| s.status != ReadinessPresentationState::Checking)?;
    let mut lines: Vec<DoctorLine> = summary
        .findings
        .iter()
        .map(|finding| DoctorLine {
            kind: LineKind::Missing,
            title: finding.title.into(),
            detail: finding.why.into(),
        })
        .collect();
    let firmware_listed = summary
        .findings
        .iter()
        .any(|finding| finding.cause == Cause::FirmwareMissing);
    match summary.firmware {
        FirmwareSummary::Missing if !firmware_listed => lines.push(DoctorLine {
            kind: LineKind::Missing,
            title: "Console startup software (BIOS / firmware) is needed".into(),
            detail: "The launch plan reports required firmware is not available.".into(),
        }),
        _ => {}
    }
    let checks_passed = matches!(
        summary.status,
        ReadinessPresentationState::Ready | ReadinessPresentationState::ReadyWithWarnings
    ) && summary.findings.is_empty();
    if !checks_passed && lines.is_empty() {
        lines.push(DoctorLine {
            kind: LineKind::Missing,
            title: summary.headline.clone(),
            detail: summary.explanation.clone(),
        });
    }
    lines.extend(summary.warnings.iter().map(|warning| DoctorLine {
        kind: LineKind::Warning,
        title: "Warning".into(),
        detail: warning.clone(),
    }));
    if summary.freshness == ReadinessFreshness::Stale {
        lines.push(DoctorLine {
            kind: LineKind::Warning,
            title: "This answer may be out of date".into(),
            detail: "The last readiness check reported an error, so this is the most recent answer, not a current one.".into(),
        });
    }
    if let Some(attempt) = &summary.attempt {
        let mut detail = format!("{} {}", attempt.why(), LAST_RUN_NOT_EVIDENCE);
        if let Some(hint) = attempt.hint() {
            detail = format!("{detail} {hint}");
        }
        lines.push(DoctorLine {
            kind: LineKind::Warning,
            title: attempt.title().into(),
            detail,
        });
    }
    // 6: passing checks is named as such, never "ready to play".
    let status = match summary.status {
        ReadinessPresentationState::Ready if checks_passed => "Launch checks passed",
        ReadinessPresentationState::ReadyWithWarnings if checks_passed => {
            "Launch checks passed with warnings"
        }
        other => status_label(other),
    };
    Some(ReadinessView {
        status,
        headline: summary.headline.clone(),
        explanation: summary.explanation.clone(),
        checks_passed,
        lines,
    })
}

pub(super) fn report(
    identity: IdentityDiagnosis,
    readiness: Option<&GameReadinessSummary>,
) -> GameDoctorReport {
    GameDoctorReport {
        identity,
        readiness: readiness_view(readiness),
    }
}

/// Collapsed by default; text only, no buttons.
pub(super) fn show(ui: &mut egui::Ui, game_id: i64, report: &GameDoctorReport) {
    egui::CollapsingHeader::new("Game Doctor")
        .id_salt(("game-doctor", game_id))
        .default_open(false)
        .show(ui, |ui| {
            ui.small("Read-only: explains what EmuWiz already knows. Nothing is changed.");
            ui.add_space(theme::SPACE_SM);
            let identity = &report.identity;
            ui.strong(format!("Identity · {}", identity.tier.label()));
            ui.label(&identity.basis);
            if !identity.candidates.is_empty() {
                ui.label(format!(
                    "Possible matches: {}",
                    identity.candidates.join(" · ")
                ));
            }
            for note in &identity.notes {
                ui.small(note);
            }
            ui.add_space(theme::SPACE_SM);
            match &report.readiness {
                None => {
                    ui.strong("Launch readiness · Not answered yet");
                    ui.label(READINESS_PENDING);
                }
                Some(view) => {
                    ui.strong(format!("Launch readiness · {}", view.status));
                    ui.label(&view.headline);
                    ui.small(&view.explanation);
                    for line in &view.lines {
                        let prefix = match line.kind {
                            LineKind::Missing => "Missing",
                            LineKind::Warning => "Warning",
                        };
                        let text = RichText::new(format!("{prefix}: {}", line.title));
                        ui.colored_label(theme::warning(ui), text);
                        ui.small(&line.detail);
                    }
                }
            }
            ui.add_space(theme::SPACE_SM);
            ui.small(NOT_PROOF_OF_PLAY);
        });
}

impl super::App {
    /// Render the Game Doctor for `game` from evidence already on screen.
    pub(super) fn show_game_doctor(&self, ui: &mut egui::Ui, game: &Game) {
        let id = game.archive.id;
        let Some(current) = self.library.game(id) else {
            return;
        };
        let identity = self.detail_identity(current);
        let review = self.identity_review_for(id);
        let readiness = self
            .native_workflows
            .as_ref()
            .and_then(|workflows| workflows.shown_readiness(id, &current.archive.absolute_path));
        let report = report(
            identity_diagnosis(identity.as_ref(), current, review.as_ref()),
            readiness,
        );
        ui.add_space(theme::SPACE_SM);
        show(ui, id, &report);
    }
}

#[cfg(test)]
mod tests;
