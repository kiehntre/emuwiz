//! A small, private, bounded record of launches that went wrong.
//!
//! Game Doctor can explain a launch only if the explanation still exists
//! when it is opened, which may be after Blue was closed or crashed. This
//! module keeps a structured summary as one file in a private folder of
//! EmuWiz's own data directory, for two kinds of launch and no other:
//!
//! - a launch that failed;
//! - a launch that did not fail but whose emulator stated that a save
//!   could not be written. That is not a failed launch, and the record
//!   says so: what happened to the launch and how serious a finding is are
//!   separate fields.
//!
//! An ordinary launch with nothing to report is never recorded. Repeats of
//! the same save warning for the same game are one record with a count.
//!
//! # What is kept, and what is not
//!
//! A record is built field by field from an allowlist. It holds:
//!
//! - when the launch failed;
//! - the game's library id and platform. Not its title: a title is free
//!   text, and anything can be typed into one. Whatever shows a record
//!   looks the title up in the library from the id;
//! - the emulator (one of a fixed list), its core (a libretro core name)
//!   and a version number of two or three groups of digits;
//! - how the process ended, as numbers (exit status or signal, duration);
//! - the verdict and, for each finding, the rule that produced it with its
//!   category, confidence and effect;
//! - which kinds of error the emulator reported and how often, as members of
//!   a fixed list, never as the sentences it printed;
//! - driver settings whose value is on a fixed list of driver names, the
//!   result of graphics checks, and missing libraries whose name is on a
//!   fixed list;
//! - what is known about the game's saves ([`super::saves`]): a failed
//!   write, no failure under the monitoring that was available, or no
//!   monitoring, with the kinds of save that were watched;
//! - how much output was examined (sizes and counts only).
//!
//! It never holds emulator output, in whole or in part; file or folder
//! paths; user names; network or web addresses; environment values; or any
//! other free text taken from a log, a settings file or a diagnosis. Such
//! text is not filtered on its way in, because no filter can be shown to
//! catch every secret. It is not copied at all: a value that is not on a
//! list is recorded as "not kept". The wording shown for a stored record is
//! regenerated from the built-in rule table.
//!
//! The detailed output of a failed launch exists only in memory, for the
//! diagnosis made right after the launch. It leaves the program only
//! through [`super::export`], which a person has to ask for.
//!
//! # Limits
//!
//! At most [`HistoryLimits::max_records`] records, none older than
//! [`HistoryLimits::max_age`], none larger than
//! [`HistoryLimits::max_record_bytes`], and never more than
//! [`HistoryLimits::quota_bytes`] in total. What is past a limit is removed,
//! oldest first, each time a finished launch is offered to the store
//! (whether that launch is recorded, is not one to record, or recording is
//! off), and by [`HistoryStore::list`]. The
//! read-only views never remove anything and leave out what is past a limit:
//! [`HistoryStore::view`] every limit, [`HistoryStore::load`] the age limit.
//! So a record past its age can stay on disk until the next launch, and is
//! not shown in the meantime. What is kept never depends on the order files
//! happen to be listed in.
//!
//! # Safety of the files themselves
//!
//! - The folder is created with mode 0700 and refused unless it is a real
//!   directory owned by this user with no access for anyone else.
//! - Every file is opened, created, renamed and removed relative to that one
//!   open directory, by a name this module generated
//!   (`YYYYMMDDTHHMMSSZ-xxxxxxxx.json`). Nothing an emulator printed is ever
//!   part of a path.
//! - A record is written to a temporary name, flushed to disk, and only then
//!   given its final name, without replacing anything. A write that was
//!   interrupted leaves a temporary file, which is removed later.
//! - A file that is not a regular file, is too large, does not parse, holds
//!   a field this module does not write, holds a value that is not on its
//!   list, or does not match its own name is never shown. It is counted.
//!   When limits are enforced it is removed if it is a regular file with a
//!   record's name; a link, a folder or anything else is left where it is.
//! - Saving is best-effort. Every failure to save is returned as a value;
//!   nothing here panics, blocks on another program or touches a launch.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::evidence::{
    ConfigLayer, EvidenceArea, ExitOutcome, Fact, GraphicsApi, LogMeaning, ProbeScope, ProcessExit,
    Trust,
};
use super::rules::{
    self, Confidence, Diagnosis, DiagnosisCategory, DiagnosisReport, Effect, LaunchOutcome, Verdict,
};
use super::saves::{self, SaveCapture, SaveMechanism, SaveWriteAssessment, SaveWriteStatus};
use crate::patch_manager::anchored_fs::{DirectoryHandle, DirectoryIdentity, EntryKind};

pub const HISTORY_SCHEMA_VERSION: u32 = 1;
/// Name of the store's folder inside EmuWiz's data directory.
pub const HISTORY_DIRECTORY: &str = "launch-diagnostics";
/// Present when the person switched history off. Its presence is the whole
/// preference, so switching either way is one atomic create or remove and
/// an interrupted switch can never turn recording back on.
const DISABLED_MARKER: &str = "history-disabled";
const TEMPORARY_PREFIX: &str = ".writing-";
/// A temporary file older than this was left by an interrupted write.
const ABANDONED_AFTER: Duration = Duration::from_secs(10 * 60);
/// Most entries of the folder ever looked at in one pass.
const MAX_DIRECTORY_ENTRIES: usize = 512;

const MAX_FINDINGS: usize = 32;
const MAX_SETTINGS: usize = 24;
const MAX_GRAPHICS_CHECKS: usize = 8;
const MAX_LIBRARIES: usize = 8;

/// Settings whose value may be kept, each with every value that may be
/// kept. A setting that is not here is not recorded at all; a value that is
/// not here is recorded as [`SavedValue::NotKept`].
const KEPT_SETTINGS: &[(&str, &[&str])] = &[
    (
        "video_driver",
        &[
            "vulkan", "gl", "glcore", "gl1", "sdl", "sdl2", "xvideo", "xshm", "caca", "vg", "null",
        ],
    ),
    (
        "audio_driver",
        &[
            "alsa",
            "alsathread",
            "tinyalsa",
            "pulse",
            "pipewire",
            "jack",
            "sdl",
            "sdl2",
            "oss",
            "openal",
            "rsound",
            "roar",
            "null",
        ],
    ),
    (
        "input_driver",
        &["udev", "sdl", "sdl2", "x", "wayland", "linuxraw", "null"],
    ),
    (
        "input_joypad_driver",
        &["udev", "sdl", "sdl2", "linuxraw", "parport", "hid", "null"],
    ),
];

