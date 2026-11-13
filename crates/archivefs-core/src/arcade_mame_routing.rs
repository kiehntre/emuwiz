//! Per-set routing of extracted Arcade set directories to the installed MAME.
//!
//! Readiness comes from MAME's *own* data for the installed build: the
//! machine's `-listxml` entry (driver status, features, controls, artwork
//! requirement) and its `-verifyroms` result, including any extra read-only
//! ROM search paths. The old DAT catalogue is not consulted.
//!
//! Categories (gambling, fruit/mechanical, mahjong, lightgun) only ever add
//! informational warnings. They are never an exclusion gate: two machines with
//! identical MAME status and verify results always get identical readiness.
//!
//! Nothing here writes. [`probe_arcade_set`] runs the given executable with
//! `-noreadconfig -nowriteconfig` and metadata-only arguments.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};

/// Output cap per MAME metadata command.
const PROBE_OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
/// Most distinct missing files named in one reason.
const MAX_LISTED: usize = 6;

// ---------------------------------------------------------------------------
// Machine facts (from `mame -listxml <set>`)
// ---------------------------------------------------------------------------

/// One flagged `<feature>` (status `imperfect` or `unemulated`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlaggedFeature {
    pub feature: String,
    pub status: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MameMachineFacts {
    pub name: String,
    /// Driver `status` (`good`, `imperfect`, `preliminary`).
    pub status: String,
    /// Driver `emulation`.
    pub emulation: String,
    pub requires_artwork: bool,
    pub is_mechanical: bool,
    pub is_bios: bool,
    pub is_device: bool,
    pub runnable: bool,
    pub has_display: bool,
    pub sourcefile: String,
    pub control_types: Vec<String>,
    pub flagged_features: Vec<FlaggedFeature>,
}

fn attr_map(
    element: &quick_xml::events::BytesStart<'_>,
) -> std::collections::BTreeMap<String, String> {
    let mut map = std::collections::BTreeMap::new();
    for attribute in element.attributes().flatten() {
        let key = String::from_utf8_lossy(attribute.key.as_ref()).to_ascii_lowercase();
        if let Ok(value) = attribute.normalized_value(XmlVersion::Implicit1_0) {
            map.insert(key, value.into_owned());
        }
    }
    map
}

/// Extracts the facts for exactly `set` from `-listxml` output. `Ok(None)`
/// means the output is well formed but names no such machine.
pub fn parse_machine_facts(xml: &str, set: &str) -> Result<Option<MameMachineFacts>, String> {
    let mut reader = Reader::from_str(xml);
    let mut facts: Option<MameMachineFacts> = None;
    let mut depth_in_target = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Err(error) => {
                if facts.is_some() {
                    break;
                }
                return Err(format!("unreadable MAME machine data: {error}"));
            }
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let a = attr_map(&e);
                if tag == "machine" {
                    if facts.is_some() {
                        // The next machine (a device dependency) begins.
                        break;
                    }
                    if a.get("name").map(String::as_str) == Some(set) {
                        let yes = |k: &str| a.get(k).map(String::as_str) == Some("yes");
                        facts = Some(MameMachineFacts {
                            name: set.to_string(),
                            is_bios: yes("isbios"),
                            is_device: yes("isdevice"),
                            is_mechanical: yes("ismechanical"),
                            // `runnable` defaults to yes.
                            runnable: a.get("runnable").map(String::as_str) != Some("no"),
                            sourcefile: a.get("sourcefile").cloned().unwrap_or_default(),
                            ..Default::default()
                        });
                        depth_in_target = 1;
                    }
                    continue;
                }
                let Some(f) = facts.as_mut() else { continue };
                if depth_in_target == 0 {
                    continue;
                }
                match tag.as_str() {
                    "driver" => {
                        f.status = a.get("status").cloned().unwrap_or_default();
                        f.emulation = a.get("emulation").cloned().unwrap_or_default();
                        f.requires_artwork =
                            a.get("requiresartwork").map(String::as_str) == Some("yes");
                    }
                    "display" => f.has_display = true,
                    "control" => {
                        if let Some(kind) = a.get("type")
                            && !f.control_types.contains(kind)
                        {
                            f.control_types.push(kind.clone());
                        }
                    }
                    "feature" => {
                        let status = a.get("status").cloned().unwrap_or_default();
                        if status == "imperfect" || status == "unemulated" {
                            f.flagged_features.push(FlaggedFeature {
                                feature: a.get("type").cloned().unwrap_or_else(|| "unknown".into()),
                                status,
                            });
                        }
                    }
                    _ => {}
                }
            }
            Ok(_) => {}
        }
    }
    Ok(facts)
}

