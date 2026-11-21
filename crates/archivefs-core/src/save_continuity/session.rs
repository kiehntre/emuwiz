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

use super::crash_evidence::{CrashEvidence, CrashSource, NoCrashSource, ProcessLifetime};
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
    /// The launcher itself ended with an error or was stopped. That is a
    /// fact about the launcher only: the emulator may have carried on, or
    /// closed normally. Never read as a crash.
    LauncherEndedOtherwise {
        detail: String,
    },
    /// The emulator's own status was read, but it cannot be interpreted:
    /// for this emulator a non-zero status has been seen on a perfectly
    /// normal close, or (RetroArch) its statuses mean something only from
    /// inside a running game and the game was not seen running, in which
    /// case this covers status 0 too. Neither a normal close nor an error.
    /// Never a warning. `detail` keeps the observed status.
    ExitUnsettled {
        detail: String,
    },
    ExitCode {
        code: i32,
    },
    Signal {
        signal: i32,
        core_dumped: bool,
    },
    /// The exit status could not be read.
    StatusUnavailable,
    /// The operating system's crash handler recorded that the emulator's
    /// own process, identified for this launch, was ended by a fatal signal.
    /// Which signal is not recorded.
    FatalTerminationRecorded {
        recorded_at: String,
    },
}

impl SessionEnd {
    pub fn label(&self) -> String {
        match self {
            Self::Clean => "Closed normally".to_string(),
            Self::LauncherExitedOk => {
                "The launcher finished normally (the emulator's own result is not known)"
                    .to_string()
            }
            Self::LauncherEndedOtherwise { detail } => {
                format!("The launcher ended ({detail}); how the emulator itself ended is not known")
            }
            Self::ExitUnsettled { detail } => {
                format!("Ended; whether that was a normal close is not known: {detail}")
            }
            Self::ExitCode { code } => format!("Ended with status {code}"),
            Self::Signal {
                signal,
                core_dumped: true,
            } => format!("Ended by a fault signal ({signal})"),
            Self::Signal { signal, .. } if is_crash_signal(*signal, false) => {
                format!("Ended by a fault signal ({signal})")
            }
            Self::Signal { signal, .. } => format!("Was stopped (signal {signal})"),
            Self::StatusUnavailable => "Ended; how is not known".to_string(),
            Self::FatalTerminationRecorded { recorded_at } => format!(
                "Ended by a fatal signal (recorded by the system's crash handler at {recorded_at})"
            ),
        }
    }

