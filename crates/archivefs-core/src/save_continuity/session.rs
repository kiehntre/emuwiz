//! Play-session journal.
//!
//! One small JSON record per launch, in `<root>/sessions/`, rewritten whole
//! (work file, then rename) at each step. The steps are kept apart because
//! they are different facts:
//!
//! 1. **Requested**: EmuWiz was asked to start a game.
//! 2. **Started**: an emulator process exists (its id and start time are
//!    recorded, so a recycled process id is never mistaken for it).
//! 3. **Ended**: EmuWiz saw the process end, cleanly or not.
//!
//! A record that never reaches the next step is settled by
//! [`SessionJournal::reconcile`] the next time EmuWiz looks: "never started"
//! or "end not seen". Neither is reported as a clean finish.
//!
//! A started process is not proof a game was played. The record says only
//! what was observed: how long the process ran, and whether a save's content
//! changed. Nothing here watches the emulator or scans the disk on a timer.

use super::status::{PrecedingSession, is_crash_signal};
use super::{ContinuityError, now_unix};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const SESSION_FORMAT_VERSION: u32 = 1;
/// A request with no process after this long is settled as "never started".
pub const START_GRACE_SECONDS: i64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Requested,
    Started,
    Ended,
    /// No process was ever recorded for the request.
    NeverStarted,
    /// The process is gone and EmuWiz did not see it end (EmuWiz was closed,
    /// crashed, or the machine shut down). How it ended is not known.
    EndNotSeen,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "how")]
pub enum SessionEnd {
    /// Exit status 0 from the emulator itself.
    Clean,
    /// Exit status 0, but from a launcher (`flatpak run`, a wrapper), which
    /// says nothing about the emulator inside.
    LauncherExitedOk,
    ExitCode {
        code: i32,
    },
    Signal {
        signal: i32,
        core_dumped: bool,
    },
    /// The exit status could not be read.
    StatusUnavailable,
}

impl SessionEnd {
    pub fn label(&self) -> String {
        match self {
            Self::Clean => "Closed normally".to_string(),
            Self::LauncherExitedOk => {
                "The launcher finished normally (the emulator's own result is not known)"
                    .to_string()
            }
            Self::ExitCode { code } => format!("Stopped with an error (exit code {code})"),
            Self::Signal {
                signal,
                core_dumped: true,
            } => format!("Crashed (signal {signal})"),
            Self::Signal { signal, .. } => format!("Was stopped (signal {signal})"),
            Self::StatusUnavailable => "Ended; how is not known".to_string(),
        }
    }

    pub fn abnormal(&self) -> bool {
        matches!(self, Self::ExitCode { .. } | Self::Signal { .. })
    }
}

/// What was actually observed about the game having run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PlayEvidence {
    /// Nothing beyond "a process was started".
    None,
    /// The process stayed up this long. Not proof of play.
    ProcessRan { seconds: i64 },
    /// A save's content was different after the session.
    SaveChanged { seconds: i64 },
}

/// One save's content at one moment, for before/after comparison.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SaveMark {
    /// Stable key: the discovered save's id.
    pub save_id: String,
    pub label: String,
    pub content_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "change")]
