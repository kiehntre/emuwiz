//! Turns "a launch began", "this process was started" and "the launch is
//! over" into session records, for a caller (Blue) that does its own launching.
//!
//! Every call does file work (discovery, the journal) and must run off the
//! drawing thread. Nothing here starts, stops or watches an emulator: it is
//! told what happened by the launcher and looks at saves at the two ends.

use super::discover::{DiscoveryCache, DiscoveryOptions, DiscoveryRoots, discover};
use super::model::DiscoveredSave;
use super::session::{
    ExitObservation, FolderMark, ProcFs, ProcessTable, SaveMark, SessionGame, SessionJournal,
    SessionLaunch, SessionRecord, SessionState,
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
        last_changed_unix: None,
        before_launch: None,
        folder: None,
    })
}

/// When `path` last changed on disk: the later of the modification and
/// status-change times, rounded up to a whole second. For a folder save,
/// the latest over the folder and every entry directly inside it. `None`
/// when anything cannot be checked. The status-change time is set by the
/// system on every write, rename and replacement and cannot be set back by
/// a program.
fn last_changed(path: &Path) -> Option<i64> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.is_dir() {
        // A folder save: the folder and everything directly inside it.
        let mut latest = folder_last_changed(path)?;
        for entry in std::fs::read_dir(path).ok()? {
            latest = latest.max(stamp(&std::fs::symlink_metadata(entry.ok()?.path()).ok()?));
        }
        return Some(latest);
    }
    Some(stamp(&meta))
}

fn stamp(meta: &std::fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    let up = |seconds: i64, nanos: i64| seconds.saturating_add(i64::from(nanos > 0));
    up(meta.mtime(), meta.mtime_nsec()).max(up(meta.ctime(), meta.ctime_nsec()))
}

/// When a folder itself last changed (an entry made, removed or renamed).
fn folder_last_changed(path: &Path) -> Option<i64> {
    Some(stamp(&std::fs::symlink_metadata(path).ok()?))
}

fn clock_ms(clock: libc::clockid_t) -> Option<i64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime only fills the struct it is given.
    if unsafe { libc::clock_gettime(clock, &mut time) } != 0 {
        return None;
    }
    Some(time.tv_sec.saturating_mul(1000) + time.tv_nsec / 1_000_000)
}

/// How far the wall clock is ahead of the time-since-boot clock, in
/// milliseconds: in effect when the machine booted, as the wall clock
/// stands now. It changes only when the wall clock is set or stepped.
///
/// A launcher notes this **before** a launch can start anything and hands
/// it to [`LaunchTracker::process_started`], which works out the launch's
/// start in wall-clock time from it and refuses to prove anything if the
/// wall clock has been stepped since (file times and the start time would
/// then be on different clocks).
pub fn clock_offset_ms() -> Option<i64> {
    Some(clock_ms(libc::CLOCK_REALTIME)? - clock_ms(libc::CLOCK_BOOTTIME)?)
}

/// The wall clock may drift this far against the time-since-boot clock
/// between the note before the launch and the judging (ordinary gradual
/// correction); more is a step, and nothing is then proven.
const CLOCK_STEP_MS: i64 = 1_000;

/// The wall-clock second the launch's first process started in, or `None`
/// when that cannot be stated safely. `clock_before` is
/// [`clock_offset_ms`] as noted before the launch, `clock_now` the same
/// now, `now_unix` the wall clock now.
pub fn launch_start_unix(
    start_ticks: Option<u64>,
    clock_before: Option<i64>,
    clock_now: Option<i64>,
    now_unix: i64,
) -> Option<i64> {
    let (ticks, before, now) = (start_ticks?, clock_before?, clock_now?);
    if (before - now).abs() > CLOCK_STEP_MS {
        return None;
    }
    // SAFETY: sysconf only reads a system constant.
    let per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if per_second <= 0 {
        return None;
    }
    let since_boot_ms = (ticks as i64).saturating_mul(1000) / per_second as i64;
    let start = before.saturating_add(since_boot_ms).div_euclid(1000);
    // A start in the future is no start time at all.
    (start <= now_unix).then_some(start)
}