    pub fn abnormal(&self) -> bool {
        matches!(
            self,
            Self::ExitCode { .. } | Self::Signal { .. } | Self::FatalTerminationRecorded { .. }
        )
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

/// What showed that a session's game really ran, and when it was seen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameSeenRunning {
    /// In plain words, what was observed.
    pub what: String,
    pub observed_unix: i64,
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
    /// When EmuWiz first saw this process (seconds since 1970). It existed
    /// then, so nothing recorded earlier can be about it. 0 on older records.
    #[serde(default)]
    pub first_seen_unix: i64,
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
    /// Nanoseconds within `requested_unix`, so sessions requested in the
    /// same second still have an order.
    #[serde(default)]
    pub requested_order: u32,
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
    /// The emulator's own process inside a Flatpak sandbox, when it was
    /// identified as the sandbox's direct command (see [`sandbox_main`]).
    #[serde(default)]
    pub sandbox_main: Option<TrackedProcess>,
    /// How `end` was learned. `None` on older records.
    #[serde(default)]
    pub exit_evidence: Option<ExitEvidence>,
    /// When the emulator inside the sandbox was last seen alive.
    #[serde(default)]
    pub sandbox_main_last_alive_unix: Option<i64>,
    /// How many process ids the system had issued, and its limit, when the
    /// emulator was last seen alive (see `ProcessTable::process_ids_issued`).
    #[serde(default)]
    pub sandbox_main_reuse_mark: Option<(u64, u64)>,
    /// Which boot of the machine that was (`ProcessTable::boot_id`).
    #[serde(default)]
    pub sandbox_main_boot: Option<String>,
    /// Evidence that the game was actually seen running in this session.
    /// A game and a core being asked for at launch is **not** such evidence:
    /// loading can fail and leave the emulator at its menu. Empty unless a
    /// caller that really observed gameplay recorded it
    /// ([`SessionJournal::record_game_seen_running`]). Nothing in EmuWiz
    /// does today.
    #[serde(default)]
    pub game_seen_running: Option<GameSeenRunning>,
    /// What the system's own crash records say about that process, when
    /// they were consulted. Independent of the exit status.
    #[serde(default)]
    pub crash_evidence: Option<CrashEvidence>,
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
    /// The parent of `pid`, while it exists.
    fn parent(&self, _pid: u32) -> Option<u32> {
        None
    }
    /// The short process name of `pid` (`comm`).
    fn name(&self, _pid: u32) -> Option<String> {
        None
    }
    /// How many processes the system has started since boot, and the
    /// largest process id it uses before starting again from the bottom.
    /// An id is unlikely to have been given to a second process until
    /// nearly that many more have been started. This is a heuristic, not a
    /// proof: the count covers successful starts only, and a program with
    /// the right to do so can ask for a particular id. `None` when this
    /// table cannot say.
    fn process_ids_issued(&self) -> Option<(u64, u64)> {
        None
    }
    /// An identifier that changes every time the machine starts.
    fn boot_id(&self) -> Option<String> {
        None
    }
}

/// How a session's `end` was learned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitEvidence {
    /// EmuWiz started the emulator itself and read its exit status.
    EmulatorProcess,
    /// EmuWiz started a Flatpak sandbox, identified the emulator as the
    /// sandbox's direct command, and the sandbox relayed that command's
    /// exit status when it ended.
    SandboxRelay,
    /// Only a launcher's end was seen. Says nothing about the emulator.
    LauncherOnly,
    /// The system's crash handler recorded that the emulator's own process
    /// was ended by a fatal signal. Independent of any exit status.
    CrashRecord,
}

/// Emulators whose exit status is believed: a status of 0 means it closed
/// normally and anything else means it did not.
///
/// The standard for being listed: on a real install, **from inside a running
/// game**, ordinary ways of closing were tried repeatedly and returned 0
/// every time, and a deliberate abnormal end returned something else. A
/// close at a menu with no game running is not enough (RetroArch returned 0
/// that way and 139 for a polite stop on another run).
///
/// **No emulator meets that standard for every kind of session, so this
/// list is empty.** RetroArch meets it for sessions in which the game was
/// seen running, which nothing records today (see
/// [`EXIT_STATUS_DEMONSTRATED_IN_GAME`]). For everything else an
/// emulator's non-zero exit is recorded as "not known whether that was a
/// normal close" and never as an abnormal exit.
///
/// Each entry is the emulator and the names (lowercase) its binary has
/// inside a Flatpak sandbox, where a relayed status is believed only for
/// that binary as the sandbox's direct command.
#[cfg(not(test))]
pub const EXIT_STATUS_DEMONSTRATED: [(&str, &[&str]); 0] = [];

/// Tests exercise the rules with a made-up emulator, so that no real one
/// has to be listed as trusted for a test to run.
#[cfg(test)]
pub const EXIT_STATUS_DEMONSTRATED: [(&str, &[&str]); 1] = [("ProbeEmu", &["probeemu"])];

/// What has been observed so far for emulators that are **not** on the
/// demonstrated list: preliminary evidence, quoted when a session of that
/// emulator ends with a status that cannot be interpreted. From
/// the probes in `scripts/qa/` (see the per-emulator table in the docs).
pub const PRELIMINARY_EXIT_EVIDENCE: [(&str, &str); 4] = [
    (
        "RetroArch",
        "it was not verified that the game actually ran in this session; RetroArch's exit status has been shown to be meaningful only from inside a running game, and at its menu a polite stop once returned 139, which is unexplained",
    ),
    (
        "PPSSPP",
        "from inside a game ordinary closes returned 0, but after a deliberate fault it hung instead of ending, so its exit status does not tell the two apart",
    ),
    (
        "PCSX2",
        "closing its window returned exit code 1 (observed at the first-run setup window, not in a game)",
    ),
    (
        "RPCS3",
        "closing its window returned 134: it aborted while shutting down in the test environment",
    ),
];