pub enum SaveChange {
    Created {
        label: String,
    },
    Modified {
        label: String,
    },
    /// Present before, absent after. Reported, never "cleaned up".
    Missing {
        label: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionGame {
    pub library_id: Option<i64>,
    pub title: String,
    pub product_code: Option<String>,
    pub content_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLaunch {
    pub emulator: String,
    pub profile: String,
    pub core: Option<String>,
    pub emulator_version: Option<String>,
    /// The process EmuWiz starts is a launcher, not the emulator itself.
    pub through_launcher: bool,
}

/// One process belonging to a session, identified so that a later process
/// with the same id is never mistaken for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedProcess {
    pub pid: u32,
    /// Start time in kernel clock ticks; with the pid, unique until reboot.
    pub start_ticks: u64,
    /// What `/proc/<pid>/exe` pointed at when first seen.
    pub executable: Option<PathBuf>,
    /// Started by EmuWiz itself (false: found as a child of one that was).
    pub launched_by_emuwiz: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub format_version: u32,
    pub session_id: String,
    pub device_id: String,
    pub game: SessionGame,
    pub launch: SessionLaunch,
    pub state: SessionState,
    pub requested_unix: i64,
    pub started_unix: Option<i64>,
    pub ended_unix: Option<i64>,
    pub process_id: Option<u32>,
    /// The process's start time in kernel clock ticks (`/proc/<pid>/stat`).
    pub process_start_ticks: Option<u64>,
    /// The launched process and every descendant seen so far.
    #[serde(default)]
    pub processes: Vec<TrackedProcess>,
    /// The launched process ended while a descendant was still running;
    /// this is how the launched process itself ended.
    #[serde(default)]
    pub launcher_end: Option<SessionEnd>,
    /// The Flatpak app scope the session's processes run in, when there is one.
    #[serde(default)]
    pub app_scope: Option<String>,
    pub end: Option<SessionEnd>,
    pub play_evidence: PlayEvidence,
    pub saves_before: Vec<SaveMark>,
    /// The saves as they were once the session had ended, when they were
    /// compared. `None`: not compared (a crash, an exit EmuWiz did not see).
    #[serde(default)]
    pub saves_after: Option<Vec<SaveMark>>,
    pub save_changes: Vec<SaveChange>,
    /// Snapshots taken for this session (ids).
    pub snapshots: Vec<String>,
    pub notes: Vec<String>,
}

impl SessionRecord {
    pub fn duration_seconds(&self) -> Option<i64> {
        Some((self.ended_unix? - self.started_unix?).max(0))
    }

    /// A one-line, honest status for a list.
    pub fn status_line(&self) -> String {
        match (&self.state, &self.end) {
            (SessionState::Requested, _) => "Starting…".to_string(),
            (SessionState::Started, _) => "Running".to_string(),
            (SessionState::NeverStarted, _) => "Did not start".to_string(),
            (SessionState::EndNotSeen, _) => {
                "Ended while EmuWiz was not watching; how is not known".to_string()
            }
            (SessionState::Ended, Some(end)) => end.label(),
            (SessionState::Ended, None) => "Ended".to_string(),
        }
    }

    pub fn needs_attention(&self) -> bool {
        matches!(
            self.state,
            SessionState::EndNotSeen | SessionState::NeverStarted
        ) || self.end.as_ref().is_some_and(SessionEnd::abnormal)
            || self
                .save_changes
                .iter()
                .any(|change| matches!(change, SaveChange::Missing { .. }))
    }
}

/// How the process ended, as the launcher saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitObservation {
    Code(i32),
    Signal { signal: i32, core_dumped: bool },
    Unavailable,
}

impl ExitObservation {
    pub fn from_status(status: &std::io::Result<std::process::ExitStatus>) -> Self {
        use std::os::unix::process::ExitStatusExt;
        match status {
            Ok(status) => match (status.code(), status.signal()) {
                (Some(code), _) => Self::Code(code),
                (None, Some(signal)) => Self::Signal {
                    signal,
                    core_dumped: status.core_dumped(),
                },
                _ => Self::Unavailable,
            },
            Err(_) => Self::Unavailable,
        }
    }
}

/// Answers "is this exact process still alive?".
pub trait ProcessTable {
    /// The start time (clock ticks) of process `pid`, or `None` if there is
    /// no such process.
    fn start_ticks(&self, pid: u32) -> Option<u64>;
    fn executable(&self, _pid: u32) -> Option<PathBuf> {
        None
    }
    /// Every process whose chain of parents leads to `pid`.
    fn descendants(&self, _pid: u32) -> Vec<u32> {
        Vec::new()
    }
    /// The Flatpak app scope `pid` runs in, if it runs in one. Every process
    /// of one `flatpak run` shares it, whoever their parent is.
    fn app_scope(&self, _pid: u32) -> Option<String> {
        None
    }
    /// Every process currently in `scope`.
    fn in_scope(&self, _scope: &str) -> Vec<u32> {
        Vec::new()
    }
}

fn flatpak_scope(cgroup: &str) -> Option<String> {
    cgroup
        .lines()
        .filter_map(|line| line.rsplit('/').next())
        .find(|leaf| leaf.starts_with("app-flatpak-") && leaf.ends_with(".scope"))
        .map(str::to_string)
}

/// `/proc/<pid>/stat`, field 22.
pub struct ProcFs;

impl ProcessTable for ProcFs {
    fn start_ticks(&self, pid: u32) -> Option<u64> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // The command name is in parentheses and may contain spaces.
        let after = stat.rsplit_once(')')?.1;
        after.split_whitespace().nth(19)?.parse().ok()
    }

