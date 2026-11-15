//! Whether a game's saves were written.
//!
//! A launch can go well and still lose a save: RetroArch runs the game,
//! exits with status 0, and prints one line saying it could not write the
//! save. That is not a failed launch, and it matters more than most failed
//! launches. This module answers one question from the evidence of one
//! launch, apart from what happened to the launch itself: was a save write
//! seen to fail, and how sure is that?
//!
//! # What counts
//!
//! - [`SaveWriteStanding::ConfirmedByCheck`]: the emulator stated that the
//!   write failed, and EmuWiz's own look at the save folder found that it
//!   cannot be written to. Two independent sources.
//! - [`SaveWriteStanding::ReportedByEmulator`]: the emulator stated that
//!   the write failed. Nothing EmuWiz measured says why. The statement is a
//!   fixed message, matched whole; ordinary messages about saving (a save
//!   being loaded, skipped or written) are never read as a failure.
//! - [`SaveWriteStanding::Suspected`]: the emulator stated nothing, but
//!   EmuWiz found that the save folder cannot be written to.
//!
//! When nothing was seen, [`SaveCapture`] says how much that is worth.
//! RetroArch prints a failed save only when it logs in detail, so without
//! detailed output, or with output that was cut, "no error seen" is not
//! "saves were written".
//!
//! # What is monitored, and what is not
//!
//! A save can be written in several ways ([`SaveMechanism`]), and a
//! diagnostic provider can watch only some of them. [`PROVIDERS`] lists,
//! per emulator, the mechanisms for which a failed write is actually
//! recognised. Today that is one row: RetroArch's battery saves (save RAM,
//! which is also how most of its cores keep memory cards), from messages
//! captured from RetroArch 1.21.0. Save states, files a core or an emulator
//! writes by itself, and every standalone emulator are not monitored.
//!
//! Coverage is never inferred. That an emulator is RetroArch does not mean
//! its save states are watched; that a launch exited with status 0 does not
//! mean a save was written. [`SaveWriteStatus`] therefore keeps three
//! things apart: a write that failed, no failure detected by the monitoring
//! that was available ([`SaveWriteAssessment::monitored`] says what that
//! was), and monitoring that was not available at all.
//!
//! Nothing here changes a save path, a permission or a file. The next
//! steps are things for a person to check.

use serde::{Deserialize, Serialize};

use super::evidence::{EvidenceArea, EvidenceSet, Fact, LogMeaning, PathPresence, PathRole, Trust};

/// How sure it is that a save was not written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveWriteStanding {
    ConfirmedByCheck,
    ReportedByEmulator,
    Suspected,
}

impl SaveWriteStanding {
    /// Whether the emulator itself said a write failed. These are write
    /// errors; [`Self::Suspected`] is a risk.
    pub fn is_write_error(self) -> bool {
        matches!(self, Self::ConfirmedByCheck | Self::ReportedByEmulator)
    }
}

/// A way a game's progress gets written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveMechanism {
    /// Save RAM written by the emulator's front end: in-game ("battery")
    /// saves, and memory cards a core hands over as save RAM.
    BatterySave,
    /// Save states.
    SaveState,
    /// Files an emulator or a core writes by itself: memory-card files,
    /// per-game save folders.
    EmulatorOwnedFiles,
}

impl SaveMechanism {
    pub const ALL: [Self; 3] = [Self::BatterySave, Self::SaveState, Self::EmulatorOwnedFiles];

    pub fn label(self) -> &'static str {
        match self {
            Self::BatterySave => "in-game (battery) saves",
            Self::SaveState => "save states",
            Self::EmulatorOwnedFiles => "save files the emulator writes by itself",
        }
    }
}

/// The mechanisms each built-in diagnostic provider can observe: those for
/// which a failed write is recognised from a real, captured message. An
/// emulator that is not listed has no provider. Adding a row needs a real
/// capture of that emulator failing to write that kind of save.
pub const PROVIDERS: &[(&str, &[SaveMechanism])] = &[("retroarch", &[SaveMechanism::BatterySave])];

/// What the provider for `emulator` can observe, or `None` when there is
/// no provider for it.
pub fn observable_mechanisms(emulator: &str) -> Option<&'static [SaveMechanism]> {
    PROVIDERS
        .iter()
        .find(|(name, _)| *name == emulator)
        .map(|(_, mechanisms)| *mechanisms)
}