/// The folder a save sits in.
fn folder_of(save: &DiscoveredSave) -> Option<PathBuf> {
    save.source_path.parent().map(Path::to_path_buf)
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
            // Sandboxed emulators: the system's own crash records, read-only.
            // Wherever that source is missing the answer is simply "not known".
            journal: SessionJournal::open(root, device_id)?
                .with_crash_source(Box::new(super::crash_evidence::ApportLog::system())),
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
        // The launcher may already have started the emulator by now, so
        // these marks are unproven until `process_started` has compared each
        // file's last change (looked up only after its content was read)
        // with the moment the launch's first process began.
        let before: Vec<SaveMark> = self
            .saves()?
            .iter()
            .filter_map(|save| {
                let mut mark = mark_of(save)?;
                mark.last_changed_unix = last_changed(&save.source_path);
                mark.before_launch = Some(false);
                mark.folder = folder_of(save);
                Some(mark)
            })
            .collect();
        // Looked up after the saves were read, like the files' own times.
        let mut folders: Vec<FolderMark> = Vec::new();
        for folder in before.iter().filter_map(|mark| mark.folder.as_ref()) {
            if folders.iter().all(|known| &known.path != folder) {
                folders.push(FolderMark {
                    path: folder.clone(),
                    last_changed_unix: std::fs::symlink_metadata(folder)
                        .ok()
                        .filter(std::fs::Metadata::is_dir)
                        .and_then(|_| folder_last_changed(folder)),
                    before_launch: false,
                });
            }
        }
        let launch = SessionLaunch {
            emulator: expected_emulator.to_string(),
            ..Default::default()
        };
        let record = self.journal.request(game, launch, before)?;
        self.journal.note_folders(&record.session_id, folders)
    }

    /// The launcher started `process` for this session. `clock_before` is
    /// [`clock_offset_ms`] as the launcher noted it before the launch could
    /// start anything; without it no "before" mark is proven.
    pub fn process_started(
        &self,
        session_id: &str,
        process: &StartedProcess,
        clock_before: Option<i64>,
    ) -> Result<SessionRecord, ContinuityError> {
        self.process_started_at(
            session_id,
            process,
            &ProcFs,
            launch_start_unix(
                process.start_ticks,
                clock_before,
                clock_offset_ms(),
                now_unix(),
            ),
        )
    }

    /// As [`Self::process_started_in`], with the start time of the launch's
    /// first process stated by the caller.
    pub fn process_started_at(
        &self,
        session_id: &str,
        process: &StartedProcess,
        processes: &dyn ProcessTable,
        launch_start_unix: Option<i64>,
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
        self.journal
            .judge_baselines(session_id, launch_start_unix)?;
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

    /// Picks up what a launcher has started, and notes that the emulator is
    /// still alive. Call a moment into a launch **and then regularly while
    /// it runs**: a crash record can be tied to the emulator only if it was
    /// seen alive within `CRASH_RECORD_WINDOW_SECONDS` (ten minutes) of
    /// being found gone, and that time is stored at most once a minute, so
    /// the interval between calls has to stay well under nine minutes.
    /// Cheap enough for once every few seconds. An error here is about the
    /// record only; the caller should ignore it, never fail a launch on it.
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
        let mut after = self.marks_for(&record.launch)?;
        if record.folders_before.is_some() {
            let saves = self.saves()?;
            for mark in &mut after {
                mark.folder = saves
                    .iter()
                    .find(|save| save.id == mark.save_id)
                    .and_then(folder_of);
            }
        }
        self.journal
            .mark_ended(session_id, observed, Some(&after), processes)
    }

    /// Settles sessions whose processes are gone. Cheap; safe to call often.
    pub fn settle(&self) -> Vec<SessionRecord> {
        self.journal.reconcile(&ProcFs, now_unix())
    }
}