    fn executable(&self, pid: u32) -> Option<PathBuf> {
        fs::read_link(format!("/proc/{pid}/exe")).ok()
    }

    fn app_scope(&self, pid: u32) -> Option<String> {
        flatpak_scope(&fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?)
    }

    fn in_scope(&self, scope: &str) -> Vec<u32> {
        fs::read_dir("/proc")
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
            .filter(|pid| {
                fs::read_to_string(format!("/proc/{pid}/cgroup"))
                    .is_ok_and(|cgroup| flatpak_scope(&cgroup).as_deref() == Some(scope))
            })
            .collect()
    }

    fn descendants(&self, pid: u32) -> Vec<u32> {
        // One pass over /proc: child -> parent, then walk down from `pid`.
        let mut parent_of: BTreeMap<u32, u32> = BTreeMap::new();
        for entry in fs::read_dir("/proc").into_iter().flatten().flatten() {
            let Some(child) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
                continue;
            };
            let parent = stat
                .rsplit_once(')')
                .and_then(|(_, after)| after.split_whitespace().nth(1)?.parse::<u32>().ok());
            if let Some(parent) = parent {
                parent_of.insert(child, parent);
            }
        }
        let mut found = vec![pid];
        let mut index = 0;
        while index < found.len() {
            let current = found[index];
            found.extend(
                parent_of
                    .iter()
                    .filter(|(child, parent)| **parent == current && !found.contains(child))
                    .map(|(child, _)| *child)
                    .collect::<Vec<_>>(),
            );
            index += 1;
        }
        found.remove(0);
        found
    }
}

fn track(record: &mut SessionRecord, pid: u32, launched: bool, processes: &dyn ProcessTable) {
    let Some(start_ticks) = processes.start_ticks(pid) else {
        return;
    };
    if record
        .processes
        .iter()
        .any(|known| known.pid == pid && known.start_ticks == start_ticks)
    {
        return;
    }
    record.processes.push(TrackedProcess {
        pid,
        start_ticks,
        executable: processes.executable(pid),
        launched_by_emuwiz: launched,
    });
}

fn alive(process: &TrackedProcess, processes: &dyn ProcessTable) -> bool {
    processes.start_ticks(process.pid) == Some(process.start_ticks)
}

/// Adds descendants that have appeared since the last look. A wrapper
/// (`flatpak run`, an AppImage runtime, a shell script) starts the emulator
/// as a child; this is how that child becomes part of the session.
fn refresh(record: &mut SessionRecord, processes: &dyn ProcessTable) {
    let live: Vec<u32> = record
        .processes
        .iter()
        .filter(|process| alive(process, processes))
        .map(|process| process.pid)
        .collect();
    for pid in live {
        for child in processes.descendants(pid) {
            track(record, child, false, processes);
        }
        if record.app_scope.is_none() {
            record.app_scope = processes.app_scope(pid);
        }
    }
    // Once a launcher has exited its children belong to another parent, so
    // the parent chain no longer finds them. The app scope still does.
    if let Some(scope) = record.app_scope.clone() {
        for member in processes.in_scope(&scope) {
            track(record, member, false, processes);
        }
    }
}