/// Why nothing was watching for failed saves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitoringGap {
    /// No diagnostic provider exists for this emulator.
    NoProvider,
    /// No output was captured from the launch.
    NoOutput,
    /// The emulator was not logging in detail, and it prints a failed save
    /// only when it is.
    NotDetailed,
    /// Part of the output was left out or not examined.
    CutShort,
    /// The output was all there and detailed, but does not show the
    /// emulator starting to write a save in the wording that is looked
    /// for: another language, another version, a log sent elsewhere, a
    /// game with nothing to save, or a launch that ended before it got
    /// that far. A failed write could not have been recognised.
    SavePhaseNotSeen,
}

/// What is known about this launch's saves. The four cases are never
/// merged: in particular, the last two are not "saves were written".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "detail", rename_all = "snake_case")]
pub enum SaveWriteStatus {
    /// The emulator stated that a save could not be written.
    WriteFailed(SaveWriteProblem),
    /// Nothing was reported, but EmuWiz found the save folder unusable.
    AtRisk(SaveWriteProblem),
    /// The monitoring that was available saw no failed write. Only the
    /// mechanisms in [`SaveWriteAssessment::monitored`] were watched.
    NoFailureDetected,
    /// Nothing was watching.
    MonitoringUnavailable(MonitoringGap),
}

/// What EmuWiz itself found about the place saves go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveFolderFinding {
    /// It exists and this user may not write to it.
    NotWritable,
    /// Nothing exists there.
    Missing,
    /// It exists and can be written to, so the reason lies elsewhere (a
    /// full drive, for example).
    Writable,
    /// EmuWiz did not look, or could not tell.
    NotKnown,
}

/// How much the captured output can say about saves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveCapture {
    /// Detailed output, none of it left out: a failed save would be in it.
    Complete,
    /// Detailed output with part of it left out or not examined.
    CutShort,
    /// Output without detailed logging, where a failed save is not printed.
    NotDetailed,
    /// No output was captured.
    Missing,
}

impl SaveCapture {
    /// Whether "no error was seen" means anything.
    pub fn would_show_a_failure(self) -> bool {
        self == Self::Complete
    }
}

/// A save write that failed, or is at risk. Repeated messages are one
/// problem with a count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveWriteProblem {
    pub standing: SaveWriteStanding,
    pub mechanism: SaveMechanism,
    /// How many times the emulator said so. Zero when it said nothing.
    pub times: u32,
    pub folder: SaveFolderFinding,
    /// The emulator said it kept a recovery copy of the save data.
    pub recovery_copy_reported: bool,
}

/// What one launch's evidence says about saves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveWriteAssessment {
    pub status: SaveWriteStatus,
    /// The mechanisms a failed write would have been seen for in this
    /// launch: the provider's, and only when its output was complete.
    pub monitored: Vec<SaveMechanism>,
    /// The mechanisms nothing was watching in this launch.
    pub not_monitored: Vec<SaveMechanism>,
    pub capture: SaveCapture,
    /// How many saves the emulator said it wrote. A report, not proof.
    pub saves_reported_written: u32,
}