// ---------------------------------------------------------------------------
// Verify result (from `mame -verifyroms <set>`)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerifyState {
    Good,
    /// Only no-good-dump / redump flags remain; MAME treats the set as usable.
    BestAvailable,
    Bad,
    /// None of the set's own files were found.
    NoOwnFiles,
    /// MAME does not know the name.
    UnknownSet,
    Unrecognised,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VerifyProblemKind {
    /// `dependency` is the BIOS/parent/device set MAME names in parentheses.
    NotFound {
        dependency: Option<String>,
    },
    NotFoundNoGoodDump,
    IncorrectLength,
    IncorrectChecksum,
    Other(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifyProblem {
    pub file: String,
    /// MAME prints no `(N bytes)` for CHD disk images.
    pub is_disk: bool,
    /// Expected size from `(N bytes)`; `None` for disks.
    pub size: Option<u64>,
    pub kind: VerifyProblemKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifyReport {
    pub state: VerifyState,
    pub problems: Vec<VerifyProblem>,
}

/// Splits `file (N bytes) - STATUS` (ROM) or `disk - STATUS` (CHD).
fn split_problem_line(rest: &str) -> Option<(String, bool, Option<u64>, &str)> {
    if let Some((file_part, status)) = rest.rsplit_once(" bytes) - ") {
        let (file, size) = match file_part.rsplit_once(" (") {
            Some((name, n)) => (name, n.trim().parse::<u64>().ok()),
            None => (file_part, None),
        };
        return Some((file.trim().to_string(), false, size, status));
    }
    for marker in [" - NOT FOUND", " - INCORRECT ", " - NO GOOD DUMP"] {
        if let Some(at) = rest.find(marker) {
            return Some((rest[..at].trim().to_string(), true, None, &rest[at + 3..]));
        }
    }
    None
}

pub fn parse_verify_output(output: &str, set: &str) -> VerifyReport {
    let mut state = VerifyState::Unrecognised;
    let mut problems = Vec::new();
    let romset_prefix = format!("romset {set} ");
    let not_found = format!("romset \"{set}\" not found");
    for line in output.lines() {
        let line = line.trim_end();
        if line.starts_with("No matching systems found") {
            state = VerifyState::UnknownSet;
        } else if line.starts_with(&not_found) {
            state = VerifyState::NoOwnFiles;
        } else if let Some(rest) = line.strip_prefix(&romset_prefix) {
            let rest = match rest.strip_prefix('[') {
                Some(after) => after.split_once("] ").map_or("", |(_, r)| r),
                None => rest,
            };
            state = match rest {
                "is good" => VerifyState::Good,
                "is best available" => VerifyState::BestAvailable,
                "is bad" => VerifyState::Bad,
                _ => state,
            };
        } else if let Some((owner, rest)) = line.split_once(" : ")
            && owner.trim() == set
            && let Some((file, is_disk, size, status)) = split_problem_line(rest)
        {
            let kind = if let Some(after) = status.strip_prefix("NOT FOUND") {
                let after = after.trim();
                if after.contains("NO GOOD DUMP KNOWN") {
                    VerifyProblemKind::NotFoundNoGoodDump
                } else {
                    let dependency = after
                        .strip_prefix('(')
                        .and_then(|d| d.split_once(')'))
                        .map(|(d, _)| d.to_string());
                    VerifyProblemKind::NotFound { dependency }
                }
            } else if status.starts_with("INCORRECT LENGTH") {
                VerifyProblemKind::IncorrectLength
            } else if status.starts_with("INCORRECT CHECKSUM") {
                VerifyProblemKind::IncorrectChecksum
            } else {
                VerifyProblemKind::Other(status.to_string())
            };
            problems.push(VerifyProblem {
                file,
                is_disk,
                size,
                kind,
            });
        }
    }
    VerifyReport { state, problems }
}

// ---------------------------------------------------------------------------
// Verdict
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArcadeReadiness {
    Ready,
    ReadyWithWarning,
    Blocked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArcadeBlockReason {
    UnknownSet,
    NotAGame,
    NotWorking,
    BiosOrDeviceMissing,
    RomsIncomplete,
    RomsWrongVersion,
    ArtworkMissing,
    CheckFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArcadeMameVerdict {
    pub readiness: ArcadeReadiness,
    pub block_reason: Option<ArcadeBlockReason>,
    /// One plain sentence per blocking reason (first is the headline).
    pub blockers: Vec<String>,
    /// Warnings naming flagged features / special controls. Never gates.
    pub warnings: Vec<String>,
    /// Neutral facts (for example "best available dumps").
    pub notes: Vec<String>,
}

const FRUIT_DRIVER_DIRS: &[&str] = &[
    "barcrest", "bfm", "jpm", "maygay", "ace", "sunwise", "pcp", "mpu", "project", "astra",
];

fn list(items: &[String]) -> String {
    let shown: Vec<&str> = items.iter().take(MAX_LISTED).map(String::as_str).collect();
    let extra = items.len().saturating_sub(MAX_LISTED);
    let mut text = shown.join(", ");
    if extra > 0 {
        text.push_str(&format!(" and {extra} more"));
    }
    text
}

/// Informational labels from MAME's own control/driver data. Never a gate.
pub fn informational_warnings(facts: &MameMachineFacts) -> Vec<String> {
    let has = |t: &str| facts.control_types.iter().any(|c| c == t);
    let mut out = Vec::new();
    for flagged in &facts.flagged_features {
        out.push(format!(
            "MAME flags {} as {}",
            flagged.feature,
            if flagged.status == "unemulated" {
                "not emulated"
            } else {
                "imperfect"
            }
        ));
    }
    if facts.status == "imperfect" && facts.flagged_features.is_empty() {
        out.push("MAME flags this driver as imperfect".into());
    }
    if facts.emulation == "imperfect" && facts.flagged_features.is_empty() {
        out.push("MAME flags the emulation as imperfect".into());
    }
    if has("lightgun") {
        out.push("lightgun game: needs a pointing device or mapped mouse".into());
    }
    if has("mahjong") || has("hanafuda") {
        out.push("mahjong/hanafuda panel controls: needs keyboard mapping".into());
    }
    if has("keyboard") || has("keypad") {
        out.push("uses keyboard/keypad controls".into());
    }
    let fruit_family = facts
        .sourcefile
        .split('/')
        .next()
        .is_some_and(|dir| FRUIT_DRIVER_DIRS.contains(&dir));
    if has("gambling") || facts.is_mechanical || fruit_family {
        out.push("gambling/AWP machine, may need operator setup on first run (special controls or credits)".into());
    }
    if facts.is_mechanical {
        out.push("mechanical machine: reels and lamps are shown only through its layout".into());
    }
    out
}

fn block(reason: ArcadeBlockReason, detail: String) -> ArcadeMameVerdict {
    ArcadeMameVerdict {
        readiness: ArcadeReadiness::Blocked,
        block_reason: Some(reason),
        blockers: vec![detail],
        warnings: Vec::new(),
        notes: Vec::new(),
    }
}

/// What sandbox evidence (claude-d0, 2026-11-06) showed MAME does for one
/// specific set. Per-set, never per-category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetOutcome {
    /// Demonstrated to run; the text qualifies how far (experimental).
    Warn(&'static str),
    /// Demonstrated unusable (or unusable without something not installed).
    Block(&'static str),
}

/// Sets with a demonstrated MAME outcome, from `wrongdump-sample.md` and
/// `artwork-sample.md`. Adding or flipping an entry is a data change.
pub const WRONG_DUMP_EVIDENCE: &[(&str, &str, SetOutcome)] = &[
    (
        "3wonders",
        "ioc1.ic1",
        SetOutcome::Warn(
            "the bad file is a small PLD; MAME showed the title and game-select menu after a notice (sandbox grade A)",
        ),
    ),
    (
        "1943b",
        "bm05.4k",
        SetOutcome::Warn(
            "experimental: MAME showed a notice then gameplay (sandbox grade B; input not isolated)",
        ),
    ),
    (
        "gauntletr4",
        "136037-104.6p",
        SetOutcome::Warn(
            "experimental: MAME showed a notice then attract gameplay (sandbox grade B)",
        ),
    ),
    (
        "sf2cejc",
        "ioc1.ic7",
        SetOutcome::Warn(
            "experimental: MAME showed a notice then the attract intro (sandbox grade B)",
        ),
    ),
    (
        "sailormnot",
        "smprg.u45",
        SetOutcome::Block(
            "a program ROM is wrong and MAME runs to a near-black, unusable screen (sandbox grade F)",
        ),
    ),
    (
        "island2a",
        "m48t35",
        SetOutcome::Block(
            "MAME stops at 'INIT MACHINE' with a blank screen (operator initialisation; sandbox grade F)",
        ),
    ),
];

/// Sets known to start but not give usable play, whatever their ROM state.
pub const KNOWN_FAILED: &[(&str, &str)] = &[(
    "cluedo",
    "MAME starts it to a black screen (not normally playable)",
)];

/// `requiresartwork` sets: block only where missing artwork prevents usable
/// play (artwork-sample.md). Others, including unlisted ones, only warn.
pub const ARTWORK_EVIDENCE: &[(&str, SetOutcome)] = &[
    (
        "solarwar",
        SetOutcome::Block(
            "requires external artwork, not installed: without the backdrop only lamps and score are drawn",
        ),
    ),
    (
        "videopin",
        SetOutcome::Block(
            "requires external artwork, not installed: without the table artwork only flippers and ball are drawn",
        ),
    ),
    (
        "triplhnt",
        SetOutcome::Block(
            "requires external artwork, not installed: its calibration square is not visible and it does not advance",
        ),
    ),
    (
        "m4conn4",
        SetOutcome::Block(
            "requires external artwork, not installed; fruit-machine play also needs an unverified coin/attendant flow",
        ),
    ),
    (
        "gollygho",
        SetOutcome::Warn(
            "MAME asks for external artwork (not installed); the game runs without it; needs a lightgun",
        ),
    ),
];

fn evidence_for(table: &'static [(&'static str, SetOutcome)], set: &str) -> Option<SetOutcome> {
    table.iter().find(|(name, _)| *name == set).map(|(_, o)| *o)
}

/// Pure readiness decision. `artwork_present` is only consulted when MAME says
/// the machine requires external artwork.
pub fn evaluate_arcade_set(
    set: &str,
    facts: Option<&MameMachineFacts>,
    verify: &VerifyReport,
    artwork_present: bool,
) -> ArcadeMameVerdict {
    let Some(facts) = facts.filter(|_| verify.state != VerifyState::UnknownSet) else {
        return block(
            ArcadeBlockReason::UnknownSet,
            format!(
                "{set} is not a machine name known to the installed MAME (renamed or removed in this version)"
            ),
        );
    };
    if facts.is_device || (facts.is_bios && !facts.runnable) || !facts.runnable {
        return block(
            ArcadeBlockReason::NotAGame,
            format!("{set} is a BIOS or device set, not a game MAME can run on its own"),
        );
    }
    // MAME's own words for a machine that does not run.
    if facts.status == "preliminary" || facts.emulation == "preliminary" {
        return block(
            ArcadeBlockReason::NotWorking,
            format!("MAME marks {set} as not working (preliminary emulation); it would not play"),
        );
    }
    if let Some((_, why)) = KNOWN_FAILED.iter().find(|(name, _)| *name == set) {
        return block(ArcadeBlockReason::NotWorking, format!("{set}: {why}"));
    }
    let mut wrong_dump_warning: Option<String> = None;
    match verify.state {
        VerifyState::Good | VerifyState::BestAvailable => {}
        VerifyState::NoOwnFiles => {
            return block(
                ArcadeBlockReason::RomsIncomplete,
                format!(
                    "none of the files this MAME version expects for {set} were found in the set folder (the set may be from a different MAME version)"
                ),
            );
        }
        VerifyState::Bad => match bad_verdict(set, verify) {
            Ok(warning) => wrong_dump_warning = Some(warning),
            Err(blocked) => return blocked,
        },
        _ => {
            return block(
                ArcadeBlockReason::CheckFailed,
                format!("MAME's ROM check for {set} gave no usable answer"),
            );
        }
    }
    let mut warnings: Vec<String> = wrong_dump_warning.into_iter().collect();
    if facts.requires_artwork && !artwork_present {
        match evidence_for(ARTWORK_EVIDENCE, set) {
            Some(SetOutcome::Block(why)) => {
                return block(ArcadeBlockReason::ArtworkMissing, format!("{set} {why}"));
            }
            Some(SetOutcome::Warn(why)) => warnings.push(format!("{set}: {why}")),
            // No demonstrated failure: MAME asks for artwork, so say so, but
            // do not claim the game cannot be played.
            None => warnings.push(format!(
                "{set} may need external artwork (none found in the set folder or MAME's artwork folders)"
            )),
        }
    }
    warnings.extend(informational_warnings(facts));
    let mut notes = Vec::new();
    if verify.state == VerifyState::BestAvailable {
        notes.push(
            "ROMs are the best available dumps (no good dump is known for some parts)".into(),
        );
    }
    ArcadeMameVerdict {
        readiness: if warnings.is_empty() {
            ArcadeReadiness::Ready
        } else {
            ArcadeReadiness::ReadyWithWarning
        },
        block_reason: None,
        blockers: Vec::new(),
        warnings,
        notes,
    }
}

/// `Ok(warning)` only for a wrong-dump-only set with a DEMONSTRATED outcome
/// (an experimental qualification); everything else is `Err(blocked)` naming
/// the bad ROMs. The old "small file" size heuristic is deliberately gone: it
/// was not proof of playability.
fn bad_verdict(set: &str, verify: &VerifyReport) -> Result<String, ArcadeMameVerdict> {
    let mut deps: Vec<String> = Vec::new();
    let mut own_missing: Vec<String> = Vec::new();
    let mut wrong: Vec<String> = Vec::new();
    let mut wrong_files: Vec<&str> = Vec::new();
    for problem in &verify.problems {
        let label = if problem.is_disk {
            format!("disk image {}", problem.file)
        } else {
            problem.file.clone()
        };
        match &problem.kind {
            VerifyProblemKind::NotFound {
                dependency: Some(dep),
            } => {
                if !deps.contains(dep) {
                    deps.push(dep.clone());
                }
            }
            VerifyProblemKind::NotFound { dependency: None } => own_missing.push(label),
            VerifyProblemKind::IncorrectLength | VerifyProblemKind::IncorrectChecksum => {
                let what = if problem.kind == VerifyProblemKind::IncorrectLength {
                    "the wrong length"
                } else {
                    "an incorrect checksum"
                };
                wrong.push(format!("{label} has {what}"));
                wrong_files.push(problem.file.as_str());
            }
            _ => {}
        }
    }
    // Every kind of problem is named; none hides another.
    let mut parts = Vec::new();
    if !own_missing.is_empty() {
        parts.push(format!("missing files: {}", list(&own_missing)));
    }
    if !deps.is_empty() {
        parts.push(format!(
            "needs BIOS/device set {} which MAME could not find",
            list(&deps)
        ));
    }
    if !wrong.is_empty() {
        parts.push(format!(
            "wrong dump version: {} - MAME may refuse or run with a warning",
            list(&wrong)
        ));
    }
    if parts.is_empty() {
        return Err(block(
            ArcadeBlockReason::RomsIncomplete,
            format!("MAME reports the ROMs for {set} are incomplete or wrong"),
        ));
    }
    let detail = format!("{set}: {}", parts.join("; "));
    if own_missing.is_empty() && deps.is_empty() {
        // Evidence is per (set, bad file): a different bad ROM than the one
        // that was tried stays Blocked.
        let outcomes: Vec<Option<SetOutcome>> = wrong_files
            .iter()
            .map(|file| {
                WRONG_DUMP_EVIDENCE
                    .iter()
                    .find(|(s, f, _)| *s == set && f == file)
                    .map(|(_, _, outcome)| *outcome)
            })
            .collect();
        if let Some(Some(SetOutcome::Block(why))) = outcomes
            .iter()
            .find(|o| matches!(o, Some(SetOutcome::Block(_))))
        {
            return Err(block(
                ArcadeBlockReason::RomsWrongVersion,
                format!("{detail}; {why}"),
            ));
        }
        if !outcomes.is_empty()
            && outcomes
                .iter()
                .all(|o| matches!(o, Some(SetOutcome::Warn(_))))
        {
            let how: Vec<&str> = outcomes
                .iter()
                .filter_map(|o| match o {
                    Some(SetOutcome::Warn(h)) => Some(*h),
                    _ => None,
                })
                .collect();
            return Ok(format!(
                "MAME reports incorrect ROM files ({}) and shows a notice; {}",
                list(&wrong),
                how.join("; ")
            ));
        }
    }
    let reason = if !own_missing.is_empty() {
        ArcadeBlockReason::RomsIncomplete
    } else if !deps.is_empty() {
        ArcadeBlockReason::BiosOrDeviceMissing
    } else {
        ArcadeBlockReason::RomsWrongVersion
    };
    Err(block(reason, detail))
}

// ---------------------------------------------------------------------------
// Extra read-only ROM search paths
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExtraRompathError {
    NotAbsolute(PathBuf),
    Missing(PathBuf),
    NotDirectory(PathBuf),
    /// Unresolvable (for example a symlink loop).
    Unresolvable(PathBuf),
    /// `;` separates MAME rompath entries and cannot be part of one.
    ContainsSeparator(PathBuf),
}

impl std::fmt::Display for ExtraRompathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAbsolute(p) => write!(f, "extra ROM path {} is not absolute", p.display()),
            Self::Missing(p) => write!(f, "extra ROM path {} does not exist", p.display()),
            Self::NotDirectory(p) => write!(f, "extra ROM path {} is not a folder", p.display()),
            Self::Unresolvable(p) => write!(f, "extra ROM path {} cannot be resolved", p.display()),
            Self::ContainsSeparator(p) => {
                write!(f, "extra ROM path {} contains ';'", p.display())
            }
        }
    }
}

/// Validates one extra rompath and returns its canonical form. Read-only.
pub fn validate_extra_rompath(path: &Path) -> Result<PathBuf, ExtraRompathError> {
    if !path.is_absolute() {
        return Err(ExtraRompathError::NotAbsolute(path.to_path_buf()));
    }
    if path.to_string_lossy().contains(';') {
        return Err(ExtraRompathError::ContainsSeparator(path.to_path_buf()));
    }
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ExtraRompathError::Missing(path.to_path_buf()));
        }
        Err(_) => return Err(ExtraRompathError::Unresolvable(path.to_path_buf())),
        Ok(_) => {}
    }
    let canonical = std::fs::canonicalize(path)
        .map_err(|_| ExtraRompathError::Unresolvable(path.to_path_buf()))?;
    if !canonical.is_dir() {
        return Err(ExtraRompathError::NotDirectory(path.to_path_buf()));
    }
    if canonical.to_string_lossy().contains(';') {
        return Err(ExtraRompathError::ContainsSeparator(canonical));
    }
    Ok(canonical)
}

