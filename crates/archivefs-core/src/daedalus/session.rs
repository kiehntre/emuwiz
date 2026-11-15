//! One call for a launch that has just finished: diagnose it, assess its
//! saves, keep a record when it is one to keep, and say what to show.
//!
//! This is what a launcher calls from a worker thread once the process has
//! ended. It reads the settings files it is pointed at and the bounded
//! output it is handed, writes at most one small record into EmuWiz's own
//! launch history, and returns values. It starts nothing, changes no
//! emulator setting and no game file, and every failure to record is a
//! value in the result: nothing here can affect a launch or a later one.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::collect::{LaunchArtefacts, Provenance, collect};
use super::evidence::{ConfigLayer, ExitOutcome};
use super::history::{HistoryStore, LaunchIdentity, NotSaved, SaveOutcome};
use super::notice::{LaunchNotice, launch_notice};
use super::redact::Redactor;
use super::rules::{DiagnosisReport, diagnose};
use super::saves::{self, SaveWriteAssessment};

/// What a launcher knows about the launch it just watched end. All of it
/// was observed by EmuWiz: this is not for facts someone typed.
#[derive(Clone, Debug)]
pub struct FinishedLaunch<'a> {
    /// EmuWiz's short name for the emulator, for example `retroarch`.
    pub emulator: &'a str,
    /// The emulator's name as people write it, for wording.
    pub emulator_display: &'a str,
    pub exit: ExitOutcome,
    pub duration: Option<Duration>,
    /// The wrapper that was started, when it was not the emulator itself.
    pub launcher: Option<&'a str>,
    /// The bounded output the launcher captured, when it captured any.
    pub output: Option<&'a [u8]>,
    /// The settings files in force, each with the layer it occupies.
    pub settings: &'a [(PathBuf, ConfigLayer)],
    pub game_file: Option<&'a Path>,
    pub core: Option<&'a Path>,
    pub save_directory: Option<&'a Path>,
    /// Variables of the environment the emulator was started in.
    pub environment: &'a [(String, Option<String>)],
    /// Who the record is about, for the history.
    pub identity: LaunchIdentity<'a>,
}

/// Everything known about a finished launch.
#[derive(Clone, Debug)]
pub struct LaunchDiagnosis {
    /// The full report, with real paths. For memory only: show it through
    /// [`Redactor::report`], and never store it.
    pub report: DiagnosisReport,
    pub saves: SaveWriteAssessment,
    /// What to show, already redacted.
    pub notice: LaunchNotice,
    /// Whether a record was kept, and if not, why.
    pub recorded: SaveOutcome,
}

/// Diagnose a finished launch and record it when it is one to keep.
/// `history` is `None` when no store is available; the diagnosis is the
/// same and nothing is recorded.
pub fn diagnose_finished_launch(
    launch: &FinishedLaunch<'_>,
    history: Option<&HistoryStore>,
    redactor: &Redactor,
    now: SystemTime,
) -> LaunchDiagnosis {
    let report = diagnose(&collect(&LaunchArtefacts {
        emulator: launch.emulator.to_string(),
        game: launch.identity.title.map(str::to_string),
        exit: Some(launch.exit.clone()),
        duration: launch.duration,
        launcher: launch.launcher.map(str::to_string),
        stderr: launch.output.map(<[u8]>::to_vec),
        log_file: None,
        settings: launch.settings.to_vec(),
        game_file: launch.game_file.map(Path::to_path_buf),
        core: launch.core.map(Path::to_path_buf),
        save_directory: launch.save_directory.map(Path::to_path_buf),
        environment: launch.environment.to_vec(),
        provenance: Provenance::ObservedByEmuWiz,
    }));
    let saves = saves::assess(&report.evidence);
    let notice = launch_notice(&redactor.report(&report), &saves, launch.emulator_display);
    let recorded = match history {
        Some(store) => store.record_launch(&launch.identity, &report, now),
        None => SaveOutcome::NotSaved(NotSaved::StorageUnavailable {
            reason: "no launch history is available".into(),
        }),
    };
    LaunchDiagnosis {
        report,
        saves,
        notice,
        recorded,
    }
}

#[cfg(test)]
#[path = "session/tests.rs"]
mod tests;
