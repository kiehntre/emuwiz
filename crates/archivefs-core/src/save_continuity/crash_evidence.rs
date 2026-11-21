//! Independent evidence that a process was ended by a fatal signal.
//!
//! For an emulator inside a Flatpak sandbox, EmuWiz gets back only an exit
//! code from `flatpak run`, and an exit code is never read as a signal. But
//! the operating system itself may have written down that a process was
//! ended by a core-dumping signal. On Ubuntu the kernel hands such events to
//! its crash handler (apport), which for a process inside a container or
//! sandbox logs the **host process id**. Not every such process is certain
//! to get a line (one that may not dump core, or too many at once, may not):
//!
//! ```text
//! ERROR: apport (pid 3167710) 2026-11-20 23:39:17,940: host pid 3167430 crashed in a container without apport support
//! ```
//!
//! This module reads that log and nothing else. It never writes, never needs
//! more rights than the user has, changes nothing about how an emulator is
//! launched, and never fails a launch or a snapshot: whenever the source is
//! missing, unreadable, unsupported or unclear, the answer is
//! [`CrashEvidence::Unavailable`], which callers treat as "not known".
//!
//! A record counts only for **one exact process of one launch**:
//!
//! - its host process id is the one EmuWiz tracked for the emulator, and
//! - the record's time lies inside that process's observed lifetime (from
//!   when it started to when EmuWiz saw it gone, with a few seconds' slack).
//!
//! The process id alone is never enough: ids are reused. A record from
//! before the process existed (a stale record, an earlier process with the
//! same id) or from after it was gone is not this process.
//!
//! What a record does and does not say: the process was ended by a signal
//! whose default action dumps core. It does not say which signal, why, or
//! anything about the game or its save. What its absence says: nothing. A
//! process ended by SIGKILL or SIGTERM, or one that handles the signal
//! itself, leaves no record.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::PathBuf;

/// Seconds of slack around a process's lifetime: the handler writes its
/// line a moment after the process is gone, and clocks have one-second
/// resolution here.
pub const LIFETIME_SLACK_SECONDS: i64 = 5;
/// A record for the right process id that falls outside its lifetime but
/// within this many seconds of it is neither accepted nor dismissed: the
/// timing is unclear, and the answer is "unavailable". Only a record
/// further away than this is treated as another process's (a stale record,
/// a reused id).
pub const UNCLEAR_TIMING_SECONDS: i64 = 120;
/// The log is small; anything larger than this is not read at all.
const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "evidence")]
pub enum CrashEvidence {
    /// The system's crash handler recorded that exactly this process was
    /// ended by a fatal signal, at a time inside its lifetime.
    FatalTerminationRecorded {
        /// The time as the handler wrote it (local time, as logged).
        recorded_at: String,
        source: String,
    },
    /// The source was available, covers the whole lifetime, and holds no
    /// record for this process. Not "closed normally".
    NoRecord { source: String },
    /// Nothing can be said. Never an error for the caller.
    Unavailable { why: String },
}

impl CrashEvidence {
    pub fn unavailable(why: impl Into<String>) -> Self {
        Self::Unavailable { why: why.into() }
    }

    pub fn confirmed(&self) -> bool {
        matches!(self, Self::FatalTerminationRecorded { .. })
    }
}

/// One process of one launch, as EmuWiz tracked it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessLifetime {
    /// Host process id.
    pub pid: u32,
    /// When the process started (seconds since 1970).
    pub started_unix: i64,
    /// When EmuWiz saw that it was gone.
    pub gone_by_unix: i64,
}

/// Somewhere fatal terminations are recorded.
pub trait CrashSource {
    fn lookup(&self, process: &ProcessLifetime) -> CrashEvidence;
}

/// A source that never knows. The default wherever none is configured.
pub struct NoCrashSource;

impl CrashSource for NoCrashSource {
    fn lookup(&self, _: &ProcessLifetime) -> CrashEvidence {
        CrashEvidence::unavailable("no crash record source on this system")
    }
}

/// Ubuntu's crash handler log.
pub struct ApportLog {
    /// `/proc/sys/kernel/core_pattern`: which program the kernel hands
    /// fatal terminations to.
    pub core_pattern: PathBuf,
    /// `/proc/sys/kernel/core_pipe_limit`. When it is not 0 the kernel waits
    /// for the handler to finish before the dying process is released, so a
    /// record exists by the time the process is seen gone (measured on the
    /// development machine: the record's time fell between the signal and
    /// the moment the parent saw the process gone). That is a property of
    /// this setting, not a guarantee: when it is 0, or cannot be read, the
    /// handler may still be writing when EmuWiz looks, and the absence of a
    /// record then establishes nothing.
    pub core_pipe_limit: PathBuf,
    /// The log files, oldest first.
    pub logs: Vec<PathBuf>,
    /// Turns the handler's local wall-clock time into seconds since 1970.
    /// `None` when that cannot be done unambiguously.
    pub to_unix: fn(&LocalTime) -> Option<i64>,
}