/// Validates a list; invalid entries are returned separately so a caller can
/// say why they were ignored. An empty input is the default and is valid.
pub fn validate_extra_rompaths(paths: &[PathBuf]) -> (Vec<PathBuf>, Vec<ExtraRompathError>) {
    let mut good = Vec::new();
    let mut bad = Vec::new();
    for path in paths {
        match validate_extra_rompath(path) {
            Ok(canonical) if !good.contains(&canonical) => good.push(canonical),
            Ok(_) => {}
            Err(error) => bad.push(error),
        }
    }
    (good, bad)
}

/// Reads one-path-per-line (`#` comments, blank lines ignored). A missing or
/// unreadable file is the default: no extra paths. Never writes.
pub fn read_extra_rompath_file(file: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(file)
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

/// The MAME `-rompath` value: the set's own folder first, then each already
/// validated extra path, `;`-separated.
pub fn rompath_argument(set_parent: &Path, extra: &[PathBuf]) -> OsString {
    let mut value = set_parent.as_os_str().to_os_string();
    for path in extra {
        value.push(";");
        value.push(path.as_os_str());
    }
    value
}

// ---------------------------------------------------------------------------
// Artwork presence (only used when MAME says artwork is required)
// ---------------------------------------------------------------------------

/// Looks for artwork in the places derivable read-only: the set folder
/// itself (`*.lay` or an `artwork` folder) and `artwork/<set>[.zip]` under the
/// collection folder, its parent and the executable's folder. MAME's real
/// artpath cannot be known from here, so "not found" means "not located".
pub fn artwork_located(set: &str, set_dir: &Path, executable: &Path) -> bool {
    if set_folder_has_artwork(set_dir) {
        return true;
    }
    let mut bases: Vec<PathBuf> = Vec::new();
    if let Some(collection) = set_dir.parent() {
        bases.push(collection.join("artwork"));
        if let Some(above) = collection.parent() {
            bases.push(above.join("artwork"));
        }
    }
    if let Some(exe_dir) = executable.parent() {
        bases.push(exe_dir.join("artwork"));
    }
    // MAME 0.264's compiled-in artpath (per `mame -showconfig`).
    if let Some(home) = std::env::var_os("HOME") {
        bases.push(PathBuf::from(home).join("mame").join("artwork"));
    }
    bases.push(PathBuf::from("/usr/local/share/games/mame/artwork"));
    bases.push(PathBuf::from("/usr/share/games/mame/artwork"));
    bases
        .iter()
        .any(|base| base.join(format!("{set}.zip")).exists() || base.join(set).is_dir())
}

/// True when the set folder itself carries a layout (`*.lay`) or an
/// `artwork` folder.
pub fn set_folder_has_artwork(set_dir: &Path) -> bool {
    if set_dir.join("artwork").is_dir() {
        return true;
    }
    std::fs::read_dir(set_dir).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("lay"))
        })
    })
}