/// Libraries whose name may be kept when the program loader reports one
/// missing, without the `.so` suffix and version. Any other missing library
/// is counted, not named.
const KEPT_LIBRARIES: &[&str] = &[
    "libEGL",
    "libGL",
    "libGLESv2",
    "libGLX",
    "libOpenGL",
    "libQt5Core",
    "libQt5Gui",
    "libQt5Widgets",
    "libQt6Core",
    "libQt6Gui",
    "libQt6Widgets",
    "libSDL2",
    "libSDL2-2.0",
    "libX11",
    "libasound",
    "libavcodec",
    "libavformat",
    "libavutil",
    "libc",
    "libcrypto",
    "libdbus-1",
    "libfontconfig",
    "libfreetype",
    "libgcc_s",
    "libjack",
    "libm",
    "libopenal",
    "libpipewire-0.3",
    "libpulse",
    "libssl",
    "libstdc++",
    "libswresample",
    "libswscale",
    "libudev",
    "libusb-1.0",
    "libvulkan",
    "libwayland-client",
    "libwayland-egl",
    "libxkbcommon",
    "libz",
];

/// How much history is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryLimits {
    pub max_records: usize,
    pub max_age: Duration,
    /// Total size of all records.
    pub quota_bytes: u64,
    /// Size of one record. A larger record is not written.
    pub max_record_bytes: u64,
}

impl Default for HistoryLimits {
    fn default() -> Self {
        Self {
            max_records: 20,
            max_age: Duration::from_secs(14 * 24 * 60 * 60),
            quota_bytes: 512 * 1024,
            max_record_bytes: 16 * 1024,
        }
    }
}

/// What EmuWiz itself knows about the launch that failed. None of it comes
/// from emulator output, and each value is still checked against its shape
/// before it is kept.
#[derive(Clone, Copy, Debug, Default)]
pub struct LaunchIdentity<'a> {
    /// The game's id in the library, when it has one.
    pub library_id: Option<i64>,
    /// The game's title as the library shows it. For what is shown at the
    /// time of the launch only: it is never stored.
    pub title: Option<&'a str>,
    /// EmuWiz's short name for the platform, for example `megadrive`.
    pub platform: Option<&'a str>,
    /// EmuWiz's short name for the emulator, for example `retroarch`. Kept
    /// only when it is one of the emulators EmuWiz starts.
    pub emulator: Option<&'a str>,
    /// The core's file name without its folder or extension, for example
    /// `gambatte_libretro`.
    pub core: Option<&'a str>,
    /// The emulator's version number, for example `1.21.0`.
    pub emulator_version: Option<&'a str>,
}

/// The game a stored failure belongs to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedGame {
    pub library_id: Option<i64>,
    pub platform: Option<String>,
}

/// The emulator a stored failure used.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedEmulator {
    pub name: Option<String>,
    pub core: Option<String>,
    pub version: Option<String>,
}

/// How the process ended: numbers and fixed names only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedExit {
    pub outcome: ExitOutcome,
    pub duration_ms: Option<u64>,
    /// The status came from a launcher (a wrapper or `flatpak run`), not
    /// from the emulator process itself.
    pub through_launcher: bool,
    /// The status was supplied by whoever asked for the diagnosis. EmuWiz
    /// did not observe it.
    pub supplied_by_caller: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// Why the launch failed, as far as the evidence went.
    Cause,
    /// What happened.
    Symptom,
    /// An error the emulator stated. A symptom, not a cause.
    Reported,
}

/// A kind of message the emulator printed, and how often. The message
/// itself is not kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedMessage {
    pub meaning: LogMeaning,
    pub times: u32,
}

/// One finding of a stored diagnosis. Its wording is not kept: it is
/// regenerated from the rule table when the record is shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedFinding {
    pub rule: String,
    pub rule_version: u32,
    pub kind: FindingKind,
    pub category: DiagnosisCategory,
    pub confidence: Confidence,
    pub effect: Effect,
    /// Some evidence spoke against it.
    pub contradicted: bool,
    /// For [`FindingKind::Reported`]: what kind of message it restates.
    pub message: Option<SavedMessage>,
}

/// The value of a setting, as far as it is kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum SavedValue {
    /// The file does not set it.
    NotSet,
    /// Set to this value, which is on the fixed list for its setting.
    Known(String),
    /// Set to something that is not on the list. What, is not kept.
    NotKept,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedSetting {
    pub key: String,
    pub layer: ConfigLayer,
    pub value: SavedValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedGraphicsCheck {
    pub api: GraphicsApi,
    pub scope: ProbeScope,
    pub usable: bool,
}

/// How much emulator output the diagnosis had. Sizes and counts only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedOutputFacts {
    /// The emulator was logging in detail.
    pub detailed_logging: bool,
    pub bytes_examined: u64,
    pub lines_examined: u32,
    /// Some output was left out or not examined.
    pub cut_short: bool,
    /// How long the emulator itself said the game ran, in whole seconds.
    pub reported_run_seconds: Option<u64>,
}

/// What is known about the saves of a stored launch. Fixed values only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedSaves {
    pub status: SaveWriteStatus,
    /// The kinds of save a failed write would have been seen for.
    pub monitored: Vec<SaveMechanism>,
    pub capture: SaveCapture,
}

impl SavedSaves {
    fn of(assessment: &SaveWriteAssessment) -> Self {
        Self {
            status: assessment.status,
            monitored: assessment.monitored.clone(),
            capture: assessment.capture,
        }
    }

    /// The assessment this was stored from, as far as it was kept.
    pub fn assessment(&self) -> SaveWriteAssessment {
        SaveWriteAssessment {
            status: self.status,
            monitored: self.monitored.clone(),
            not_monitored: SaveMechanism::ALL
                .into_iter()
                .filter(|mechanism| !self.monitored.contains(mechanism))
                .collect(),
            capture: self.capture,
            saves_reported_written: 0,
        }
    }
}

/// One stored launch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureRecord {
    pub schema_version: u32,
    pub id: String,
    pub recorded_at_unix: u64,
    /// How many times this was recorded. More than one only for a
    /// repeated save warning. Launches in between where it did not happen
    /// are not recorded, so this is a count, not a run.
    pub occurrences: u32,
    /// When the first of those was recorded.
    pub first_recorded_at_unix: u64,
    /// What happened to the launch, apart from any finding.
    pub launch: LaunchOutcome,
    pub saves: SavedSaves,
    pub game: SavedGame,
    pub emulator: SavedEmulator,
    /// Absent when the evidence held no process exit, or more than one.
    pub exit: Option<SavedExit>,
    pub verdict: Verdict,
    pub engine_version: u32,
    /// Lower-case hex digest of the rule set that produced the diagnosis.
    pub rule_set_digest: String,
    pub findings: Vec<SavedFinding>,
    /// Findings beyond the limit, or from a rule that is not built in.
    pub findings_not_kept: u32,
    pub settings: Vec<SavedSetting>,
    pub graphics: Vec<SavedGraphicsCheck>,
    pub missing_libraries: Vec<String>,
    /// Missing libraries whose name is not on the fixed list.
    pub missing_libraries_not_named: u32,
    pub output: SavedOutputFacts,
    /// Areas the diagnosis had no current evidence for.
    pub not_examined: Vec<EvidenceArea>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryListing {
    /// Newest first.
    pub records: Vec<FailureRecord>,
    /// Files in the folder that could not be read as records.
    pub unreadable: usize,
}

