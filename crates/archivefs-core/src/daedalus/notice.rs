//! What Blue shows about a finished launch, as plain values.
//!
//! One function turns a diagnosis and a save assessment into a
//! [`LaunchNotice`]: a headline, a few lines, a next step and a level that
//! says how loud it should be. The interface only draws it. Keeping the
//! decision here means it is tested without a window, and that Play, the
//! launch page and Game Doctor cannot disagree.
//!
//! How loud:
//!
//! - [`NoticeLevel::LaunchFailed`]: the launch failed, or a program said
//!   it could not start the game. Prominent.
//! - [`NoticeLevel::SaveWriteFailed`]: the launch did not fail and the
//!   emulator said a save could not be written. Prominent: the launch
//!   going well is exactly why nobody would otherwise notice.
//! - [`NoticeLevel::SavesAtRisk`]: nothing was reported, and EmuWiz found
//!   the save folder unusable. A warning.
//! - [`NoticeLevel::Quiet`]: nothing to report. At most one quiet line
//!   says what save checking covered. An ordinary launch never produces a
//!   banner, and nothing here ever says that saves were written.
//!
//! The text is built from the report it is given. Pass a report that has
//! been through [`super::redact::Redactor::report`]: this module shows
//! what it is handed.

use super::evidence::{Fact, LogMeaning};
use super::rules::{Diagnosis, DiagnosisReport, Effect, LaunchOutcome, Verdict};
use super::saves::{SaveWriteAssessment, SaveWriteStatus};

/// How prominently a notice is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeLevel {
    LaunchFailed,
    SaveWriteFailed,
    SavesAtRisk,
    Quiet,
}

impl NoticeLevel {
    /// Whether this is shown as a banner. A quiet notice never is.
    pub fn is_prominent(self) -> bool {
        self != Self::Quiet
    }
}

/// What to show about one finished launch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchNotice {
    pub level: NoticeLevel,
    /// Empty for a quiet notice.
    pub headline: String,
    /// What was found, most important first.
    pub lines: Vec<String>,
    /// Something a person can do or check. Nothing is done for them.
    pub next_step: Option<String>,
    /// One quiet line about save checking, when there is no save problem
    /// to show in its place.
    pub save_coverage: Option<String>,
    /// A save problem found beside a failed launch, kept apart from what is
    /// said about the launch so that neither hides the other. `None` when
    /// there is none, and when the notice is itself the save warning
    /// ([`NoticeLevel::SaveWriteFailed`], [`NoticeLevel::SavesAtRisk`]).
    pub save_warning: Option<SaveWarning>,
}

/// A save problem, in the words shown for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveWarning {
    pub headline: String,
    pub explanation: String,
    /// Something a person can do or check. Nothing is done for them.
    pub next_step: String,
}

/// Most lines a notice carries. The rest is in Game Doctor.
const MAX_LINES: usize = 4;

fn stopping_cause(report: &DiagnosisReport) -> Option<&Diagnosis> {
    report
        .causes
        .iter()
        .find(|cause| cause.effect != Effect::Degrades)
}

/// Whether a reported symptom restates a failed save write. When the save
/// problem has its own line, the report is not repeated beside it.
fn restates_a_failed_save(report: &DiagnosisReport, symptom: &Diagnosis) -> bool {
    symptom.supporting.iter().any(|id| {
        matches!(
            report.evidence.get(id).map(|item| &item.fact),
            Some(Fact::LogEvent(event)) if event.meaning == LogMeaning::SaveWriteFailed
        )
    })
}

/// The notice for a finished launch. `emulator` is a display name, such as
/// "RetroArch".
pub fn launch_notice(
    report: &DiagnosisReport,
    saves: &SaveWriteAssessment,
    emulator: &str,
) -> LaunchNotice {
    let failed = matches!(
        report.launch,
        LaunchOutcome::Failed | LaunchOutcome::FailureReported
    );
    let save_problem = saves.problem();

    if failed {
        let cause = stopping_cause(report);
        // Whether the process itself failed, or a program only said so.
        let what = if report.launch == LaunchOutcome::Failed {
            "The launch failed".to_string()
        } else {
            format!("{emulator} reported an error that stops a game from starting")
        };
        let headline = match (report.verdict, cause) {
            (Verdict::Explained, Some(cause)) => cause.title.clone(),
            (Verdict::PartlyExplained, Some(cause)) => {
                format!("{what}. A {} cause: {}", cause.standing(), cause.title)
            }
            (Verdict::Conflicting, _) => {
                format!("{what}, and the evidence about why disagrees")
            }
            _ => format!("{what}, and the evidence does not show why"),
        };
        let mut lines = Vec::new();
        if let Some(cause) = cause {
            lines.push(match report.verdict {
                // The headline already names the cause and how it stands.
                Verdict::Explained | Verdict::PartlyExplained => cause.explanation.clone(),
                // The headline says the evidence disagrees or does not show
                // why: a cause listed under it carries its own standing.
                _ => format!(
                    "One {} cause: {}. {}",
                    cause.standing(),
                    cause.title,
                    cause.explanation
                ),
            });
        }
        // What the programs themselves said, as reports. A save problem is
        // not one of these lines: it has its own place in the notice.
        for symptom in report.symptoms.iter().filter(|symptom| {
            symptom.reported && !(save_problem.is_some() && restates_a_failed_save(report, symptom))
        }) {
            lines.push(format!("{} (as reported).", symptom.title));
        }
        if lines.is_empty() {
            // How the process ended is the only thing known.
            lines.extend(report.symptoms.iter().map(|symptom| symptom.title.clone()));
        }
        lines.truncate(MAX_LINES);
        return LaunchNotice {
            level: NoticeLevel::LaunchFailed,
            headline,
            lines,
            next_step: cause
                .and_then(|cause| cause.next_checks.first())
                .map(|check| check.describe())
                .or_else(|| Some("Open Game Doctor for what was and was not examined.".into())),
            save_coverage: match save_problem {
                Some(_) => None,
                None => saves.coverage_line(),
            },
            save_warning: save_problem.map(|problem| SaveWarning {
                headline: problem.headline().to_string(),
                explanation: problem.explanation(emulator),
                next_step: problem.next_step().to_string(),
            }),
        };
    }

    match (&saves.status, save_problem) {
        (SaveWriteStatus::WriteFailed(_), Some(problem)) => LaunchNotice {
            level: NoticeLevel::SaveWriteFailed,
            headline: problem.headline().to_string(),
            lines: vec![problem.explanation(emulator)],
            next_step: Some(problem.next_step().to_string()),
            save_coverage: None,
            save_warning: None,
        },
        (SaveWriteStatus::AtRisk(_), Some(problem)) => LaunchNotice {
            level: NoticeLevel::SavesAtRisk,
            headline: problem.headline().to_string(),
            lines: vec![problem.explanation(emulator)],
            next_step: Some(problem.next_step().to_string()),
            save_coverage: None,
            save_warning: None,
        },
        _ => LaunchNotice {
            level: NoticeLevel::Quiet,
            headline: String::new(),
            lines: Vec::new(),
            next_step: None,
            save_coverage: saves.coverage_line(),
            save_warning: None,
        },
    }
}

#[cfg(test)]
#[path = "notice/tests.rs"]
mod tests;