// ---------------------------------------------------------------------------
// Probe (runs the executable with metadata-only arguments)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArcadeProbe {
    pub facts: Option<MameMachineFacts>,
    pub verify: VerifyReport,
    pub verdict: ArcadeMameVerdict,
}

fn read_bounded(reader: impl Read) -> Vec<u8> {
    let mut bytes = Vec::new();
    reader
        .take((PROBE_OUTPUT_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .ok();
    bytes.truncate(PROBE_OUTPUT_LIMIT);
    bytes
}

/// Runs the executable with a deadline and cancel flag; `None` on spawn
/// failure, timeout or cancellation. Output (stdout+stderr) is bounded.
fn run_bounded(
    executable: &Path,
    args: &[OsString],
    timeout: Duration,
    cancel: &AtomicBool,
) -> Option<String> {
    let mut child =
        std::os::unix::process::CommandExt::process_group(Command::new(executable).args(args), 0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?;
    let stdout = child.stdout.take()?;
    let stderr = child.stderr.take()?;
    let out = thread::spawn(|| read_bounded(stdout));
    let err = thread::spawn(|| read_bounded(stderr));
    let deadline = Instant::now() + timeout;
    let finished = loop {
        match child.try_wait() {
            Ok(Some(_)) => break true,
            Ok(None) if Instant::now() >= deadline || cancel.load(Ordering::Relaxed) => {
                // Kill the whole group while the child is still unreaped, so
                // its pid (= group id) cannot have been reused yet.
                // SAFETY: the group id is our own just-spawned child's pid.
                unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                let _ = child.wait();
                break false;
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => break false,
        }
    };
    // After a normal exit the child is already reaped, so there is a tiny pid
    // reuse window before this group kill; it only matters to reach a
    // grandchild that kept the pipes open (otherwise the readers would hang).
    // SAFETY: the group id is our own just-spawned child's pid (process_group(0)).
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    let mut bytes = out.join().unwrap_or_default();
    bytes.extend(err.join().unwrap_or_default());
    finished.then(|| String::from_utf8_lossy(&bytes).into_owned())
}

/// A set name becomes one argv component and a folder name.
pub fn is_probe_safe_set_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Asks the installed MAME about one set. Read-only: both commands are
/// metadata queries run with `-noreadconfig -nowriteconfig`; the set folder's
/// parent (and any validated extra paths) are the only ROM search paths.
pub fn probe_arcade_set(
    executable: &Path,
    set: &str,
    set_dir: &Path,
    extra_rompaths: &[PathBuf],
    timeout: Duration,
    cancel: &AtomicBool,
) -> ArcadeProbe {
    let failed = |detail: &str| ArcadeProbe {
        facts: None,
        verify: VerifyReport {
            state: VerifyState::Unrecognised,
            problems: Vec::new(),
        },
        verdict: block(ArcadeBlockReason::CheckFailed, detail.to_string()),
    };
    if !is_probe_safe_set_name(set) {
        return failed("the set folder name is not a plain MAME set name");
    }
    let Some(parent) = set_dir.parent() else {
        return failed("the set folder has no parent collection folder");
    };
    if parent.to_string_lossy().contains(';') || !parent.is_absolute() {
        return failed(
            "the collection folder path contains ';' or is not absolute, so it cannot be a MAME ROM path",
        );
    }
    let (extra, _rejected) = validate_extra_rompaths(extra_rompaths);
    let base = [
        OsString::from("-noreadconfig"),
        OsString::from("-nowriteconfig"),
    ];
    let mut verify_args = base.to_vec();
    verify_args.extend([
        OsString::from("-rompath"),
        rompath_argument(parent, &extra),
        OsString::from("-verifyroms"),
        OsString::from(set),
    ]);
    let mut xml_args = base.to_vec();
    xml_args.extend([OsString::from("-listxml"), OsString::from(set)]);
    let Some(verify_text) = run_bounded(executable, &verify_args, timeout, cancel) else {
        return failed("MAME's ROM check did not finish (timed out or cancelled)");
    };
    let Some(xml_text) = run_bounded(executable, &xml_args, timeout, cancel) else {
        return failed("MAME's machine data query did not finish (timed out or cancelled)");
    };
    let verify = parse_verify_output(&verify_text, set);
    let facts = parse_machine_facts(&xml_text, set).unwrap_or(None);
    let artwork = artwork_located(set, set_dir, executable);
    let verdict = evaluate_arcade_set(set, facts.as_ref(), &verify, artwork);
    ArcadeProbe {
        facts,
        verify,
        verdict,
    }
}

#[cfg(test)]
mod tests;