/// Why nothing was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotSaved {
    /// The launch did not fail and no save-write failure was found.
    /// Ordinary launches are not recorded.
    NothingToRecord,
    /// The person switched history off.
    Disabled,
    /// The record is larger than one record may be.
    TooLarge { bytes: u64 },
    /// The folder is not private, or could not be used.
    StorageUnavailable { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveOutcome {
    Saved { id: String, removed: usize },
    NotSaved(NotSaved),
}

#[derive(Debug)]
pub enum HistoryError {
    /// The folder is missing, not private, or not usable.
    Storage(String),
    /// No record has this id.
    NotFound,
    /// The id is not one this module generates.
    InvalidId,
    /// The file exists and is not a readable record.
    Unreadable(String),
}

impl std::fmt::Display for HistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(reason) => write!(f, "launch history storage: {reason}"),
            Self::NotFound => f.write_str("no such record"),
            Self::InvalidId => f.write_str("not a record id"),
            Self::Unreadable(reason) => write!(f, "record could not be read: {reason}"),
        }
    }
}

impl std::error::Error for HistoryError {}

// --- what may be kept ---------------------------------------------------

/// A short identifier EmuWiz uses itself: ASCII letters and digits, then
/// those and the characters in `extra`.
fn is_short_name(value: &str, limit: usize, extra: &[u8]) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= limit
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || extra.contains(byte))
}

/// EmuWiz's short name for a platform: lower-case letters, digits, `_` and
/// `-`, at most 24 characters. A name made of digits only is kept only when
/// it is short (`2600`), so that a long number cannot pass for one.
fn is_platform_name(value: &str) -> bool {
    is_short_name(value, 24, b"_-")
        && !value.bytes().any(|byte| byte.is_ascii_uppercase())
        && (value.len() <= 4 || value.bytes().any(|byte| byte.is_ascii_lowercase()))
}

/// The emulators EmuWiz starts. A name that is not on this list is not
/// kept: the record then says "an emulator".
const KEPT_EMULATORS: &[&str] = &[
    "atari800",
    "azahar",
    "bsnes",
    "cemu",
    "desmume",
    "dolphin",
    "dolphin-emu",
    "dosbox",
    "dosbox-staging",
    "dosbox-x",
    "duckstation",
    "flycast",
    "fs-uae",
    "hatari",
    "mame",
    "melonds",
    "mgba",
    "openrct2",
    "pcsx2",
    "ppsspp",
    "retroarch",
    "rmg",
    "rpcs3",
    "scummvm",
    "shadps4",
    "snes9x",
    "stella",
    "vice",
    "vita3k",
    "xemu",
    "xenia",
];

fn is_emulator_name(value: &str) -> bool {
    KEPT_EMULATORS.contains(&value)
}

/// A libretro core's file name without folder or extension: lower-case
/// letters, digits and `_ - . +`, ending in `_libretro`.
fn is_core_name(value: &str) -> bool {
    is_short_name(value, 48, b"_-.+")
        && !value.bytes().any(|byte| byte.is_ascii_uppercase())
        && value.len() > "_libretro".len()
        && value.ends_with("_libretro")
}