/// A time as the handler writes it: local, no zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl ApportLog {
    /// The real files on this machine.
    pub fn system() -> Self {
        Self {
            core_pattern: PathBuf::from("/proc/sys/kernel/core_pattern"),
            core_pipe_limit: PathBuf::from("/proc/sys/kernel/core_pipe_limit"),
            logs: vec![
                PathBuf::from("/var/log/apport.log.1"),
                PathBuf::from("/var/log/apport.log"),
            ],
            to_unix: local_to_unix,
        }
    }
}

/// The machine's own idea of local time, through the C library.
fn local_to_unix(time: &LocalTime) -> Option<i64> {
    // The handler runs as root from the kernel and writes the machine's
    // local time. If this process was given a time zone of its own, its
    // idea of local time may differ: the times cannot then be compared.
    // (Set at all: an empty value means UTC to the C library.)
    if std::env::var_os("TZ").is_some() {
        return None;
    }
    one_reading(local_as(time, 0), local_as(time, 1))
}

/// The wall-clock time read as standard time (`dst` 0) or as daylight-saving
/// time (`dst` 1); `None` when that reading is not a real time.
fn local_as(time: &LocalTime, dst: i32) -> Option<i64> {
    // SAFETY: `tm` is fully initialised before use; mktime only reads and
    // normalises it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = time.year - 1900;
    tm.tm_mon = time.month as i32 - 1;
    tm.tm_mday = time.day as i32;
    tm.tm_hour = time.hour as i32;
    tm.tm_min = time.minute as i32;
    tm.tm_sec = time.second as i32;
    tm.tm_isdst = dst;
    let unix = unsafe { libc::mktime(&mut tm) };
    // mktime normalises instead of refusing: an impossible date, or a
    // reading (standard/daylight) that does not apply at that time, comes
    // back as a different wall-clock time or with the other flag.
    let unchanged = tm.tm_isdst == dst
        && tm.tm_mday == time.day as i32
        && tm.tm_mon == time.month as i32 - 1
        && tm.tm_hour == time.hour as i32
        && tm.tm_min == time.minute as i32;
    (unix != -1 && unchanged).then_some(unix as i64)
}

/// A wall-clock time is usable only when exactly one reading of it is real.
/// In the hour that repeats when the clocks go back both are, an hour apart:
/// the log does not say which was meant, so the time cannot be placed. In
/// the hour skipped when they go forward neither is.
pub(crate) fn one_reading(standard: Option<i64>, daylight: Option<i64>) -> Option<i64> {
    match (standard, daylight) {
        (Some(one), None) | (None, Some(one)) => Some(one),
        (Some(a), Some(b)) if a == b => Some(a),
        _ => None,
    }
}

/// For tests: the real conversion.
#[cfg(test)]
pub(crate) fn local_to_unix_for_tests(time: &LocalTime) -> Option<i64> {
    local_to_unix(time)
}

/// `YYYY-MM-DD HH:MM:SS` at the start of `text`.
fn parse_time(text: &str) -> Option<LocalTime> {
    let bytes = text.as_bytes();
    if bytes.len() < 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b' '
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let number = |from: usize, to: usize| -> Option<u32> {
        let part = text.get(from..to)?;
        part.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let time = LocalTime {
        year: number(0, 4)? as i32,
        month: number(5, 7)?,
        day: number(8, 10)?,
        hour: number(11, 13)?,
        minute: number(14, 16)?,
        second: number(17, 19)?,
    };
    let plausible = (1..=12).contains(&time.month)
        && (1..=31).contains(&time.day)
        && time.hour < 24
        && time.minute < 60
        && time.second < 61;
    plausible.then_some(time)
}

/// One line of the log, if it is a handler line with a time.
struct Line<'a> {
    time_text: &'a str,
    time: LocalTime,
    /// `Some` for "host pid N crashed ...".
    crashed_host_pid: Option<u32>,
}

fn parse_line(line: &str) -> Option<Line<'_>> {
    // ERROR: apport (pid 123) 2026-11-20 23:39:17,940: <message>
    let rest = line.strip_prefix("ERROR: apport (pid ")?;
    let (_, rest) = rest.split_once(") ")?;
    let time = parse_time(rest)?;
    let time_text = rest.get(..19)?;
    let message = rest.get(19..)?.split_once(": ")?.1;
    let crashed_host_pid = message
        .strip_prefix("host pid ")
        .and_then(|after| after.split_once(" crashed"))
        .and_then(|(pid, _)| pid.parse().ok());
    Some(Line {
        time_text,
        time,
        crashed_host_pid,
    })
}