/// Assess the evidence of one launch. Reads nothing and changes nothing.
/// The emulator is the one the evidence says it is about.
pub fn assess(evidence: &EvidenceSet) -> SaveWriteAssessment {
    let provider = evidence
        .subject
        .emulator
        .as_deref()
        .and_then(observable_mechanisms);
    let observed = || {
        evidence
            .items()
            .iter()
            .filter(|item| item.is_observed() && item.trust != Trust::Asserted)
    };
    let count = |meaning: LogMeaning| -> u32 {
        let events = observed()
            .filter(|item| matches!(&item.fact, Fact::LogEvent(event) if event.meaning == meaning))
            .count();
        u32::try_from(events).unwrap_or(u32::MAX)
    };
    let failed = count(LogMeaning::SaveWriteFailed);
    let folder = observed()
        .filter(|item| item.area == EvidenceArea::Filesystem)
        .find_map(|item| match &item.fact {
            Fact::PathAccess(access) if access.role == PathRole::SaveDirectory => {
                Some(match (access.presence, access.writable) {
                    (PathPresence::Missing, _) => SaveFolderFinding::Missing,
                    (_, Some(false)) => SaveFolderFinding::NotWritable,
                    (_, Some(true)) => SaveFolderFinding::Writable,
                    _ => SaveFolderFinding::NotKnown,
                })
            }
            _ => None,
        })
        .unwrap_or(SaveFolderFinding::NotKnown);
    let folder_unusable = matches!(
        folder,
        SaveFolderFinding::NotWritable | SaveFolderFinding::Missing
    );
    let standing = match (failed > 0, folder_unusable) {
        (true, true) => Some(SaveWriteStanding::ConfirmedByCheck),
        (true, false) => Some(SaveWriteStanding::ReportedByEmulator),
        (false, true) => Some(SaveWriteStanding::Suspected),
        (false, false) => None,
    };

    let mut any_output = false;
    let mut detailed = false;
    let mut cut = false;
    for item in observed() {
        if let Fact::LogCoverage(coverage) = &item.fact {
            any_output = true;
            detailed |= coverage.verbose;
            cut |= coverage.truncated || coverage.lines_ignored > 0;
        }
    }
    let capture = match (any_output, detailed, cut) {
        (false, _, _) => SaveCapture::Missing,
        (true, false, _) => SaveCapture::NotDetailed,
        (true, true, true) => SaveCapture::CutShort,
        (true, true, false) => SaveCapture::Complete,
    };

    // A mechanism was monitored only if a provider recognises its failures,
    // the output it needs was all there, and that output shows the emulator
    // starting to write a save in the wording the provider knows. Being
    // RetroArch, exiting cleanly, or simply not printing a failure, is not
    // monitoring.
    let save_phase_seen = count(LogMeaning::SaveAttempted) > 0;
    let monitored: Vec<SaveMechanism> = match provider {
        Some(mechanisms) if capture == SaveCapture::Complete && save_phase_seen => {
            mechanisms.to_vec()
        }
        _ => Vec::new(),
    };
    let not_monitored = SaveMechanism::ALL
        .into_iter()
        .filter(|mechanism| !monitored.contains(mechanism))
        .collect();
    let problem = |standing| SaveWriteProblem {
        standing,
        mechanism: SaveMechanism::BatterySave,
        times: failed,
        folder,
        recovery_copy_reported: count(LogMeaning::SaveRecoveryWritten) > 0,
    };
    let status = match standing {
        Some(SaveWriteStanding::Suspected) => {
            SaveWriteStatus::AtRisk(problem(SaveWriteStanding::Suspected))
        }
        Some(standing) => SaveWriteStatus::WriteFailed(problem(standing)),
        None if !monitored.is_empty() => SaveWriteStatus::NoFailureDetected,
        None => SaveWriteStatus::MonitoringUnavailable(match (provider, capture) {
            (None, _) => MonitoringGap::NoProvider,
            (Some(_), SaveCapture::Missing) => MonitoringGap::NoOutput,
            (Some(_), SaveCapture::NotDetailed) => MonitoringGap::NotDetailed,
            (Some(_), SaveCapture::CutShort) => MonitoringGap::CutShort,
            (Some(_), SaveCapture::Complete) => MonitoringGap::SavePhaseNotSeen,
        }),
    };
    SaveWriteAssessment {
        status,
        monitored,
        not_monitored,
        capture,
        saves_reported_written: count(LogMeaning::SaveWritten),
    }
}

impl SaveWriteProblem {
    /// One sentence for a heading.
    pub fn headline(&self) -> &'static str {
        match self.standing {
            SaveWriteStanding::ConfirmedByCheck | SaveWriteStanding::ReportedByEmulator => {
                "Your progress may not have been saved"
            }
            SaveWriteStanding::Suspected => "Progress may not be saved for this game",
        }
    }

    /// What was found, in plain words. `emulator` is a display name.
    pub fn explanation(&self, emulator: &str) -> String {
        let said = match self.times {
            0 => String::new(),
            1 => format!("{emulator} reported that it could not write this game's save."),
            times => format!(
                "{emulator} reported {times} times that it could not write this game's save."
            ),
        };
        let folder = match self.folder {
            SaveFolderFinding::NotWritable => {
                " EmuWiz checked the save folder: it cannot be written to."
            }
            SaveFolderFinding::Missing => " EmuWiz checked the save folder: it does not exist.",
            SaveFolderFinding::Writable => {
                " EmuWiz checked the save folder and it can be written to, so the reason is \
                 something else."
            }
            SaveFolderFinding::NotKnown => "",
        };
        let recovery = if self.recovery_copy_reported {
            " It also said it kept a recovery copy of the save data in its own settings \
             folder, in a file whose name starts with RetroArch-recovery."
        } else {
            ""
        };
        format!("{said}{folder}{recovery}").trim().to_string()
    }

    /// What a person can check. EmuWiz changes none of it.
    pub fn next_step(&self) -> &'static str {
        match self.folder {
            SaveFolderFinding::NotWritable => {
                "Check the permissions of the save folder, and that the drive it is on is \
                 not read-only. EmuWiz has not changed anything."
            }
            SaveFolderFinding::Missing => {
                "Check where the emulator is set to keep saves: that folder does not exist. \
                 EmuWiz has not changed anything."
            }
            SaveFolderFinding::Writable => {
                "Check that the drive the save folder is on has free space and is still \
                 connected. EmuWiz has not changed anything."
            }
            SaveFolderFinding::NotKnown => {
                "Check that the save folder exists and can be written to, that its drive has \
                 free space, and where the emulator is set to keep saves. EmuWiz has not \
                 changed anything."
            }
        }
    }
}