/// Two or three groups of one to four digits, separated by dots. Four
/// groups are refused: that is also what a network address looks like.
fn is_version_number(value: &str) -> bool {
    let groups: Vec<&str> = value.split('.').collect();
    (2..=3).contains(&groups.len())
        && groups.iter().all(|group| {
            (1..=4).contains(&group.len()) && group.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn is_rule_id(value: &str) -> bool {
    is_short_name(value, 64, b"_.") && !value.bytes().any(|byte| byte.is_ascii_uppercase())
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn kept_setting_value(key: &str, value: &str) -> Option<SavedValue> {
    let (_, values) = KEPT_SETTINGS.iter().find(|(kept, _)| *kept == key)?;
    Some(if values.contains(&value) {
        SavedValue::Known(value.to_string())
    } else {
        SavedValue::NotKept
    })
}

/// `libname.so`, optionally followed by up to three version numbers, where
/// `libname` is on the fixed list.
fn is_kept_library(name: &str) -> bool {
    let Some((stem, version)) = name.split_once(".so") else {
        return false;
    };
    let version_ok = version.is_empty()
        || (version.starts_with('.')
            && version[1..].split('.').count() <= 3
            && version[1..].split('.').all(|group| {
                (1..=4).contains(&group.len()) && group.bytes().all(|byte| byte.is_ascii_digit())
            }));
    KEPT_LIBRARIES.contains(&stem) && version_ok
}

fn saved_finding(diagnosis: &Diagnosis, report: &DiagnosisReport) -> Option<SavedFinding> {
    // A rule that is not built in has no wording to regenerate and is not
    // one this program vouches for.
    let rule = rules::rule_info(&diagnosis.rule)?;
    let kind = if diagnosis.reported {
        FindingKind::Reported
    } else if diagnosis.explains_cause {
        FindingKind::Cause
    } else {
        FindingKind::Symptom
    };
    let message = if kind == FindingKind::Reported {
        let mut meanings =
            diagnosis
                .supporting
                .iter()
                .filter_map(|id| match &report.evidence.get(id)?.fact {
                    Fact::LogEvent(event) => Some(event.meaning),
                    _ => None,
                });
        let meaning = meanings.next()?;
        Some(SavedMessage {
            meaning,
            times: u32::try_from(1 + meanings.filter(|other| *other == meaning).count())
                .unwrap_or(u32::MAX),
        })
    } else {
        None
    };
    Some(SavedFinding {
        rule: rule.id.to_string(),
        rule_version: diagnosis.rule_version,
        kind,
        category: diagnosis.category,
        confidence: diagnosis.confidence,
        effect: diagnosis.effect,
        contradicted: !diagnosis.contradicting.is_empty(),
        message,
    })
}

/// Whether a diagnosis describes a launch that failed: the process failed,
/// or the emulator stated an error that stops a game from starting. A
/// problem that only degrades a game that ran is not a failed launch.
pub fn is_failed_launch(report: &DiagnosisReport) -> bool {
    let process_failed = report
        .evidence
        .observed(EvidenceArea::ProcessExit)
        .any(|item| {
            item.trust != Trust::Asserted
                && matches!(&item.fact, Fact::ProcessExit(exit) if exit.is_failure())
        });
    process_failed
        || report
            .symptoms
            .iter()
            .any(|symptom| symptom.effect != Effect::Degrades)
}

/// Whether a launch is one to keep: it failed, or its emulator stated that
/// a save could not be written.
pub fn is_worth_recording(report: &DiagnosisReport, saves: &SaveWriteAssessment) -> bool {
    is_failed_launch(report) || saves.is_write_failure()
}

impl FailureRecord {
    /// Build the record for a diagnosis. Reads nothing and writes nothing.
    /// Every value is taken from a fixed list, is a number, or is a short
    /// name EmuWiz supplied that fits its shape; anything else is left out.
    pub fn summarise(
        id: String,
        recorded_at_unix: u64,
        launch: &LaunchIdentity<'_>,
        report: &DiagnosisReport,
        saves: &SaveWriteAssessment,
    ) -> Self {
        let observed = || {
            report
                .evidence
                .items()
                .iter()
                .filter(|item| item.is_observed() && item.trust != Trust::Asserted)
        };

        let mut exits = observed().filter_map(|item| match &item.fact {
            Fact::ProcessExit(exit) => Some((exit, item.trust)),
            _ => None,
        });
        let exit = match (exits.next(), exits.next()) {
            (Some((exit, trust)), None) => Some(SavedExit {
                outcome: exit.outcome.clone(),
                duration_ms: exit.duration_ms,
                through_launcher: exit.launcher.is_some(),
                supplied_by_caller: trust != Trust::Measured,
            }),
            // None, or several runs mixed together: nothing to state.
            _ => None,
        };

        let mut findings = Vec::new();
        let mut findings_not_kept = 0u32;
        for diagnosis in report.causes.iter().chain(&report.symptoms) {
            match saved_finding(diagnosis, report) {
                Some(finding) if findings.len() < MAX_FINDINGS => findings.push(finding),
                _ => findings_not_kept = findings_not_kept.saturating_add(1),
            }
        }

        let mut settings = Vec::new();
        let mut graphics = Vec::new();
        let mut libraries: BTreeMap<String, ()> = BTreeMap::new();
        let mut missing_libraries_not_named = 0u32;
        let mut output = SavedOutputFacts::default();
        for item in observed() {
            match &item.fact {
                Fact::ConfigValue {
                    layer, key, value, ..
                } => {
                    let saved = match value {
                        None => KEPT_SETTINGS
                            .iter()
                            .any(|(kept, _)| kept == key)
                            .then_some(SavedValue::NotSet),
                        Some(value) => kept_setting_value(key, value),
                    };
                    if let Some(value) = saved
                        && settings.len() < MAX_SETTINGS
                    {
                        settings.push(SavedSetting {
                            key: key.clone(),
                            layer: *layer,
                            value,
                        });
                    }
                }
                Fact::GraphicsApi {
                    api, scope, usable, ..
                } if graphics.len() < MAX_GRAPHICS_CHECKS => graphics.push(SavedGraphicsCheck {
                    api: *api,
                    scope: *scope,
                    usable: *usable,
                }),
                Fact::LogEvent(event) => match event.meaning {
                    LogMeaning::SharedLibraryMissing => match event.value.as_deref() {
                        Some(name) if is_kept_library(name) && libraries.len() < MAX_LIBRARIES => {
                            libraries.insert(name.to_string(), ());
                        }
                        _ => {
                            missing_libraries_not_named =
                                missing_libraries_not_named.saturating_add(1);
                        }
                    },
                    LogMeaning::ContentRanFor => {
                        if let Some(seconds) = event
                            .value
                            .as_deref()
                            .and_then(super::retroarch::ran_for_seconds)
                        {
                            output.reported_run_seconds = Some(seconds);
                        }
                    }
                    _ => {}
                },
                Fact::LogCoverage(coverage) => {
                    output.detailed_logging |= coverage.verbose;
                    output.cut_short |= coverage.truncated || coverage.lines_ignored > 0;
                    output.bytes_examined = output.bytes_examined.max(coverage.bytes_examined);
                    output.lines_examined = output.lines_examined.max(coverage.lines_examined);
                }
                _ => {}
            }
        }

        Self {
            schema_version: HISTORY_SCHEMA_VERSION,
            id,
            recorded_at_unix,
            occurrences: 1,
            first_recorded_at_unix: recorded_at_unix,
            launch: report.launch,
            saves: SavedSaves::of(saves),
            game: SavedGame {
                library_id: launch.library_id,
                platform: launch
                    .platform
                    .filter(|name| is_platform_name(name))
                    .map(str::to_string),
            },
            emulator: SavedEmulator {
                name: launch
                    .emulator
                    .filter(|name| is_emulator_name(name))
                    .map(str::to_string),
                core: launch
                    .core
                    .filter(|name| is_core_name(name))
                    .map(str::to_string),
                version: launch
                    .emulator_version
                    .filter(|version| is_version_number(version))
                    .map(str::to_string),
            },
            exit,
            verdict: report.verdict,
            engine_version: report.rule_set.engine_version,
            rule_set_digest: Some(report.rule_set.digest.as_str())
                .filter(|digest| is_digest(digest))
                .unwrap_or_default()
                .to_string(),
            findings,
            findings_not_kept,
            settings,
            graphics,
            missing_libraries: libraries.into_keys().collect(),
            missing_libraries_not_named,
            output,
            not_examined: report
                .coverage
                .iter()
                .filter(|area| !area.has_current_evidence())
                .map(|area| area.area)
                .collect(),
        }
    }

    /// Whether every field holds only what this module would have written.
    /// A stored file is checked with this before any of it is shown, so a
    /// file that was edited, damaged or planted cannot put its own text on
    /// screen.
    fn is_well_formed(&self) -> bool {
        let optional =
            |value: &Option<String>, check: fn(&str) -> bool| value.as_deref().is_none_or(check);
        self.schema_version == HISTORY_SCHEMA_VERSION
            && is_record_id(&self.id)
            && self.id.starts_with(&timestamp(self.recorded_at_unix))
            && self.occurrences >= 1
            && self.first_recorded_at_unix <= self.recorded_at_unix
            && self.saves.monitored.len() <= SaveMechanism::ALL.len()
            && optional(&self.game.platform, is_platform_name)
            && optional(&self.emulator.name, is_emulator_name)
            && optional(&self.emulator.core, is_core_name)
            && optional(&self.emulator.version, is_version_number)
            && (self.rule_set_digest.is_empty() || is_digest(&self.rule_set_digest))
            && self.findings.len() <= MAX_FINDINGS
            && self.findings.iter().all(|finding| {
                is_rule_id(&finding.rule)
                    && (finding.kind == FindingKind::Reported) == finding.message.is_some()
            })
            && self.settings.len() <= MAX_SETTINGS
            && self.settings.iter().all(|setting| match &setting.value {
                SavedValue::Known(value) => {
                    kept_setting_value(&setting.key, value).as_ref() == Some(&setting.value)
                }
                SavedValue::NotSet | SavedValue::NotKept => {
                    KEPT_SETTINGS.iter().any(|(kept, _)| *kept == setting.key)
                }
            })
            && self.graphics.len() <= MAX_GRAPHICS_CHECKS
            && self.missing_libraries.len() <= MAX_LIBRARIES
            && self
                .missing_libraries
                .iter()
                .all(|name| is_kept_library(name))
            && self.not_examined.len() <= 32
    }

    /// The game, for a heading.
    pub fn game_label(&self) -> String {
        match self.game.library_id {
            Some(id) => format!("Game {id} in the library"),
            None => "A game".to_string(),
        }
    }

    /// The emulator and core, for a heading.
    pub fn emulator_label(&self) -> String {
        let name = self.emulator.name.as_deref().unwrap_or("an emulator");
        let mut label = match &self.emulator.version {
            Some(version) => format!("{name} {version}"),
            None => name.to_string(),
        };
        if let Some(core) = &self.emulator.core {
            label.push_str(&format!(", core {core}"));
        }
        label
    }

    /// How the process ended, in plain words.
    pub fn exit_label(&self) -> Option<String> {
        let exit = self.exit.as_ref()?;
        let mut text = ProcessExit {
            outcome: exit.outcome.clone(),
            duration_ms: exit.duration_ms,
            launcher: exit.through_launcher.then(|| "a launcher".to_string()),
        }
        .describe();
        if exit.supplied_by_caller {
            text.push_str(" (as supplied; EmuWiz did not observe it)");
        }
        Some(text)
    }

    /// The record in plain words, one statement per line. Built only from
    /// the record's own fields and the built-in tables.
    pub fn describe(&self) -> Vec<String> {
        let mut lines = vec![
            format!("When: {}", readable_time(self.recorded_at_unix)),
            format!("Game: {}", self.game_label()),
            format!("Emulator: {}", self.emulator_label()),
        ];
        if let Some(platform) = &self.game.platform {
            lines.push(format!("Platform: {platform}"));
        }
        if self.occurrences > 1 {
            lines.push(format!(
                "This happened {} times since {}; launches where it did not happen are not counted.",
                self.occurrences,
                readable_time(self.first_recorded_at_unix)
            ));
        }
        match self.exit_label() {
            Some(exit) => lines.push(format!("The emulator {exit}.")),
            None => lines.push("How the emulator ended was not recorded.".to_string()),
        }
        lines.push(format!("Launch: {}.", self.launch.describe()));
        lines.extend(self.describe_saves());
        lines.push(format!("Diagnosis: {}.", self.verdict.headline()));
        for finding in &self.findings {
            lines.push(describe_finding(finding));
        }
        if self.findings_not_kept > 0 {
            lines.push(format!(
                "{} more finding(s) were not kept.",
                self.findings_not_kept
            ));
        }
        for setting in &self.settings {
            let value = match &setting.value {
                SavedValue::NotSet => continue,
                SavedValue::Known(value) => format!("\"{value}\""),
                SavedValue::NotKept => "a value that is not kept in history".to_string(),
            };
            lines.push(format!(
                "Setting {} was {value} ({}).",
                setting.key,
                setting.layer.label()
            ));
        }
        for check in &self.graphics {
            lines.push(format!(
                "A check of {} {} found it {}.",
                check.api.label(),
                match check.scope {
                    ProbeScope::LaunchEnvironment => "where the emulator runs",
                    ProbeScope::Host => "on this computer in general",
                },
                if check.usable { "usable" } else { "not usable" }
            ));
        }
        for library in &self.missing_libraries {
            lines.push(format!("The program loader could not find {library}."));
        }
        if self.missing_libraries_not_named > 0 {
            lines.push(format!(
                "{} other missing library report(s); their names are not kept in history.",
                self.missing_libraries_not_named
            ));
        }
        if let Some(seconds) = self.output.reported_run_seconds {
            lines.push(format!(
                "The emulator said the game ran for {seconds} second(s)."
            ));
        }
        lines.push(format!(
            "Emulator output examined: {} line(s){}{}. The output itself is not kept.",
            self.output.lines_examined,
            if self.output.detailed_logging {
                ", with detailed logging"
            } else {
                ", without detailed logging"
            },
            if self.output.cut_short {
                ", some of it left out"
            } else {
                ""
            }
        ));
        if !self.not_examined.is_empty() {
            let areas: Vec<&str> = self.not_examined.iter().map(|area| area.label()).collect();
            lines.push(format!("Not examined: {}.", areas.join(", ")));
        }
        lines
    }
}

impl FailureRecord {
    /// The emulator's name as people write it, for wording.
    fn emulator_display_name(&self) -> &'static str {
        match self.emulator.name.as_deref() {
            Some("retroarch") => "RetroArch",
            _ => "The emulator",
        }
    }

    /// What is known about saves, regenerated from fixed values: a problem
    /// with its next step, or what was and was not being checked.
    pub fn describe_saves(&self) -> Vec<String> {
        let assessment = self.saves.assessment();
        match assessment.problem() {
            Some(problem) => vec![
                format!("Saves: {}.", problem.headline()),
                problem.explanation(self.emulator_display_name()),
                format!("What to check: {}", problem.next_step()),
            ],
            None => {
                let mut lines = Vec::new();
                if let Some(line) = assessment.coverage_line() {
                    lines.push(line);
                }
                lines.extend(assessment.coverage_detail());
                lines
            }
        }
    }

    /// Whether this record is a repeat of `earlier`: the same save warning
    /// for the same game on the same emulator, on a launch that did not
    /// fail. Failed launches are never merged; each may differ.
    fn repeats(&self, earlier: &Self) -> bool {
        let warning_only = |record: &Self| {
            !matches!(
                record.launch,
                LaunchOutcome::Failed | LaunchOutcome::FailureReported
            ) && matches!(record.saves.status, SaveWriteStatus::WriteFailed(_))
        };
        let shape = |record: &Self| match record.saves.status {
            SaveWriteStatus::WriteFailed(problem) => {
                Some((problem.standing, problem.mechanism, problem.folder))
            }
            _ => None,
        };
        warning_only(self)
            && warning_only(earlier)
            && self.game.library_id.is_some()
            && self.game.library_id == earlier.game.library_id
            && self.emulator == earlier.emulator
            && self.launch == earlier.launch
            && shape(self) == shape(earlier)
    }
}

fn describe_finding(finding: &SavedFinding) -> String {
    let known = rules::rule_info(&finding.rule).filter(|rule| rule.version == finding.rule_version);
    let contradicted = if finding.contradicted {
        " Some evidence spoke against it."
    } else {
        ""
    };
    match (finding.kind, finding.message) {
        (FindingKind::Reported, Some(message)) => {
            let what = rules::reported_error_phrase(message.meaning)
                .unwrap_or("something this version of EmuWiz has no wording for");
            let times = match message.times {
                0 | 1 => String::new(),
                times => format!(" ({times} times)"),
            };
            format!("Reported: {what}{times}. A report, not a cause.")
        }
        (FindingKind::Cause, _) => format!(
            "Cause ({}): {}. {}{contradicted}",
            finding.confidence.label(),
            finding.category.label(),
            known.map_or(
                "Found by a rule this version of EmuWiz does not have.",
                |rule| rule.summary
            ),
        ),
        (FindingKind::Symptom | FindingKind::Reported, _) => format!(
            "What happened: {}. {}{contradicted}",
            finding.category.label(),
            known.map_or(
                "Found by a rule this version of EmuWiz does not have.",
                |rule| rule.summary
            ),
        ),
    }
}

// --- the store ------------------------------------------------------------

/// The store. Holds no open files between calls.
#[derive(Clone, Debug)]
pub struct HistoryStore {
    directory: PathBuf,
    limits: HistoryLimits,
}

/// `YYYYMMDDTHHMMSSZ-xxxxxxxx`: digits, `T`, `Z`, `-` and lower-case hex only.
fn is_record_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 25
        && bytes[..8].iter().all(u8::is_ascii_digit)
        && bytes[8] == b'T'
        && bytes[9..15].iter().all(u8::is_ascii_digit)
        && bytes[15] == b'Z'
        && bytes[16] == b'-'
        && bytes[17..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn record_file_name(id: &str) -> String {
    format!("{id}.json")
}

fn id_of_file_name(name: &str) -> Option<&str> {
    name.strip_suffix(".json").filter(|id| is_record_id(id))
}

/// Year, month, day, hour, minute, second (UTC) from seconds since the
/// epoch, without a calendar dependency.
fn civil(seconds: u64) -> (i64, i64, i64, u64, u64, u64) {
    let days = seconds / 86_400;
    let rest = seconds % 86_400;
    // Days since 1970-01-01 to year/month/day (proleptic Gregorian).
    let z = i64::try_from(days).unwrap_or(i64::MAX / 2) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (
        year,
        month,
        day,
        rest / 3_600,
        (rest % 3_600) / 60,
        rest % 60,
    )
}

fn timestamp(seconds: u64) -> String {
    let (year, month, day, hour, minute, second) = civil(seconds);
    format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z")
}

/// The time of a record for display, in UTC.
pub fn readable_time(seconds: u64) -> String {
    let (year, month, day, hour, minute, _) = civil(seconds);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC")
}

fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn random_suffix() -> String {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut bytes = [0u8; 4];
    if SystemRandom::new().fill(&mut bytes).is_err() {
        // Not secret: only needs to differ between two records of the same
        // second, and the no-replace rename settles a collision anyway.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        bytes = (nanos ^ std::process::id()).to_le_bytes();
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl HistoryStore {
    /// The store in EmuWiz's data directory.
    pub fn open_default() -> Result<Self, HistoryError> {
        let data = crate::app_dirs::data_dir()
            .map_err(|error| HistoryError::Storage(error.to_string()))?;
        Ok(Self::at(data.join(HISTORY_DIRECTORY)))
    }

    /// A store in a given folder, with the default limits.
    pub fn at(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            limits: HistoryLimits::default(),
        }
    }

    pub fn with_limits(mut self, limits: HistoryLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn limits(&self) -> HistoryLimits {
        self.limits
    }

    /// Open the folder, creating it private if it does not exist, and
    /// refuse it unless it is a real directory that only this user can
    /// reach.
    fn open(&self, create: bool) -> Result<DirectoryHandle, HistoryError> {
        let storage = HistoryError::Storage;
        if create {
            // Only this one folder is created: EmuWiz's data directory must
            // already exist, and is never created or re-permissioned here.
            match std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&self.directory)
            {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(storage(format!("could not be created: {}", error.kind())));
                }
            }
        }
        let metadata = std::fs::symlink_metadata(&self.directory)
            .map_err(|error| storage(format!("could not be examined: {}", error.kind())))?;
        if !metadata.file_type().is_dir() {
            return Err(storage("is not a directory".into()));
        }
        // SAFETY: geteuid has no preconditions and cannot fail.
        let me = unsafe { libc::geteuid() };
        if metadata.uid() != me {
            return Err(storage("belongs to another user".into()));
        }
        if metadata.mode() & 0o077 != 0 {
            return Err(storage(
                "can be reached by other users; it must be private (mode 700)".into(),
            ));
        }
        // The directory that was just checked is the one that gets opened.
        DirectoryHandle::open_verified(&self.directory, DirectoryIdentity::of_metadata(&metadata))
            .map_err(|error| storage(format!("could not be opened: {}", error.kind())))
    }

    /// The folder when there is one. A store nobody has written to yet has
    /// no folder, which is "nothing stored", not an error.
    fn open_existing(&self) -> Result<Option<DirectoryHandle>, HistoryError> {
        match self.open(false) {
            Ok(directory) => Ok(Some(directory)),
            Err(_)
                if std::fs::symlink_metadata(&self.directory)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn entry_names(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.directory) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .take(MAX_DIRECTORY_ENTRIES)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        names.sort();
        names
    }

    /// Whether failed launches are being recorded. On unless switched off.
    /// If the folder cannot be examined, nothing is recorded either way.
    pub fn is_enabled(&self) -> bool {
        match self.open(false) {
            Ok(directory) => !matches!(
                directory.lstat(OsStr::new(DISABLED_MARKER)),
                Ok(Some(_)) | Err(_)
            ),
            // No folder yet: nothing was ever switched off.
            Err(_) => std::fs::symlink_metadata(&self.directory).is_err(),
        }
    }

    /// Switch recording on or off. Switching it off keeps what is already
    /// stored; use [`Self::clear`] to remove that.
    pub fn set_enabled(&self, enabled: bool) -> Result<(), HistoryError> {
        let directory = self.open(true)?;
        let written = if enabled {
            match directory.lstat(OsStr::new(DISABLED_MARKER)) {
                Ok(None) => Ok(()),
                Ok(Some(EntryKind::RegularFile { .. })) => {
                    directory.unlink(OsStr::new(DISABLED_MARKER))
                }
                Ok(Some(_)) => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "the switch is not a regular file",
                )),
                Err(error) => Err(error),
            }
        } else {
            match publish(&directory, DISABLED_MARKER, b"") {
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                other => other,
            }
        };
        written.and_then(|()| directory.sync()).map_err(|error| {
            HistoryError::Storage(format!("could not be changed: {}", error.kind()))
        })
    }

    /// Record a launch that failed, or one whose emulator stated that a
    /// save could not be written. Any other launch is not recorded. Never
    /// panics and never returns an error to propagate: whatever goes wrong
    /// is described in the outcome and the launch is unaffected.
    pub fn record_launch(
        &self,
        launch: &LaunchIdentity<'_>,
        report: &DiagnosisReport,
        now: SystemTime,
    ) -> SaveOutcome {
        let saves = saves::assess(&report.evidence);
        // A launch that adds nothing still brings what is already stored
        // back within its limits (here, and below when recording is off):
        // an old record does not wait for the next failure to be removed.
        if !is_worth_recording(report, &saves) {
            if let Ok(Some(directory)) = self.open_existing() {
                self.enforce_in(&directory, now, 0, 0);
            }
            return SaveOutcome::NotSaved(NotSaved::NothingToRecord);
        }
        // Switched off, or a folder that cannot be used: said apart, so that
        // a broken folder is never reported as a choice the person made.
        match self.open_existing() {
            Ok(Some(directory)) => {
                if !matches!(directory.lstat(OsStr::new(DISABLED_MARKER)), Ok(None)) {
                    self.enforce_in(&directory, now, 0, 0);
                    return SaveOutcome::NotSaved(NotSaved::Disabled);
                }
            }
            // No folder yet: nothing was ever switched off.
            Ok(None) => {}
            Err(HistoryError::Storage(reason)) => {
                return SaveOutcome::NotSaved(NotSaved::StorageUnavailable { reason });
            }
            Err(error) => {
                return SaveOutcome::NotSaved(NotSaved::StorageUnavailable {
                    reason: error.to_string(),
                });
            }
        }
        let recorded_at_unix = unix_seconds(now);
        let id = format!("{}-{}", timestamp(recorded_at_unix), random_suffix());
        let mut record =
            FailureRecord::summarise(id.clone(), recorded_at_unix, launch, report, &saves);
        let unavailable =
            |reason: String| SaveOutcome::NotSaved(NotSaved::StorageUnavailable { reason });
        let directory = match self.open(true) {
            Ok(directory) => directory,
            Err(HistoryError::Storage(reason)) => return unavailable(reason),
            Err(error) => return unavailable(error.to_string()),
        };
        // The same save warning for the same game again: one record that
        // counts, not a new one each time.
        let repeated = self.newest_repeat_of(&directory, &record, now);
        if let Some(earlier) = &repeated {
            record.occurrences = earlier.occurrences.saturating_add(1);
            record.first_recorded_at_unix =
                earlier.first_recorded_at_unix.min(record.recorded_at_unix);
        }
        if !record.is_well_formed() {
            // Cannot happen for a record built above; if it ever does, the
            // record is not one this module would show, so it is not kept.
            return unavailable("the record could not be prepared".into());
        }
        let bytes = match serde_json::to_vec(&record) {
            Ok(bytes) => bytes,
            Err(_) => return unavailable("the record could not be prepared".into()),
        };
        if bytes.len() as u64 > self.limits.max_record_bytes {
            return SaveOutcome::NotSaved(NotSaved::TooLarge {
                bytes: bytes.len() as u64,
            });
        }
        // Room is made first, so the quota holds even between the two steps.
        let mut removed = self.enforce_in(&directory, now, bytes.len() as u64, 1);
        if let Err(error) = publish(&directory, &record_file_name(&id), &bytes) {
            return unavailable(format!("could not be written: {}", error.kind()));
        }
        // Only once the new record is safely stored does the one it
        // replaces go.
        if let Some(earlier) = repeated
            && remove_regular_file(&directory, &record_file_name(&earlier.id))
        {
            removed += 1;
            let _ = directory.sync();
        }
        removed += self.enforce_in(&directory, now, 0, 0);
        SaveOutcome::Saved { id, removed }
    }

    /// The newest stored record that `record` repeats, if any.
    fn newest_repeat_of(
        &self,
        directory: &DirectoryHandle,
        record: &FailureRecord,
        now: SystemTime,
    ) -> Option<FailureRecord> {
        let now_seconds = unix_seconds(now);
        self.entry_names()
            .into_iter()
            .filter(|name| id_of_file_name(name).is_some())
            .filter_map(|name| self.read_record(directory, &name).ok())
            .filter(|earlier| {
                record.repeats(earlier)
                    && earlier.recorded_at_unix <= now_seconds
                    && now_seconds - earlier.recorded_at_unix <= self.limits.max_age.as_secs()
            })
            .max_by(|a, b| (a.recorded_at_unix, &a.id).cmp(&(b.recorded_at_unix, &b.id)))
    }

    /// Read one file as a record. Anything that is not exactly a record
    /// this module could have written is an error.
    fn read_record(
        &self,
        directory: &DirectoryHandle,
        name: &str,
    ) -> Result<FailureRecord, String> {
        let id = id_of_file_name(name).ok_or("not a record name")?;
        let bytes = read_bounded(directory, name, self.limits.max_record_bytes)
            .map_err(|error| error.kind().to_string())?;
        let record: FailureRecord =
            serde_json::from_slice(&bytes).map_err(|_| "not a record".to_string())?;
        if record.id != id {
            return Err("does not match its own name".into());
        }
        if !record.is_well_formed() {
            return Err("holds something a record never holds".into());
        }
        Ok(record)
    }

    /// Every stored record, newest first, after limits were enforced.
    pub fn list(&self, now: SystemTime) -> HistoryListing {
        let Ok(directory) = self.open(false) else {
            return HistoryListing::default();
        };
        self.enforce_in(&directory, now, 0, 0);
        let mut listing = HistoryListing::default();
        for name in self.entry_names() {
            if id_of_file_name(&name).is_none() {
                continue;
            }
            match self.read_record(&directory, &name) {
                Ok(record) => listing.records.push(record),
                Err(_) => listing.unreadable += 1,
            }
        }
        listing
            .records
            .sort_by(|a, b| (b.recorded_at_unix, &b.id).cmp(&(a.recorded_at_unix, &a.id)));
        listing
    }

    /// What [`Self::list`] would show, without removing anything: for a
    /// caller that must only read. Records past a limit are left out of the
    /// view and stay on disk until something that may write tidies up.
    pub fn view(&self, now: SystemTime) -> HistoryListing {
        let Ok(directory) = self.open(false) else {
            return HistoryListing::default();
        };
        let now_seconds = unix_seconds(now);
        let mut listing = HistoryListing::default();
        let mut records: Vec<(FailureRecord, u64)> = Vec::new();
        for name in self.entry_names() {
            if id_of_file_name(&name).is_none() {
                continue;
            }
            match (
                self.read_record(&directory, &name),
                directory.lstat(OsStr::new(&name)),
            ) {
                (Ok(record), Ok(Some(EntryKind::RegularFile { len }))) => {
                    if self.is_within_age(&record, now_seconds) {
                        records.push((record, len));
                    }
                }
                _ => listing.unreadable += 1,
            }
        }
        records.sort_by(|(a, _), (b, _)| {
            (b.recorded_at_unix, &b.id).cmp(&(a.recorded_at_unix, &a.id))
        });
        let mut total = 0u64;
        for (record, size) in records {
            if listing.records.len() >= self.limits.max_records
                || total.saturating_add(size) > self.limits.quota_bytes
            {
                break;
            }
            total += size;
            listing.records.push(record);
        }
        listing
    }

    /// The newest record for a game in the library, if any.
    pub fn latest_for_game(&self, library_id: i64, now: SystemTime) -> Option<FailureRecord> {
        self.list(now)
            .records
            .into_iter()
            .find(|record| record.game.library_id == Some(library_id))
    }

    /// One record by its id, as [`Self::view`] would show it: a record past
    /// the age limit is not found, whether or not it has been removed yet.
    /// Removes nothing.
    pub fn load(&self, id: &str, now: SystemTime) -> Result<FailureRecord, HistoryError> {
        if !is_record_id(id) {
            return Err(HistoryError::InvalidId);
        }
        let Some(directory) = self.open_existing()? else {
            return Err(HistoryError::NotFound);
        };
        let name = record_file_name(id);
        match directory.lstat(OsStr::new(&name)) {
            Ok(None) => return Err(HistoryError::NotFound),
            Ok(Some(_)) => {}
            Err(error) => return Err(HistoryError::Storage(error.kind().to_string())),
        }
        let record = self
            .read_record(&directory, &name)
            .map_err(HistoryError::Unreadable)?;
        if !self.is_within_age(&record, unix_seconds(now)) {
            return Err(HistoryError::NotFound);
        }
        Ok(record)
    }

    /// Whether a record is young enough to be kept and shown. A record dated
    /// more than a day in the future is not trusted to stay.
    fn is_within_age(&self, record: &FailureRecord, now_seconds: u64) -> bool {
        now_seconds.saturating_sub(record.recorded_at_unix) <= self.limits.max_age.as_secs()
            && record.recorded_at_unix <= now_seconds.saturating_add(86_400)
    }

    /// Remove one record. Returns whether there was one to remove.
    pub fn delete(&self, id: &str) -> Result<bool, HistoryError> {
        if !is_record_id(id) {
            return Err(HistoryError::InvalidId);
        }
        let Some(directory) = self.open_existing()? else {
            return Ok(false);
        };
        let removed = remove_regular_file(&directory, &record_file_name(id));
        if removed {
            let _ = directory.sync();
        }
        Ok(removed)
    }

    /// Remove every record and every leftover of an interrupted write. The
    /// folder and the on/off switch are kept. Returns how many files
    /// were removed.
    pub fn clear(&self) -> Result<usize, HistoryError> {
        let directory = match self.open(false) {
            Ok(directory) => directory,
            Err(_) if std::fs::symlink_metadata(&self.directory).is_err() => return Ok(0),
            Err(error) => return Err(error),
        };
        let mut removed = 0;
        for name in self.entry_names() {
            if is_store_file(&name) && remove_regular_file(&directory, &name) {
                removed += 1;
            }
        }
        let _ = directory.sync();
        Ok(removed)
    }

    /// Enforce every limit now. Returns how many files were removed.
    pub fn enforce_limits(&self, now: SystemTime) -> usize {
        match self.open(false) {
            Ok(directory) => self.enforce_in(&directory, now, 0, 0),
            Err(_) => 0,
        }
    }

    /// Remove, in this order: leftovers of interrupted writes, files that
    /// are not readable records, records past the age limit, then the
    /// oldest records until `incoming_records` more records of
    /// `incoming_bytes` in total fit within the count and size limits.
    fn enforce_in(
        &self,
        directory: &DirectoryHandle,
        now: SystemTime,
        incoming_bytes: u64,
        incoming_records: usize,
    ) -> usize {
        let now_seconds = unix_seconds(now);
        let mut removed = 0;
        // (recorded, id, size), for records that are intact.
        let mut kept: Vec<(u64, String, u64)> = Vec::new();
        for name in self.entry_names() {
            if name.starts_with(TEMPORARY_PREFIX) {
                let abandoned = std::fs::symlink_metadata(self.directory.join(&name))
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|modified| now.duration_since(modified).ok())
                    .is_some_and(|age| age >= ABANDONED_AFTER);
                if abandoned && remove_regular_file(directory, &name) {
                    removed += 1;
                }
                continue;
            }
            if id_of_file_name(&name).is_none() {
                // Not this module's file: left exactly where it is.
                continue;
            }
            match (
                self.read_record(directory, &name),
                directory.lstat(OsStr::new(&name)),
            ) {
                (Ok(record), Ok(Some(EntryKind::RegularFile { len }))) => {
                    if !self.is_within_age(&record, now_seconds) {
                        if remove_regular_file(directory, &name) {
                            removed += 1;
                        }
                    } else {
                        kept.push((record.recorded_at_unix, record.id, len));
                    }
                }
                _ => {
                    if remove_regular_file(directory, &name) {
                        removed += 1;
                    }
                }
            }
        }
        // Oldest first; the id breaks ties, so the order is total.
        kept.sort();
        let mut total: u64 = kept.iter().map(|(_, _, size)| size).sum();
        let mut index = 0;
        while index < kept.len()
            && (kept.len() - index + incoming_records > self.limits.max_records
                || total.saturating_add(incoming_bytes) > self.limits.quota_bytes)
        {
            let (_, id, size) = &kept[index];
            if remove_regular_file(directory, &record_file_name(id)) {
                removed += 1;
                total = total.saturating_sub(*size);
            }
            index += 1;
        }
        if removed > 0 {
            let _ = directory.sync();
        }
        removed
    }
}

/// Names this module creates for history: records and temporary files. The
/// on/off switch is not history and is never removed by clearing.
fn is_store_file(name: &str) -> bool {
    id_of_file_name(name).is_some() || name.starts_with(TEMPORARY_PREFIX)
}

/// Remove `name` if it is a regular file in the store's folder. A link, a
/// folder or anything else is left alone: nothing here ever follows a link
/// or removes more than one plain file.
fn remove_regular_file(directory: &DirectoryHandle, name: &str) -> bool {
    matches!(
        directory.lstat(OsStr::new(name)),
        Ok(Some(EntryKind::RegularFile { .. }))
    ) && directory.unlink(OsStr::new(name)).is_ok()
}

fn read_bounded(directory: &DirectoryHandle, name: &str, limit: u64) -> std::io::Result<Vec<u8>> {
    match directory.lstat(OsStr::new(name))? {
        Some(EntryKind::RegularFile { len }) if len <= limit => {}
        Some(EntryKind::RegularFile { .. }) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "larger than a record may be",
            ));
        }
        Some(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        None => return Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
    }
    let mut bytes = Vec::new();
    directory
        .open_file(OsStr::new(name))?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "grew past the limit while it was read",
        ));
    }
    Ok(bytes)
}

/// Write `bytes` under `name` so that the name only ever refers to a
/// complete file: write a private temporary file, flush it to disk, give it
/// its final name without replacing anything, then flush the folder.
pub(super) fn publish(
    directory: &DirectoryHandle,
    name: &str,
    bytes: &[u8],
) -> std::io::Result<()> {
    let temporary = format!(
        "{TEMPORARY_PREFIX}{}-{}",
        std::process::id(),
        random_suffix()
    );
    let written = (|| {
        let mut file = directory.create_new_file(OsStr::new(&temporary))?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        directory.rename_no_replace(OsStr::new(&temporary), OsStr::new(name))
    })();
    if written.is_err() {
        let _ = directory.unlink(OsStr::new(&temporary));
        return written;
    }
    directory.sync()
}

#[cfg(test)]
#[path = "history/tests.rs"]
mod tests;