pub struct SessionJournal {
    dir: PathBuf,
    device_id: String,
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn diff_saves(before: &[SaveMark], after: &[SaveMark]) -> Vec<SaveChange> {
    let index = |marks: &[SaveMark]| -> BTreeMap<String, SaveMark> {
        marks
            .iter()
            .map(|mark| (mark.save_id.clone(), mark.clone()))
            .collect()
    };
    let (old, new) = (index(before), index(after));
    let mut changes = Vec::new();
    for (id, mark) in &new {
        match old.get(id) {
            None => changes.push(SaveChange::Created {
                label: mark.label.clone(),
            }),
            Some(was) if was.content_sha256 != mark.content_sha256 => {
                changes.push(SaveChange::Modified {
                    label: mark.label.clone(),
                });
            }
            Some(_) => {}
        }
    }
    for (id, mark) in &old {
        if !new.contains_key(id) {
            changes.push(SaveChange::Missing {
                label: mark.label.clone(),
            });
        }
    }
    changes
}

impl SessionJournal {
    pub fn open(root: &Path, device_id: &str) -> Result<Self, ContinuityError> {
        let dir = root.join("sessions");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(|error| super::io("cannot create", &dir, error))?;
        Ok(Self {
            dir,
            device_id: device_id.to_string(),
        })
    }

    fn path(&self, record: &SessionRecord) -> PathBuf {
        self.dir.join(format!(
            "{:012}-{}.json",
            record.requested_unix.max(0),
            record.session_id
        ))
    }