impl CrashSource for ApportLog {
    fn lookup(&self, process: &ProcessLifetime) -> CrashEvidence {
        const SOURCE: &str = "the system's crash handler log";
        if process.gone_by_unix < process.started_unix {
            return CrashEvidence::unavailable("the process's lifetime is not known");
        }
        // Is this handler the one the kernel uses? If not, its log (even if
        // one is lying around) says nothing about what happens now.
        match fs::read_to_string(&self.core_pattern) {
            Ok(pattern) if pattern.contains("apport") => {}
            Ok(_) => {
                return CrashEvidence::unavailable(
                    "this system does not use the crash handler EmuWiz can read (other systems are not supported yet)",
                );
            }
            Err(_) => {
                return CrashEvidence::unavailable("the system's crash handling could not be read");
            }
        }
        let (not_before, not_after) = (
            process.started_unix - LIFETIME_SLACK_SECONDS,
            process.gone_by_unix + LIFETIME_SLACK_SECONDS,
        );
        // Does the kernel wait for the handler? Only then is "no record by
        // now" a statement about the process.
        let handler_is_waited_for = fs::read_to_string(&self.core_pipe_limit)
            .ok()
            .and_then(|text| text.trim().parse::<i64>().ok())
            .is_some_and(|limit| limit != 0);
        let mut earliest: Option<i64> = None;
        let mut matches: Vec<String> = Vec::new();
        // The right process id, close to its lifetime but not inside it.
        let mut unclear_timing = false;
        let mut read_any = false;
        for path in &self.logs {
            let mut file = match fs::File::open(path) {
                Ok(file) => file,
                // A rotated file that does not exist yet is not a problem.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    return CrashEvidence::unavailable(
                        "the system's crash handler log cannot be read by this user",
                    );
                }
            };
            // Never more than the limit, however the file grows meanwhile.
            let mut bytes = Vec::new();
            let read = (&mut file).take(MAX_LOG_BYTES + 1).read_to_end(&mut bytes);
            if read.is_err() || bytes.len() as u64 > MAX_LOG_BYTES {
                return CrashEvidence::unavailable(
                    "the system's crash handler log could not be read",
                );
            }
            read_any = true;
            // The handler also logs each crashed program's command line,
            // which that program chose and which may contain line breaks.
            // Until such a quotation is closed, a line is not known to be
            // the handler's own words.
            let mut inside_quoted_command = false;
            for line in String::from_utf8_lossy(&bytes).lines() {
                let was_inside = inside_quoted_command;
                if line.contains("(command line \"") {
                    inside_quoted_command = !line.ends_with("\")");
                } else if was_inside && line.ends_with("\")") {
                    inside_quoted_command = false;
                }
                if was_inside && line.contains(&format!("host pid {} ", process.pid)) {
                    return CrashEvidence::unavailable(
                        "the crash handler log names this process inside another program's quoted command line",
                    );
                }
                let Some(parsed) = parse_line(line) else {
                    // A line that names this process but cannot be understood
                    // must not be silently skipped: the answer is then unclear.
                    if line.contains(&format!("host pid {} ", process.pid)) {
                        return CrashEvidence::unavailable(
                            "the crash handler log has an entry for this process that EmuWiz cannot read",
                        );
                    }
                    continue;
                };
                let Some(at) = (self.to_unix)(&parsed.time) else {
                    if parsed.crashed_host_pid == Some(process.pid) {
                        return CrashEvidence::unavailable(
                            "the crash handler log has an entry for this process with a time EmuWiz cannot place",
                        );
                    }
                    continue;
                };
                earliest = Some(earliest.map_or(at, |seen| seen.min(at)));
                if parsed.crashed_host_pid == Some(process.pid) {
                    if (not_before..=not_after).contains(&at) {
                        matches.push(parsed.time_text.to_string());
                    } else if (not_before - UNCLEAR_TIMING_SECONDS
                        ..=not_after + UNCLEAR_TIMING_SECONDS)
                        .contains(&at)
                    {
                        unclear_timing = true;
                    }
                    // Further away than that: another process that had this
                    // id (before this one existed, or long after it ended).
                }
            }
        }
        if !read_any {
            return CrashEvidence::unavailable("the system's crash handler has no log to read");
        }
        // A record just outside the lifetime may be this process with a late
        // or early clock reading, or a neighbour in time that reused the id.
        // It is not accepted and it is not explained away.
        if unclear_timing {
            return CrashEvidence::unavailable(
                "the crash handler log has an entry for this process id close to, but not inside, the time the process was running",
            );
        }
        match matches.as_slice() {
            [one] => CrashEvidence::FatalTerminationRecorded {
                recorded_at: one.clone(),
                source: SOURCE.to_string(),
            },
            // One process ends once. Two records in its lifetime cannot both
            // be it, so neither is believed.
            [_, _, ..] => CrashEvidence::unavailable(
                "the crash handler log has more than one entry that could be this process",
            ),
            // No record. That is only a statement if the kernel waits for
            // the handler (otherwise it may not have written yet)...
            [] if !handler_is_waited_for => CrashEvidence::unavailable(
                "this system does not wait for its crash handler, so a record may not have been written yet",
            ),
            [] => match earliest {
                // ...and if the log reaches back to before the process
                // started; otherwise an entry may have been rotated away.
                Some(first) if first <= not_before => CrashEvidence::NoRecord {
                    source: SOURCE.to_string(),
                },
                None => CrashEvidence::unavailable(
                    "no time in the crash handler log could be placed (it is empty, or this program runs with a time zone of its own)",
                ),
                Some(_) => CrashEvidence::unavailable(
                    "the crash handler log does not reach back to when this process started",
                ),
            },
        }
    }
}
