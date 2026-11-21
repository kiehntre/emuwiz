//! Turns "a launch began", "this process was started" and "the launch is
//! over" into session records, for a caller (Blue) that does its own launching.
//!
//! Every call does file work (discovery, the journal) and must run off the
//! drawing thread. Nothing here starts, stops or watches an emulator: it is
//! told what happened by the launcher and looks at saves at the two ends.

use super::discover::{DiscoveryCache, DiscoveryOptions, DiscoveryRoots, discover};
use super::model::DiscoveredSave;
use super::session::{
    ExitObservation, ProcFs, ProcessTable, SaveMark, SessionGame, SessionJournal, SessionLaunch,
    SessionRecord, SessionState,
};
use super::{ContinuityError, now_unix};
use crate::launch::process_log::{ExitSummary, StartedProcess};
use std::path::{Path, PathBuf};

/// Flatpak app ids of the emulators whose saves discovery knows.
const FLATPAK_IDS: [(&str, &str); 6] = [
    ("net.pcsx2.PCSX2", "PCSX2"),
    ("org.libretro.RetroArch", "RetroArch"),
    ("org.duckstation.DuckStation", "DuckStation"),
    ("org.DolphinEmu.dolphin-emu", "Dolphin"),
    ("net.rpcs3.RPCS3", "RPCS3"),
    ("org.ppsspp.PPSSPP", "PPSSPP"),
];

/// Executable names (lowercase, start of the file name) of native builds.
const NATIVE_NAMES: [(&str, &str); 6] = [
    ("pcsx2", "PCSX2"),
    ("retroarch", "RetroArch"),
    ("duckstation", "DuckStation"),
    ("dolphin-emu", "Dolphin"),
    ("rpcs3", "RPCS3"),
    ("ppsspp", "PPSSPP"),
];

/// What a started process is, from its executable and first arguments.
/// `through_launcher` is true when the process EmuWiz holds is not the
/// emulator itself, so its exit status is not the emulator's.
pub fn classify_started(executable: &Path, arguments: &[String]) -> SessionLaunch {
    let name = executable
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let flatpak_app = |texts: &mut dyn Iterator<Item = &str>| {
        for text in texts {
            if let Some((_, emulator)) = FLATPAK_IDS.iter().find(|(id, _)| text.contains(id)) {
                return Some((*emulator).to_string());
            }
        }
        None
    };
    let launch = |emulator: String, profile: &str, through_launcher: bool| SessionLaunch {
        emulator,
        profile: profile.to_string(),
        core: None,
        emulator_version: None,
        through_launcher,
    };
    if name == "flatpak" {
        let emulator = flatpak_app(&mut arguments.iter().map(String::as_str))
            .unwrap_or_else(|| "Unknown".to_string());
        return launch(emulator, "flatpak", true);
    }
    if let Some((_, emulator)) = NATIVE_NAMES
        .iter()
        .find(|(prefix, _)| name.starts_with(prefix))
    {
        // A file called like the emulator may still be a small script that
        // hands over to `flatpak run` (a common ~/.local/bin wrapper).
        // If the file cannot be read, what it is cannot be told: treated as
        // a launcher, so its exit status is never taken for the emulator's.
        let Ok(bytes) = std::fs::read(executable) else {
            return launch((*emulator).to_string(), "native", true);
        };
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]).into_owned();
        if head.starts_with("#!") {
            return match flatpak_app(&mut head.lines()) {
                Some(emulator) => launch(emulator, "flatpak", true),
                None => launch((*emulator).to_string(), "native", true),
            };
        }
        return launch((*emulator).to_string(), "native", false);
    }
    launch(
        executable
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Unknown".to_string()),
        "native",
        // Not a program EmuWiz recognises as an emulator itself: it may be
        // a launcher (`gamemoderun`, `env`, a script), so its exit status is
        // never taken for an emulator's.
        true,
    )
}

fn mark_of(save: &DiscoveredSave) -> Option<SaveMark> {
    Some(SaveMark {
        save_id: save.id.clone(),
        label: match (&save.identity.title, &save.entry_name) {
            (Some(title), Some(name)) => format!("{title} [{name}]"),
            (None, Some(name)) => name.clone(),
            _ => save
                .source_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        },
        content_sha256: save.content_sha256.clone()?,
    })
}

pub struct LaunchTracker {
    root: PathBuf,
    roots: DiscoveryRoots,
    journal: SessionJournal,
    device_id: String,
}

impl LaunchTracker {
    /// `root` is the save-continuity folder; `roots` is where saves are
    /// looked for (the person's profile, or a scratch one).
    pub fn open(
        root: &Path,
        roots: DiscoveryRoots,
        device_id: &str,
    ) -> Result<Self, ContinuityError> {
        Ok(Self {
            root: root.to_path_buf(),
            roots,
            journal: SessionJournal::open(root, device_id)?,
            device_id: device_id.to_string(),
        })
    }