    fn write(&self, record: &SessionRecord) -> Result<(), ContinuityError> {
        let path = self.path(record);
        let work = path.with_extension(format!(
            "part-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let text = serde_json::to_vec_pretty(record)
            .map_err(|error| ContinuityError::Io(error.to_string()))?;
        let result = fs::write(&work, text)
            .and_then(|()| fs::File::open(&work)?.sync_all())
            .and_then(|()| fs::rename(&work, &path));
        if let Err(error) = result {
            let _ = fs::remove_file(&work);
            return Err(super::io("cannot record the session in", &path, error));
        }
        Ok(())
    }

    /// Step 1: a launch was asked for.
    pub fn request(
        &self,
        game: SessionGame,
        launch: SessionLaunch,
        saves_before: Vec<SaveMark>,
    ) -> Result<SessionRecord, ContinuityError> {
        let requested_unix = now_unix();
        let seed = format!(
            "{}\0{}\0{}\0{}\0{}",
            self.device_id,
            requested_unix,
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            game.title
        );
        let record = SessionRecord {
            format_version: SESSION_FORMAT_VERSION,
            session_id: super::sha256_hex(seed.as_bytes())[..16].to_string(),
            device_id: self.device_id.clone(),
            game,
            launch,
            state: SessionState::Requested,
            requested_unix,
            started_unix: None,
            ended_unix: None,
            process_id: None,
            process_start_ticks: None,
            processes: Vec::new(),
            launcher_end: None,
            app_scope: None,
            end: None,
            play_evidence: PlayEvidence::None,
            saves_before,
            saves_after: None,
            save_changes: Vec::new(),
            snapshots: Vec::new(),
            notes: Vec::new(),
        };
        self.write(&record)?;
        Ok(record)
    }

    pub fn get(&self, session_id: &str) -> Result<SessionRecord, ContinuityError> {
        self.all()
            .into_iter()
            .find(|record| record.session_id == session_id)
            .ok_or_else(|| ContinuityError::NotFound(format!("session {session_id}")))
    }

    /// Step 2: the process exists.
    pub fn mark_started(
        &self,
        session_id: &str,
        pid: u32,
        processes: &dyn ProcessTable,
    ) -> Result<SessionRecord, ContinuityError> {
        let mut record = self.get(session_id)?;
        if record.state != SessionState::Requested {
            return Err(ContinuityError::Refused(format!(
                "session {session_id} is already past starting"
            )));
        }
        record.state = SessionState::Started;
        record.started_unix = Some(now_unix());
        record.process_id = Some(pid);
        record.process_start_ticks = processes.start_ticks(pid);
        track(&mut record, pid, true, processes);
        refresh(&mut record, processes);
        self.write(&record)?;
        Ok(record)
    }

    /// Looks again for processes the session's processes have started. A
    /// launcher starts the emulator a moment after it starts itself, and
    /// once the launcher exits its children can no longer be found through
    /// it, so a launcher-based session should call this a few seconds in.
    /// Cheap: one pass over the process list.
    pub fn observe_running(
        &self,
        session_id: &str,
        processes: &dyn ProcessTable,
    ) -> Result<SessionRecord, ContinuityError> {
        let mut record = self.get(session_id)?;
        if record.state == SessionState::Started {
            let before = (record.processes.len(), record.app_scope.clone());
            refresh(&mut record, processes);
            if before != (record.processes.len(), record.app_scope.clone()) {
                self.write(&record)?;
            }
        }
        Ok(record)
    }

    /// The launch failed before any process existed.
    pub fn mark_never_started(
        &self,
        session_id: &str,
        why: &str,
    ) -> Result<SessionRecord, ContinuityError> {
        let mut record = self.get(session_id)?;
        if record.state != SessionState::Requested {
            return Err(ContinuityError::Refused(format!(
                "session {session_id} did start"
            )));
        }
        record.state = SessionState::NeverStarted;
        record.ended_unix = Some(now_unix());
        record.notes.push(why.to_string());
        self.write(&record)?;
        Ok(record)
    }

    /// Step 3: EmuWiz saw the process end.
    ///
    /// If the launched process was only a launcher and a process it started
    /// is still alive, the session is **not** ended: it stays `Started`, the
    /// launcher's result is kept, and [`Self::reconcile`] settles it once
    /// every tracked process is gone. Saves are compared only then.
    pub fn mark_ended(
        &self,
        session_id: &str,
        exit: ExitObservation,
        saves_after: Option<&[SaveMark]>,
        processes: &dyn ProcessTable,
    ) -> Result<SessionRecord, ContinuityError> {
        let mut record = self.get(session_id)?;
        if record.state != SessionState::Started || record.launcher_end.is_some() {
            return Err(ContinuityError::Refused(format!(
                "session {session_id} is not running"
            )));
        }
        refresh(&mut record, processes);
        let end = match exit {
            ExitObservation::Code(0) if record.launch.through_launcher => {
                SessionEnd::LauncherExitedOk
            }
            ExitObservation::Code(0) => SessionEnd::Clean,
            ExitObservation::Code(code) => SessionEnd::ExitCode { code },
            ExitObservation::Signal {
                signal,
                core_dumped,
            } => SessionEnd::Signal {
                signal,
                core_dumped,
            },
            ExitObservation::Unavailable => SessionEnd::StatusUnavailable,
        };
        let launched = record.process_id;
        let still_running = record
            .processes
            .iter()
            .filter(|process| Some(process.pid) != launched)
            .any(|process| alive(process, processes));
        if still_running {
            record.launcher_end = Some(end);
            record.notes.push(
                "the process EmuWiz started has finished, but a process it started is still running; the session is still open"
                    .to_string(),
            );
            self.write(&record)?;
            return Ok(record);
        }
        record.state = SessionState::Ended;
        record.ended_unix = Some(now_unix());
        record.end = Some(end);
        self.finish(&mut record, saves_after);
        self.write(&record)?;
        Ok(record)
    }

    fn finish(&self, record: &mut SessionRecord, saves_after: Option<&[SaveMark]>) {
        let seconds = record.duration_seconds().unwrap_or(0);
        match saves_after {
            Some(after) => {
                record.saves_after = Some(after.to_vec());
                record.save_changes = diff_saves(&record.saves_before, after);
                let wrote = record.save_changes.iter().any(|change| {
                    matches!(
                        change,
                        SaveChange::Created { .. } | SaveChange::Modified { .. }
                    )
                });
                record.play_evidence = if wrote {
                    PlayEvidence::SaveChanged { seconds }
                } else {
                    PlayEvidence::ProcessRan { seconds }
                };
            }
            None => {
                record.play_evidence = PlayEvidence::ProcessRan { seconds };
                record
                    .notes
                    .push("saves were not compared after this session".to_string());
            }
        }
    }

    pub fn link_snapshot(
        &self,
        session_id: &str,
        snapshot_id: &str,
    ) -> Result<SessionRecord, ContinuityError> {
        let mut record = self.get(session_id)?;
        if !record.snapshots.iter().any(|id| id == snapshot_id) {
            record.snapshots.push(snapshot_id.to_string());
            self.write(&record)?;
        }
        Ok(record)
    }

    /// Settles records left open by a crash, a shutdown or a closed EmuWiz.
    /// Returns the records it changed.
    pub fn reconcile(&self, processes: &dyn ProcessTable, now: i64) -> Vec<SessionRecord> {
        let mut settled = Vec::new();
        for mut record in self.all() {
            match record.state {
                SessionState::Requested if now - record.requested_unix > START_GRACE_SECONDS => {
                    record.state = SessionState::NeverStarted;
                    record
                        .notes
                        .push("no emulator process was ever recorded for this request".to_string());
                }
                SessionState::Started => {
                    let before = record.processes.len();
                    refresh(&mut record, processes);
                    // A process counts only while both its id and its start
                    // time match: a recycled id is a different process.
                    if record
                        .processes
                        .iter()
                        .any(|process| alive(process, processes))
                    {
                        if record.processes.len() != before {
                            let _ = self.write(&record);
                        }
                        continue;
                    }
                    record.ended_unix = Some(now);
                    if let Some(end) = record.launcher_end.take() {
                        // The launcher's end was seen; the emulator's own
                        // was not. Say exactly that.
                        record.state = SessionState::Ended;
                        record.end = Some(match end {
                            SessionEnd::Clean => SessionEnd::LauncherExitedOk,
                            other => other,
                        });
                        record.notes.push(
                            "the emulator kept running after its launcher finished; EmuWiz saw that it has now stopped, not how"
                                .to_string(),
                        );
                        if self.write(&record).is_ok() {
                            settled.push(record);
                        }
                        continue;
                    }
                    record.state = SessionState::EndNotSeen;
                    record.notes.push(
                        "the emulator process is gone and EmuWiz did not see it end; whether it closed normally is not known, \
                         and its last save may be incomplete"
                            .to_string(),
                    );
                }
                _ => continue,
            }
            if self.write(&record).is_ok() {
                settled.push(record);
            }
        }
        settled
    }

    /// Every readable record, newest first. File names sort by request time.
    pub fn all(&self) -> Vec<SessionRecord> {
        self.recent(usize::MAX)
    }

    /// The newest `limit` records. Reads only that many files.
    pub fn recent(&self, limit: usize) -> Vec<SessionRecord> {
        let mut names: Vec<PathBuf> = fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        names.sort();
        names
            .iter()
            .rev()
            .filter_map(|path| {
                let bytes = fs::read(path).ok()?;
                let record: SessionRecord = serde_json::from_slice(&bytes).ok()?;
                (record.format_version == SESSION_FORMAT_VERSION).then_some(record)
            })
            .take(limit)
            .collect()
    }
}

/// Decides how the emulator ended the last time it could have written the
/// saves in `current`, from recorded sessions (`sessions`, newest first).
///
/// A session speaks for a save only when all of this holds; anything else is
/// [`PrecedingSession::Unknown`], with the reason:
///
/// - it is the **most recent** session of the same emulator and install on
///   this device that actually started (an older one says nothing once a
///   newer one has run);
/// - it **recorded this save** (by the save's id) before or after;
/// - when the saves were compared after the session, each save's content is
///   **still what it was then** (otherwise something has written it since,
///   outside any recorded session);
/// - a normal exit only counts if the saves were compared afterwards.
///
/// What this cannot see: an emulator started outside EmuWiz leaves no
/// session at all, so the answer after one is "unknown", never "clean".
pub fn preceding_session(
    sessions: &[SessionRecord],
    device_id: &str,
    emulator: &str,
    profile: &str,
    current: &[SaveMark],
) -> PrecedingSession {
    let Some(latest) = sessions.iter().find(|session| {
        session.device_id == device_id
            && session.launch.emulator.eq_ignore_ascii_case(emulator)
            && session.launch.profile.eq_ignore_ascii_case(profile)
            && session.state != SessionState::NeverStarted
    }) else {
        return PrecedingSession::unknown(&format!(
            "EmuWiz has no recorded session of {emulator} ({profile}) on this device"
        ));
    };
    if current.is_empty() {
        return PrecedingSession::unknown("this save has no identity to match against a session");
    }
    let id = latest.session_id.clone();
    if matches!(
        latest.state,
        SessionState::Requested | SessionState::Started
    ) {
        return PrecedingSession::unknown(
            "the most recent session of this emulator is still recorded as open",
        );
    }
    let listed = |marks: &[SaveMark]| {
        current
            .iter()
            .all(|now| marks.iter().any(|mark| mark.save_id == now.save_id))
    };
    let recorded =
        listed(&latest.saves_before) || latest.saves_after.as_deref().is_some_and(listed);
    if !recorded {
        return PrecedingSession::unknown(&format!(
            "the most recent {emulator} session did not record this save, so what happened to it then was not seen"
        ));
    }
    if let Some(after) = &latest.saves_after {
        let unchanged_since = current.iter().all(|now| {
            after.iter().any(|mark| {
                mark.save_id == now.save_id && mark.content_sha256 == now.content_sha256
            })
        });
        if !unchanged_since {
            return PrecedingSession::unknown(
                "the save has changed since the last recorded session ended, outside any session EmuWiz saw",
            );
        }
    }
    match (&latest.state, &latest.end) {
        (SessionState::EndNotSeen, _) => PrecedingSession::ExitNotSeen {
            session_id: id,
            detail: "EmuWiz was not watching when the emulator stopped".to_string(),
        },
        (_, Some(SessionEnd::Clean)) if latest.saves_after.is_some() => {
            PrecedingSession::CleanExit { session_id: id }
        }
        (_, Some(SessionEnd::Clean)) => PrecedingSession::unknown(
            "the last session ended normally, but the saves were not compared afterwards",
        ),
        (_, Some(SessionEnd::LauncherExitedOk)) => PrecedingSession::ExitNotSeen {
            session_id: id,
            detail: "only the launcher's exit was seen, not the emulator's own".to_string(),
        },
        (_, Some(SessionEnd::ExitCode { code })) => PrecedingSession::ErrorExit {
            session_id: id,
            code: *code,
        },
        (
            _,
            Some(SessionEnd::Signal {
                signal,
                core_dumped,
            }),
        ) => {
            if is_crash_signal(*signal, *core_dumped) {
                PrecedingSession::Crashed {
                    session_id: id,
                    signal: *signal,
                }
            } else {
                PrecedingSession::ForcedStop {
                    session_id: id,
                    signal: *signal,
                }
            }
        }
        (_, Some(SessionEnd::StatusUnavailable)) | (_, None) => PrecedingSession::ExitNotSeen {
            session_id: id,
            detail: "the emulator's exit status could not be read".to_string(),
        },
    }
}