fn list(mechanisms: &[SaveMechanism]) -> String {
    let labels: Vec<&str> = mechanisms
        .iter()
        .map(|mechanism| mechanism.label())
        .collect();
    match labels.as_slice() {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

impl SaveWriteAssessment {
    /// The problem, when a write failed or is at risk.
    pub fn problem(&self) -> Option<&SaveWriteProblem> {
        match &self.status {
            SaveWriteStatus::WriteFailed(problem) | SaveWriteStatus::AtRisk(problem) => {
                Some(problem)
            }
            SaveWriteStatus::NoFailureDetected | SaveWriteStatus::MonitoringUnavailable(_) => None,
        }
    }

    /// Whether this is a save-write failure to show prominently and keep.
    pub fn is_write_failure(&self) -> bool {
        matches!(self.status, SaveWriteStatus::WriteFailed(_))
    }

    /// One quiet line about save checking, for a launch that showed no save
    /// problem. `None` when there is a problem to show instead. It never
    /// says that saves were written.
    pub fn coverage_line(&self) -> Option<String> {
        match self.status {
            SaveWriteStatus::WriteFailed(_) | SaveWriteStatus::AtRisk(_) => None,
            SaveWriteStatus::NoFailureDetected => Some(if self.not_monitored.is_empty() {
                format!(
                    "Save check: no error reported for {}.",
                    list(&self.monitored)
                )
            } else {
                format!(
                    "Save check: no error reported for {}. Not checked: {}.",
                    list(&self.monitored),
                    list(&self.not_monitored)
                )
            }),
            SaveWriteStatus::MonitoringUnavailable(gap) => Some(
                match gap {
                    MonitoringGap::NoProvider => "Save check: not available for this emulator.",
                    MonitoringGap::NoOutput
                    | MonitoringGap::NotDetailed
                    | MonitoringGap::CutShort
                    | MonitoringGap::SavePhaseNotSeen => {
                        "Save check: not available for this launch."
                    }
                }
                .to_string(),
            ),
        }
    }

    /// The same, in full, for Game Doctor: what was and was not watched,
    /// and what that is worth.
    pub fn coverage_detail(&self) -> Vec<String> {
        let mut lines = Vec::new();
        match self.status {
            SaveWriteStatus::WriteFailed(_) | SaveWriteStatus::AtRisk(_) => {}
            SaveWriteStatus::NoFailureDetected => lines.push(format!(
                "No failed write was reported for {}. That is what was being checked; it \
                 is not proof that a save was written.",
                list(&self.monitored)
            )),
            SaveWriteStatus::MonitoringUnavailable(gap) => lines.push(
                match gap {
                    MonitoringGap::NoProvider => {
                        "EmuWiz cannot yet read this emulator's output, so it cannot tell \
                         whether a save failed to write."
                    }
                    MonitoringGap::NoOutput => {
                        "No output was captured from this launch, so a failed save could \
                         not have been seen."
                    }
                    MonitoringGap::NotDetailed => {
                        "The emulator was not logging in detail, and it reports a failed \
                         save only when it is. A failed save could not have been seen."
                    }
                    MonitoringGap::CutShort => {
                        "Part of the emulator's output was left out, so a failed save could \
                         have been missed."
                    }
                    MonitoringGap::SavePhaseNotSeen => {
                        "The emulator's output does not show it starting to write a save in \
                         the wording EmuWiz knows, so a failed save could not have been \
                         recognised."
                    }
                }
                .to_string(),
            ),
        }
        if !self.not_monitored.is_empty() {
            lines.push(format!("Not checked: {}.", list(&self.not_monitored)));
        }
        if self.saves_reported_written > 0 {
            lines.push(
                "The emulator reported writing an in-game save. That is its own report."
                    .to_string(),
            );
        }
        lines.push("A launch that went well does not show that saves were written.".to_string());
        lines
    }
}

#[cfg(test)]
#[path = "saves/tests.rs"]
mod tests;