/// The names (lowercase) each emulator's binary has inside its Flatpak
/// sandbox. Used only to *identify* the emulator's own process there, so
/// that evidence about exactly that process (a crash record) can be tied to
/// the launch. Being listed here does not make an exit status believed.
#[cfg(not(test))]
pub const SANDBOX_BINARIES: [(&str, &[&str]); 4] = [
    ("PCSX2", &["pcsx2-qt", "pcsx2"]),
    ("RPCS3", &["rpcs3"]),
    ("PPSSPP", &["ppssppsdl", "ppssppqt"]),
    ("RetroArch", &["retroarch"]),
];

#[cfg(test)]
pub const SANDBOX_BINARIES: [(&str, &[&str]); 5] = [
    ("PCSX2", &["pcsx2-qt", "pcsx2"]),
    ("RPCS3", &["rpcs3"]),
    ("PPSSPP", &["ppssppsdl", "ppssppqt"]),
    ("RetroArch", &["retroarch"]),
    ("ProbeEmu", &["probeemu"]),
];

/// Emulators whose exit status is believed **only for a session in which
/// the game was actually seen running** (owner decisions 2026-11-21).
///
/// RetroArch, native 1.21.0 and Flatpak 1.22.2, from inside a running game
/// (`scripts/qa/retroarch_exit_probe.py`): 18 ordinary endings per route
/// returned 0 every time, 3 deliberate faults per route returned 139 every
/// time, nothing hung.
///
/// Not covered, and deliberately not believed:
/// - a session where gameplay was not verified. That includes RetroArch at
///   its menu (one polite stop there returned 139 and is unexplained) and a
///   session that asked for a game which may never have loaded. A game and
///   a core being *requested* proves nothing;
/// - any launch through a wrapper or launcher other than the checked
///   Flatpak sandbox: that status is the wrapper's (decided before this
///   list is consulted).
///
/// **EmuWiz has no reliable way to see that a game ran, and none is
/// invented here.** [`SessionRecord::game_seen_running`] is therefore never
/// set by shipped code, and every exit status of these emulators, 0
/// included, is recorded as "not known" with the observed number kept. The
/// rule takes effect only once a trustworthy observation exists.
pub const EXIT_STATUS_DEMONSTRATED_IN_GAME: [&str; 1] = ["RetroArch"];

fn demonstrated_in_game_only(emulator: &str) -> bool {
    !exit_status_demonstrated(emulator)
        && EXIT_STATUS_DEMONSTRATED_IN_GAME
            .iter()
            .any(|name| name.eq_ignore_ascii_case(emulator))
}

/// Whether this session's exit status is believed: its emulator is on the
/// unconditional list, or it is on the in-game list and the game was seen
/// running in this session.
pub fn exit_status_believed(record: &SessionRecord) -> bool {
    let emulator = &record.launch.emulator;
    exit_status_demonstrated(emulator)
        || (demonstrated_in_game_only(emulator) && record.game_seen_running.is_some())
}

/// Whether `emulator`'s exit status is believed for every kind of session
/// (see [`EXIT_STATUS_DEMONSTRATED`]).
pub fn exit_status_demonstrated(emulator: &str) -> bool {
    EXIT_STATUS_DEMONSTRATED
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case(emulator))
}