    pub fn journal(&self) -> &SessionJournal {
        &self.journal
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Every save, read through the on-disk cache so an unchanged file is
    /// not read again. No in-use check: this is only for content marks.
    fn saves(&self) -> Result<Vec<DiscoveredSave>, ContinuityError> {
        let cache_file = self.root.join("discovery-cache.json");
        let mut cache = DiscoveryCache::load(&cache_file);
        let report = discover(
            &self.roots,
            DiscoveryOptions {
                cache: Some(&mut cache),
                ..Default::default()
            },
        )?;
        // A cache that cannot be written only costs time next launch.
        let _ = cache.save(&cache_file);
        Ok(report.saves)
    }

    fn marks_for(&self, launch: &SessionLaunch) -> Result<Vec<SaveMark>, ContinuityError> {
        Ok(self
            .saves()?
            .iter()
            .filter(|save| {
                save.emulator.eq_ignore_ascii_case(&launch.emulator)
                    && save.profile.eq_ignore_ascii_case(&launch.profile)
            })
            .filter_map(mark_of)
            .collect())
    }

    /// A launch was asked for. Which emulator will really start is not known
    /// yet, so every save's content is marked; the session keeps only the
    /// right emulator's once the process exists.
    pub fn begin(
        &self,
        game: SessionGame,
        expected_emulator: &str,
    ) -> Result<SessionRecord, ContinuityError> {
        // Settle anything a previous run of EmuWiz left open first.
        self.journal.reconcile(&ProcFs, now_unix());
        let before: Vec<SaveMark> = self.saves()?.iter().filter_map(mark_of).collect();
        let launch = SessionLaunch {
            emulator: expected_emulator.to_string(),
            ..Default::default()
        };
        self.journal.request(game, launch, before)
    }

    /// The launcher started `process` for this session.
    pub fn process_started(
        &self,
        session_id: &str,
        process: &StartedProcess,
    ) -> Result<SessionRecord, ContinuityError> {
        self.process_started_in(session_id, process, &ProcFs)
    }

    pub fn process_started_in(
        &self,
        session_id: &str,
        process: &StartedProcess,
        processes: &dyn ProcessTable,
    ) -> Result<SessionRecord, ContinuityError> {
        let launch = classify_started(&process.executable, &process.arguments);
        // First, and quickly, note the process itself; only then the slower
        // look at the saves.
        self.journal.mark_started_as(
            session_id,
            process.pid,
            launch.clone(),
            process.start_ticks,
            processes,
        )?;
        let its_saves: Vec<String> = self
            .saves()?
            .iter()
            .filter(|save| {
                save.emulator.eq_ignore_ascii_case(&launch.emulator)
                    && save.profile.eq_ignore_ascii_case(&launch.profile)
            })
            .map(|save| save.id.clone())
            .collect();
        self.journal.keep_saves(session_id, &its_saves)
    }

    /// A moment into a launch: picks up what a launcher has started.
    pub fn look_again(&self, session_id: &str) -> Result<SessionRecord, ContinuityError> {
        self.journal.observe_running(session_id, &ProcFs)
    }

    /// The launch is over. `exit` is the started process's own exit, when a
    /// process was started at all; `failure` is the launcher's explanation
    /// when it could not start one.
    pub fn finished(
        &self,
        session_id: &str,
        exit: Option<ExitSummary>,
        failure: Option<&str>,
    ) -> Result<SessionRecord, ContinuityError> {
        self.finished_in(session_id, exit, failure, &ProcFs)
    }

    pub fn finished_in(
        &self,
        session_id: &str,
        exit: Option<ExitSummary>,
        failure: Option<&str>,
        processes: &dyn ProcessTable,
    ) -> Result<SessionRecord, ContinuityError> {
        let record = self.journal.get(session_id)?;
        if record.state == SessionState::Requested {
            return self.journal.mark_never_started(
                session_id,
                failure.unwrap_or("the launch ended without an emulator process being started"),
            );
        }
        let observed = match exit {
            Some(ExitSummary::Code(code)) => ExitObservation::Code(code),
            Some(ExitSummary::Signal {
                signal,
                core_dumped,
            }) => ExitObservation::Signal {
                signal,
                core_dumped,
            },
            Some(ExitSummary::Unavailable) | None => ExitObservation::Unavailable,
        };
        let after = self.marks_for(&record.launch)?;
        self.journal
            .mark_ended(session_id, observed, Some(&after), processes)
    }

    /// Settles sessions whose processes are gone. Cheap; safe to call often.
    pub fn settle(&self) -> Vec<SessionRecord> {
        self.journal.reconcile(&ProcFs, now_unix())
    }
}
