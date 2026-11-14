//! Assemble an [`EvidenceSet`] from what a finished launch left behind.
//!
//! Everything here is a read: files are opened without following a final
//! symbolic link and read up to a fixed limit, paths are only looked at.
//! Nothing is launched, written or changed. Every input that was not
//! supplied is recorded as not collected, so the report shows the gap
//! instead of implying the area was clean.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::evidence::{
    ConfigLayer, EvidenceArea, EvidenceSet, ExitOutcome, Fact, LogStream, PathRole, Subject, Trust,
    observe_path_access,
};
use super::{process, retroarch, text};
use crate::launch::process_spawn::open_regular_file_nonblocking;

/// What is known about one launch. Every field is optional: supply what
/// exists.
#[derive(Clone, Debug, Default)]
pub struct LaunchArtefacts {
    /// Short emulator name, for example `retroarch`.
    pub emulator: String,
    /// Display name of the game, when known.
    pub game: Option<String>,
    pub exit: Option<ExitOutcome>,
    pub duration: Option<Duration>,
    /// The wrapper that was started when it was not the emulator itself.
    pub launcher: Option<String>,
    /// Captured standard error of the launched process.
    pub stderr: Option<Vec<u8>>,
    /// The emulator's own log file.
    pub log_file: Option<PathBuf>,
    /// Settings files, each with the layer it occupies.
    pub settings: Vec<(PathBuf, ConfigLayer)>,
    pub game_file: Option<PathBuf>,
    pub core: Option<PathBuf>,
    pub save_directory: Option<PathBuf>,
    /// Variables of the environment the emulator was started in, with
    /// `None` for a variable that was checked and found unset.
    pub environment: Vec<(String, Option<String>)>,
}

/// Read at most [`text::LOG_BYTES_LIMIT`] bytes (plus one, so that a longer
/// file is recognised as cut) from a regular file, without following a
/// final symbolic link.
pub fn read_log_prefix(path: &Path) -> std::io::Result<Vec<u8>> {
    let file = open_regular_file_nonblocking(path)?;
    let mut bytes = Vec::new();
    file.take(text::LOG_BYTES_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Build the evidence for one launch.
pub fn collect(artefacts: &LaunchArtefacts) -> EvidenceSet {
    let emulator = artefacts.emulator.as_str();
    let is_retroarch = emulator == retroarch::EMULATOR;
    let mut set = EvidenceSet::new(Subject {
        emulator: Some(text::sanitise(emulator, text::VALUE_LIMIT)),
        game: artefacts
            .game
            .as_deref()
            .map(|game| text::sanitise(game, text::VALUE_LIMIT)),
    });

    match &artefacts.exit {
        Some(outcome) => {
            process::record_exit(
                &mut set,
                outcome.clone(),
                artefacts.duration,
                artefacts.launcher.as_deref(),
            );
        }
        None => {
            set.not_collected(
                EvidenceArea::ProcessExit,
                "how the emulator process ended was not supplied",
            );
        }
    }

    let emulator_signatures = if is_retroarch {
        retroarch::SIGNATURES
    } else {
        &[]
    };
    let mut any_log = false;
    if let Some(stderr) = &artefacts.stderr {
        any_log = true;
        if is_retroarch {
            // One pass that also decides whether the output was verbose.
            retroarch::record_log(&mut set, stderr, LogStream::Stderr);
            process::record_stderr(&mut set, stderr, emulator, &[]);
        } else {
            process::record_stderr(&mut set, stderr, emulator, emulator_signatures);
        }
    }
    if let Some(log_file) = &artefacts.log_file {
        any_log = true;
        match read_log_prefix(log_file) {
            Ok(bytes) if is_retroarch => {
                retroarch::record_log(&mut set, &bytes, LogStream::LogFile);
            }
            Ok(_) => {
                set.not_collected(
                    EvidenceArea::LogEvents,
                    format!("no log reader exists for {emulator} yet"),
                );
            }
            Err(error) => {
                set.unavailable(
                    EvidenceArea::LogEvents,
                    format!("log file {}", log_file.display()),
                    format!("could not be read: {}", error.kind()),
                );
            }
        }
    }
    if !any_log {
        set.not_collected(
            EvidenceArea::LogEvents,
            "no emulator output or log file was supplied",
        );
    }

    if artefacts.settings.is_empty() {
        set.not_collected(
            EvidenceArea::EmulatorConfiguration,
            "no settings file was supplied",
        );
    }
    for (path, layer) in &artefacts.settings {
        if is_retroarch {
            retroarch::record_config(&mut set, path, *layer);
        } else {
            set.not_collected(
                EvidenceArea::EmulatorConfiguration,
                format!("no settings reader exists for {emulator} yet"),
            );
        }
    }

    let paths = [
        (PathRole::GameFile, &artefacts.game_file),
        (PathRole::EmulatorCore, &artefacts.core),
        (PathRole::SaveDirectory, &artefacts.save_directory),
    ];
    if paths.iter().all(|(_, path)| path.is_none()) {
        set.not_collected(
            EvidenceArea::Filesystem,
            "no game file, core or save folder was supplied",
        );
    }
    for (role, path) in paths {
        if let Some(path) = path {
            set.observe(
                EvidenceArea::Filesystem,
                Trust::Measured,
                "read-only look at the path",
                Fact::PathAccess(observe_path_access(role, path)),
            );
        }
    }

    if artefacts.environment.is_empty() {
        set.not_collected(
            EvidenceArea::LaunchEnvironment,
            "the launch environment was not supplied",
        );
    }
    for (name, value) in &artefacts.environment {
        set.observe(
            EvidenceArea::LaunchEnvironment,
            Trust::Measured,
            "environment the emulator was started in",
            Fact::EnvironmentVariable {
                name: text::sanitise(name, text::VALUE_LIMIT),
                value: value
                    .as_deref()
                    .map(|value| text::sanitise(value, text::PATH_LIMIT)),
            },
        );
    }

    // A file read a moment ago can already have changed.
    set.refresh_freshness();
    set
}

#[cfg(test)]
#[path = "collect/tests.rs"]
mod tests;