/// The emulator's own process among a session's tracked processes, but only
/// when the session is a checked Flatpak route and the process sits exactly
/// where the sandbox's command sits: its executable is the emulator binary,
/// and its parents up to the process EmuWiz started are all `bwrap`. A shell
/// or anything else in between means the status EmuWiz gets back may not be
/// the emulator's, so there is no answer.
pub fn sandbox_main(
    record: &SessionRecord,
    processes: &dyn ProcessTable,
) -> Option<TrackedProcess> {
    if !record.launch.profile.eq_ignore_ascii_case("flatpak") {
        return None;
    }
    let binaries = SANDBOX_BINARIES
        .iter()
        .find(|(emulator, _)| emulator.eq_ignore_ascii_case(&record.launch.emulator))?
        .1;
    let launched = record.process_id?;
    // The process EmuWiz started must itself be the sandbox (`flatpak run`,
    // which becomes `bwrap`). If it is a script or anything else, then what
    // comes back to EmuWiz is that program's exit status, which can be 0
    // whatever happened to the emulator it ran.
    let launcher_is_sandbox = |text: Option<String>| {
        text.as_deref()
            .and_then(|text| Path::new(text.trim_end_matches(" (deleted)")).file_name())
            .and_then(|name| name.to_str())
            .is_some_and(|name| matches!(name, "flatpak" | "bwrap"))
    };
    let by_name = launcher_is_sandbox(processes.name(launched));
    let by_file = launcher_is_sandbox(
        processes
            .executable(launched)
            .map(|path| path.to_string_lossy().into_owned()),
    );
    if !(by_name && by_file) {
        return None;
    }
    record
        .processes
        .iter()
        .filter(|process| process.pid != launched && alive(process, processes))
        .find(|process| {
            let binary = process
                .executable
                .as_deref()
                .and_then(Path::file_name)
                .map(|name| name.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if !binaries.contains(&binary.as_str()) {
                return false;
            }
            // Walk up: every process between it and the launched one is bwrap.
            let mut current = process.pid;
            for _ in 0..8 {
                let Some(parent) = processes.parent(current) else {
                    return false;
                };
                if parent == launched {
                    return true;
                }
                if processes.name(parent).as_deref() != Some("bwrap") {
                    return false;
                }
                current = parent;
            }
            false
        })
        .cloned()
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
    fn boot_id(&self) -> Option<String> {
        let id = fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
        Some(id.trim().to_string()).filter(|id| !id.is_empty())
    }
    fn process_ids_issued(&self) -> Option<(u64, u64)> {
        let stat = fs::read_to_string("/proc/stat").ok()?;
        let issued = stat
            .lines()
            .find_map(|line| line.strip_prefix("processes "))?
            .trim()
            .parse()
            .ok()?;
        let limit = fs::read_to_string("/proc/sys/kernel/pid_max")
            .ok()?
            .trim()
            .parse()
            .ok()?;
        Some((issued, limit))
    }
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

    fn parent(&self, pid: u32) -> Option<u32> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        stat.rsplit_once(')')?
            .1
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()
    }

    fn name(&self, pid: u32) -> Option<String> {
        Some(
            fs::read_to_string(format!("/proc/{pid}/comm"))
                .ok()?
                .trim()
                .to_string(),
        )
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
        first_seen_unix: now_unix(),
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
    // Identify the emulator inside the sandbox while the chain can still be
    // walked. Once found it is kept: the answer cannot be re-derived later.
    if record.sandbox_main.is_none() {
        record.sandbox_main = sandbox_main(record, processes);
    }
    if record
        .sandbox_main
        .as_ref()
        .is_some_and(|main| alive(main, processes))
    {
        record.sandbox_main_last_alive_unix = Some(now_unix());
        record.sandbox_main_reuse_mark = processes.process_ids_issued();
        // A boot id that could not be read this once does not erase the one
        // already known.
        if let Some(boot) = processes.boot_id() {
            record.sandbox_main_boot = Some(boot);
        }
    }
}

/// How stale the stored "last seen alive" time may get before it is written
/// again. It decides whether a crash record may still be tied to the
/// process, so it has to follow a long session; it need not be written on
/// every look.
pub const LAST_ALIVE_WRITE_SECONDS: i64 = 60;

fn last_alive_moved_on(before: Option<i64>, record: &SessionRecord) -> bool {
    match (before, record.sandbox_main_last_alive_unix) {
        (None, Some(_)) => true,
        (Some(old), Some(new)) => new - old >= LAST_ALIVE_WRITE_SECONDS,
        _ => false,
    }
}

/// How long after the emulator was last seen alive a crash record may
/// still be matched to it. Process ids are reused; the longer nobody was
/// looking, the less a matching id means.
pub const CRASH_RECORD_WINDOW_SECONDS: i64 = 600;

/// Asks the crash source about the emulator process identified for this
/// session, if there is one, it is gone, and it was seen alive recently
/// enough for its id to still mean that process. Stores what it learns.
/// Returns the time of a confirmed record.
fn consult_crash_source(
    record: &mut SessionRecord,
    processes: &dyn ProcessTable,
    crash: &dyn CrashSource,
    now: i64,
) -> Option<String> {
    let main = record.sandbox_main.clone()?;
    if alive(&main, processes) || main.first_seen_unix <= 0 {
        return None;
    }
    let last_alive = record
        .sandbox_main_last_alive_unix
        .unwrap_or(main.first_seen_unix);
    // Could the id have gone round and been given to another process since
    // the emulator was last seen? Only where the system can say.
    let reused = match (
        record.sandbox_main_reuse_mark,
        processes.process_ids_issued(),
    ) {
        // Ids are handed out in a circle, skipping those in use, so coming
        // back to one takes nearly a full round. Half a round is already
        // treated as too many.
        // A count that went backwards means the machine restarted: every
        // id is being handed out afresh.
        (Some((then, _)), Some((issued, limit))) => issued < then || issued - then >= limit / 2,
        _ => false,
    } || match (&record.sandbox_main_boot, processes.boot_id()) {
        // A different boot: the id belongs to whatever has it now.
        (Some(then), Some(now)) => *then != now,
        _ => false,
    };
    let evidence = if now - last_alive > CRASH_RECORD_WINDOW_SECONDS || now < last_alive {
        CrashEvidence::unavailable(
            "the emulator was last seen too long before it was found gone for a record to be tied to it",
        )
    } else if reused {
        CrashEvidence::unavailable(
            "the machine restarted, or started very many processes, since the emulator was last seen, so its process id may have been given to another",
        )
    } else {
        crash.lookup(&ProcessLifetime {
            pid: main.pid,
            started_unix: main.first_seen_unix,
            gone_by_unix: now,
        })
    };
    let confirmed = match &evidence {
        CrashEvidence::FatalTerminationRecorded { recorded_at, .. } => Some(recorded_at.clone()),
        _ => None,
    };
    record.crash_evidence = Some(evidence);
    confirmed
}

pub struct SessionJournal {
    dir: PathBuf,
    device_id: String,
    /// Where fatal terminations are looked up. Never knows, by default.
    crash: Box<dyn CrashSource>,
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
            crash: Box::new(NoCrashSource),
        })
    }

    /// Uses `crash` to look up fatal terminations of sandboxed emulators.
    pub fn with_crash_source(mut self, crash: Box<dyn CrashSource>) -> Self {
        self.crash = crash;
        self
    }

    fn path(&self, record: &SessionRecord) -> PathBuf {
        // Second, then an order number within this run of EmuWiz, then id:
        // file names sort in the order the sessions were requested, even
        // when several fall in the same second.
        self.dir.join(format!(
            "{:012}-{:09}-{}.json",
            record.requested_unix.max(0),
            record.requested_order,
            record.session_id
        ))
    }

    fn write(&self, record: &SessionRecord) -> Result<(), ContinuityError> {
        let path = self.path(record);
        let text = serde_json::to_vec_pretty(record)
            .map_err(|error| ContinuityError::Io(error.to_string()))?;
        super::write_state_file(&path, &text)
    }

    /// Step 1: a launch was asked for.
    pub fn request(
        &self,
        game: SessionGame,
        launch: SessionLaunch,
        saves_before: Vec<SaveMark>,
    ) -> Result<SessionRecord, ContinuityError> {
        let clock = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let requested_unix = clock.as_secs() as i64;
        let requested_order = clock.subsec_nanos();
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
            requested_order,
            started_unix: None,
            ended_unix: None,
            process_id: None,
            process_start_ticks: None,
            processes: Vec::new(),
            launcher_end: None,
            app_scope: None,
            sandbox_main: None,
            exit_evidence: None,
            sandbox_main_last_alive_unix: None,
            sandbox_main_reuse_mark: None,
            sandbox_main_boot: None,
            game_seen_running: None,
            crash_evidence: None,
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

    /// Step 2, when what was actually started is only known now: records
    /// which emulator and install the process is. Quick: no file work
    /// besides the record, so the process is noted while it certainly
    /// still exists. `spawn_start_ticks` is the start time read when the
    /// process was created, used if it has already gone by now.
    pub fn mark_started_as(
        &self,
        session_id: &str,
        pid: u32,
        launch: SessionLaunch,
        spawn_start_ticks: Option<u64>,
        processes: &dyn ProcessTable,
    ) -> Result<SessionRecord, ContinuityError> {
        let mut record = self.get(session_id)?;
        if record.state != SessionState::Requested {
            return Err(ContinuityError::Refused(format!(
                "session {session_id} is already past starting"
            )));
        }
        record.launch = launch;
        self.write(&record)?;
        let mut record = self.mark_started(session_id, pid, processes)?;
        // The start time read when the process was created is the truth. If
        // the process table now shows nothing, or a different start time
        // (the id was already given to something else), the launched
        // process has gone and is recorded by its real start time.
        if record.process_start_ticks != spawn_start_ticks {
            if let Some(start_ticks) = spawn_start_ticks {
                record.processes.retain(|process| process.pid != pid);
                record.process_start_ticks = Some(start_ticks);
                record.processes.push(TrackedProcess {
                    pid,
                    start_ticks,
                    executable: None,
                    launched_by_emuwiz: true,
                    first_seen_unix: now_unix(),
                });
                self.write(&record)?;
            }
        }
        Ok(record)
    }

    /// Records that the game was actually **observed running** in this
    /// session, and what showed it. Only for a real observation: never
    /// because a game was requested, a window appeared or time passed.
    /// Without it, an emulator on [`EXIT_STATUS_DEMONSTRATED_IN_GAME`] has
    /// its exit status recorded as "not known".
    pub fn record_game_seen_running(
        &self,
        session_id: &str,
        what: &str,
    ) -> Result<SessionRecord, ContinuityError> {
        let mut record = self.get(session_id)?;
        if record.state != SessionState::Started {
            return Err(ContinuityError::Refused(format!(
                "session {session_id} is not running"
            )));
        }
        if what.trim().is_empty() {
            return Err(ContinuityError::Refused(
                "what showed the game running must be stated".to_string(),
            ));
        }
        record.game_seen_running = Some(GameSeenRunning {
            what: what.trim().to_string(),
            observed_unix: now_unix(),
        });
        self.write(&record)?;
        Ok(record)
    }

    /// Keeps only the "before" marks of the launched emulator's own saves,
    /// so the session speaks for those saves and no others.
    pub fn keep_saves(
        &self,
        session_id: &str,
        its_saves: &[String],
    ) -> Result<SessionRecord, ContinuityError> {
        let mut record = self.get(session_id)?;
        let before = record.saves_before.len();
        record
            .saves_before
            .retain(|mark| its_saves.contains(&mark.save_id));
        if record.saves_before.len() != before {
            self.write(&record)?;
        }
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
            let seen_before = record.sandbox_main_last_alive_unix;
            refresh(&mut record, processes);
            if before != (record.processes.len(), record.app_scope.clone()) {
                self.write(&record)?;
            } else if last_alive_moved_on(seen_before, &record) {
                // Only the "still alive" time moved on. Failing to store it
                // (a full disk) must not turn a look into an error.
                let _ = self.write(&record);
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
        // A session that a concurrent "settle" marked "end not seen" a
        // moment ago (the process had just gone, and this call was still
        // comparing saves) is completed by the real exit status now: what
        // was actually observed replaces "not seen". A recycled process id
        // cannot get here, because such a session has no status to give.
        // "A moment ago" is meant literally: within a few minutes of being
        // settled. An old "end not seen" record is never rewritten.
        let settled_too_early = record.state == SessionState::EndNotSeen
            && record.end.is_none()
            && record
                .ended_unix
                .is_some_and(|settled| (now_unix() - settled).abs() <= 300);
        if (record.state != SessionState::Started && !settled_too_early)
            || record.launcher_end.is_some()
        {
            return Err(ContinuityError::Refused(format!(
                "session {session_id} is not running"
            )));
        }
        if settled_too_early {
            // Only while the launched process really is gone.
            let gone = record
                .processes
                .iter()
                .all(|process| !alive(process, processes));
            if !gone {
                return Err(ContinuityError::Refused(format!(
                    "session {session_id} is not running"
                )));
            }
            record.notes.retain(|note| {
                !note.starts_with("the emulator process is gone and EmuWiz did not see it end")
            });
        }
        refresh(&mut record, processes);
        // Whose exit is this? Three cases, never mixed:
        //  - EmuWiz started the emulator itself: the status is the emulator's.
        //  - EmuWiz started a checked Flatpak sandbox, the emulator was
        //    identified as its direct command, that process is now gone and
        //    the sandbox exited by itself: the status is the emulator's,
        //    relayed (a signal N arrives as exit code 128 + N).
        //  - anything else through a launcher: the status is the launcher's
        //    and says nothing about the emulator, clean or otherwise.
        let relayed = record.launch.through_launcher
            && matches!(exit, ExitObservation::Code(_))
            // Checked again here, not only when the emulator was identified:
            // a stored record never widens what is believed.
            && exit_status_believed(&record)
            && record
                .sandbox_main
                .as_ref()
                .is_some_and(|main| !alive(main, processes));
        let (end, evidence) = match exit {
            _ if record.launch.through_launcher && !relayed => (
                match exit {
                    ExitObservation::Code(0) => SessionEnd::LauncherExitedOk,
                    ExitObservation::Code(code) => SessionEnd::LauncherEndedOtherwise {
                        detail: format!("exit code {code}"),
                    },
                    ExitObservation::Signal { signal, .. } => SessionEnd::LauncherEndedOtherwise {
                        detail: format!("stopped by signal {signal}"),
                    },
                    ExitObservation::Unavailable => SessionEnd::StatusUnavailable,
                },
                ExitEvidence::LauncherOnly,
            ),
            ExitObservation::Code(0) => (
                SessionEnd::Clean,
                if relayed {
                    ExitEvidence::SandboxRelay
                } else {
                    ExitEvidence::EmulatorProcess
                },
            ),
            // An exit *code* is never turned into a signal. 139 or 134 coming
            // back through a sandbox or wrapper may be 128 + a signal, or may
            // be a number some program chose; only the kernel reporting a
            // signal for a process EmuWiz itself waited on is a signal.
            ExitObservation::Code(code) => (
                SessionEnd::ExitCode { code },
                if relayed {
                    ExitEvidence::SandboxRelay
                } else {
                    ExitEvidence::EmulatorProcess
                },
            ),
            ExitObservation::Signal {
                signal,
                core_dumped,
            } => (
                SessionEnd::Signal {
                    signal,
                    core_dumped,
                },
                ExitEvidence::EmulatorProcess,
            ),
            ExitObservation::Unavailable => {
                (SessionEnd::StatusUnavailable, ExitEvidence::EmulatorProcess)
            }
        };
        // Two different kinds of evidence:
        //
        // - A *signal* reported by the kernel for the emulator's own process
        //   (EmuWiz started it and waited on it) is a direct observation: it
        //   was ended by that signal. That is recorded and warned about for
        //   any emulator. It says the process ended abnormally; it does not
        //   say the game crashed or the save is damaged.
        // - A non-zero exit *code* needs interpreting, and is called
        //   abnormal only for an emulator whose codes have been demonstrated
        //   to mean that. For every other emulator it is recorded as a
        //   status EmuWiz cannot interpret. Zero is still zero.
        // For an emulator whose statuses mean something only from inside a
        // running game, that includes 0: without evidence that the game
        // ran, neither a normal in-game close nor an abnormal one is
        // claimed. The number itself is kept either way.
        let code_was_read = evidence != ExitEvidence::LauncherOnly
            && (matches!(end, SessionEnd::ExitCode { .. })
                || (end == SessionEnd::Clean
                    && demonstrated_in_game_only(&record.launch.emulator)));
        let end = if code_was_read && !exit_status_believed(&record) {
            let seen_so_far = PRELIMINARY_EXIT_EVIDENCE
                .iter()
                .find(|(emulator, _)| emulator.eq_ignore_ascii_case(&record.launch.emulator))
                .map_or(
                    "a normal close of this emulator has not been shown to return 0",
                    |(_, evidence)| evidence,
                );
            let observed = match &end {
                SessionEnd::Clean => "Ended with status 0".to_string(),
                other => other.label(),
            };
            SessionEnd::ExitUnsettled {
                detail: format!("{observed} ({seen_so_far})"),
            }
        } else {
            end
        };
        record.exit_evidence = Some(evidence);
        let launched = record.process_id;
        // When the sandbox has relayed its command's exit, the emulator is
        // over, whatever helper processes are still winding down.
        let still_running = !relayed
            && record
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
        // Independent evidence, for a sandboxed emulator only: did the
        // system record that exactly that process was ended by a fatal
        // signal? If so that is what happened, whatever number came back.
        if record.launch.through_launcher {
            if let Some(recorded_at) =
                consult_crash_source(&mut record, processes, self.crash.as_ref(), now_unix())
            {
                record.end = Some(SessionEnd::FatalTerminationRecorded { recorded_at });
                record.exit_evidence = Some(ExitEvidence::CrashRecord);
            }
        }
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
                    let seen_before = record.sandbox_main_last_alive_unix;
                    refresh(&mut record, processes);
                    // A process counts only while both its id and its start
                    // time match: a recycled id is a different process.
                    if record
                        .processes
                        .iter()
                        .any(|process| alive(process, processes))
                    {
                        if record.processes.len() != before
                            || last_alive_moved_on(seen_before, &record)
                        {
                            let _ = self.write(&record);
                        }
                        continue;
                    }
                    record.ended_unix = Some(now);
                    // Whoever saw (or did not see) the end: if the system
                    // recorded that exactly this emulator process was ended
                    // by a fatal signal, that is what happened.
                    if record.launch.through_launcher {
                        if let Some(recorded_at) =
                            consult_crash_source(&mut record, processes, self.crash.as_ref(), now)
                        {
                            record.launcher_end = None;
                            record.state = SessionState::Ended;
                            record.end = Some(SessionEnd::FatalTerminationRecorded { recorded_at });
                            record.exit_evidence = Some(ExitEvidence::CrashRecord);
                            if self.write(&record).is_ok() {
                                settled.push(record);
                            }
                            continue;
                        }
                    }
                    if let Some(end) = record.launcher_end.take() {
                        // The launcher's end was seen; the emulator's own
                        // was not. Say exactly that.
                        record.state = SessionState::Ended;
                        // Whatever the launcher's own end was, it happened
                        // while the emulator was still running, so it is
                        // the launcher's alone: neither clean nor a crash.
                        record.exit_evidence = Some(ExitEvidence::LauncherOnly);
                        record.end = Some(match end {
                            SessionEnd::Clean | SessionEnd::LauncherExitedOk => {
                                SessionEnd::LauncherExitedOk
                            }
                            SessionEnd::LauncherEndedOtherwise { detail } => {
                                SessionEnd::LauncherEndedOtherwise { detail }
                            }
                            other => SessionEnd::LauncherEndedOtherwise {
                                detail: other.label(),
                            },
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
        (_, Some(SessionEnd::ExitUnsettled { detail })) => PrecedingSession::ExitNotSeen {
            session_id: id,
            detail: format!(
                "this emulator's exit status does not tell a normal close from a problem ({detail})"
            ),
        },
        (_, Some(SessionEnd::LauncherExitedOk | SessionEnd::LauncherEndedOtherwise { .. })) => {
            PrecedingSession::ExitNotSeen {
                session_id: id,
                detail: "only the launcher's end was seen, not the emulator's own".to_string(),
            }
        }
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
                PrecedingSession::FatalSignal {
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
        (_, Some(SessionEnd::FatalTerminationRecorded { .. })) => {
            PrecedingSession::FatalTerminationRecorded {
                session_id: id,
                saves_compared_afterwards: latest.saves_after.is_some(),
            }
        }
        (_, Some(SessionEnd::StatusUnavailable)) | (_, None) => PrecedingSession::ExitNotSeen {
            session_id: id,
            detail: "the emulator's exit status could not be read".to_string(),
        },
    }
}
