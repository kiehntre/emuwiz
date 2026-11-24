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

use crate::launch::process_spawn::CapturedFileIdentity;
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
    /// ROM files this machine lists ONLY under a BIOS version other than the
    /// one MAME starts by default (`<biosset default="yes">`, else the first).
    /// MAME loads just the selected BIOS version's files, so these being
    /// missing never stops a default start. A file name that also appears
    /// under the default version, or with no `bios` attribute, is never here.
    pub alternative_bios_files: Vec<AlternativeBiosFile>,
    /// Devices this machine uses (`<device_ref>`). MAME names the owning
    /// device in parentheses when a file that device needs is missing.
    pub device_refs: Vec<String>,
    /// `cloneof` and `romof` from `-listxml` (MAME reads the parent's ini).
    pub clone_of: Option<String>,
    pub rom_of: Option<String>,
    /// Declared BIOS version names in order, and the one MAME starts when no
    /// setting chooses (`default="yes"`, else the first).
    pub bios_versions: Vec<String>,
    pub default_bios: Option<String>,
    /// `<display>` (`type`, `rotate`) pairs, for the screen and orientation
    /// ini files MAME reads.
    pub displays: Vec<(String, String)>,
}

/// A ROM file used only by a non-default BIOS version, with its listed size.
/// A verify line is exempted only when both name and size match.
#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct AlternativeBiosFile {
    pub name: String,
    pub size: Option<u64>,
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
    // (name, declared default) in document order; (rom file, bios version or None).
    let mut bios_sets: Vec<(String, bool)> = Vec::new();
    let mut rom_bios: Vec<(String, Option<String>, Option<u64>)> = Vec::new();
    // Set only on the target's own `</machine>`: a read error or EOF before
    // it (truncated or killed output) leaves the BIOS data unproven.
    let mut target_complete = false;
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
                            clone_of: a.get("cloneof").cloned(),
                            rom_of: a.get("romof").cloned(),
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
                    "display" => {
                        f.has_display = true;
                        f.displays.push((
                            a.get("type").cloned().unwrap_or_default(),
                            a.get("rotate").cloned().unwrap_or_default(),
                        ));
                    }
                    "biosset" => {
                        if let Some(name) = a.get("name") {
                            let default = a.get("default").map(String::as_str) == Some("yes");
                            bios_sets.push((name.clone(), default));
                        }
                    }
                    "rom" => {
                        if let Some(name) = a.get("name") {
                            rom_bios.push((
                                name.clone(),
                                a.get("bios").cloned(),
                                a.get("size").and_then(|n| n.parse::<u64>().ok()),
                            ));
                        }
                    }
                    "device_ref" => {
                        if let Some(name) = a.get("name")
                            && !f.device_refs.contains(name)
                        {
                            f.device_refs.push(name.clone());
                        }
                    }
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
            Ok(Event::End(e)) => {
                if facts.is_some() && e.name().as_ref() == b"machine" {
                    target_complete = true;
                    break;
                }
            }
            Ok(_) => {}
        }
    }
    if let Some(f) = facts.as_mut()
        && target_complete
    {
        f.alternative_bios_files = alternative_bios_files(&bios_sets, &rom_bios);
        f.bios_versions = bios_sets.iter().map(|(name, _)| name.clone()).collect();
        f.default_bios = bios_sets
            .iter()
            .find(|(_, is_default)| *is_default)
            .or_else(|| bios_sets.first())
            .map(|(name, _)| name.clone());
    }
    Ok(facts)
}

/// Files listed only under non-default BIOS versions (see
/// [`MameMachineFacts::alternative_bios_files`]). Nothing is optional when
/// there are no BIOS versions, or when any file names a BIOS version the
/// machine does not declare (the metadata cannot be trusted). With no
/// `default="yes"`, MAME starts the first declared version: `-listxml` marks
/// only an explicit default (2,740 of 3,446 BIOS machines in 0.264 have
/// none), and Neo Geo / PlayChoice-10 sets with only other versions' files
/// missing were demonstrated to start (Oracle A1-A start check).
fn alternative_bios_files(
    bios_sets: &[(String, bool)],
    rom_bios: &[(String, Option<String>, Option<u64>)],
) -> Vec<AlternativeBiosFile> {
    let default = bios_sets
        .iter()
        .find(|(_, is_default)| *is_default)
        .or_else(|| bios_sets.first())
        .map(|(name, _)| name.as_str());
    let Some(default) = default else {
        return Vec::new();
    };
    let declared = |b: &str| bios_sets.iter().any(|(name, _)| name == b);
    if rom_bios
        .iter()
        .any(|(_, bios, _)| bios.as_deref().is_some_and(|b| !declared(b)))
    {
        return Vec::new();
    }
    let needed_by_default = |file: &str| {
        rom_bios
            .iter()
            .any(|(name, bios, _)| name == file && bios.as_deref().is_none_or(|b| b == default))
    };
    let mut out: Vec<AlternativeBiosFile> = rom_bios
        .iter()
        .filter(|(name, bios, _)| {
            bios.as_deref().is_some_and(|b| b != default) && !needed_by_default(name)
        })
        .map(|(name, _, size)| AlternativeBiosFile {
            name: name.clone(),
            size: *size,
        })
        .collect();
    out.sort();
    out.dedup();
    out
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
        VerifyState::Bad => match bad_verdict(
            set,
            verify,
            &facts.alternative_bios_files,
            &facts.device_refs,
        ) {
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
fn is_alternative_bios_problem(
    problem: &VerifyProblem,
    alternative_bios_files: &[AlternativeBiosFile],
    device_refs: &[String],
) -> bool {
    let owned_by_device = matches!(
        &problem.kind,
        VerifyProblemKind::NotFound { dependency: Some(dep) } if device_refs.contains(dep)
    );
    matches!(problem.kind, VerifyProblemKind::NotFound { .. })
        && !owned_by_device
        && !problem.is_disk
        && problem.size.is_some()
        && alternative_bios_files
            .iter()
            .any(|f| f.name == problem.file && f.size == problem.size)
}

/// The sets named in parentheses on the exempted lines (the BIOS root, for
/// example `neogeo`), or `None` when the verdict does not use the exemption.
fn exemption_providers(facts: &MameMachineFacts, verify: &VerifyReport) -> Option<Vec<String>> {
    if verify.state != VerifyState::Bad {
        return None;
    }
    let mut used = false;
    let mut providers = Vec::new();
    for problem in &verify.problems {
        if is_alternative_bios_problem(problem, &facts.alternative_bios_files, &facts.device_refs) {
            used = true;
            if let VerifyProblemKind::NotFound {
                dependency: Some(dep),
            } = &problem.kind
                && !providers.contains(dep)
            {
                providers.push(dep.clone());
            }
        }
    }
    used.then_some(providers)
}

/// The `bios` settings MAME's ini files hold, read once and shared by every
/// set (see [`read_bios_settings`]).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BiosSettings {
    /// The global value `-showconfig` reports (mame.ini); empty when unset.
    pub global: String,
    /// Ini name (`aof`, `source/neogeo`) -> (file, `bios` value). Only the
    /// first file of each name on the `inipath` counts, as MAME opens only
    /// that one; a file without a `bios` line is recorded with an empty value
    /// because it still hides later files of the same name.
    pub files: std::collections::BTreeMap<String, (PathBuf, String)>,
    /// What these settings were read from, to see later whether they changed.
    pub evidence: BiosSettingsEvidence,
}

/// What the BIOS settings were read from, recorded before they were parsed:
/// for every ini folder (and its `source/` folder) its identity and the names
/// of its `.ini` files, and for every `.ini` file its identity and a SHA-256
/// of its contents. Identity is device, inode, size, modification and
/// status-change time, of the path itself and of what it resolves to.
///
/// Timestamps alone are not enough: the kernel stamps them from a coarse
/// clock, so a change within the same tick keeps them equal. The name list
/// sees a file added or removed and the digest sees any edit, including one
/// of the same size with the modification time put back; identity also sees
/// an atomic replacement and a retargeted symlink. Checking lists only these
/// few folders and reads only these small files.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BiosSettingsEvidence {
    observed: Vec<(PathBuf, Observed)>,
}

type FileStamp = (CapturedFileIdentity, CapturedFileIdentity);

#[derive(Clone, Debug, Eq, PartialEq)]
enum Observed {
    Absent,
    /// Never equal to a later observation: fails closed.
    Unreadable,
    Folder(FileStamp, Vec<String>),
    File(FileStamp, Vec<u8>),
}

fn stamp_of(path: &Path) -> Option<FileStamp> {
    let own = CapturedFileIdentity::capture(&std::fs::symlink_metadata(path).ok()?);
    let resolved = CapturedFileIdentity::capture(&std::fs::metadata(path).ok()?);
    Some((own, resolved))
}

fn absent_or_unreadable(path: &Path) -> Observed {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Observed::Absent,
        _ => Observed::Unreadable,
    }
}

/// The `.ini` file names directly in `dir`, sorted.
fn ini_names(dir: &Path) -> Option<Vec<String>> {
    let mut names = Vec::new();
    for item in std::fs::read_dir(dir).ok()? {
        let name = item.ok()?.file_name();
        if let Some(name) = name.to_str()
            && name.ends_with(".ini")
        {
            names.push(name.to_string());
        }
    }
    names.sort();
    Some(names)
}

fn observe_folder(dir: &Path) -> Observed {
    let Some(stamp) = stamp_of(dir) else {
        return absent_or_unreadable(dir);
    };
    match ini_names(dir) {
        Some(names) => Observed::Folder(stamp, names),
        None => Observed::Unreadable,
    }
}

fn observe_file(path: &Path) -> Observed {
    use sha2::{Digest, Sha256};
    if std::fs::metadata(path).is_ok_and(|meta| meta.is_dir()) {
        return observe_folder(path);
    }
    let Some(stamp) = stamp_of(path) else {
        return absent_or_unreadable(path);
    };
    if stamp.1.size > 1 << 20 {
        return Observed::Unreadable;
    }
    match std::fs::read(path) {
        Ok(bytes) => Observed::File(stamp, Sha256::digest(&bytes).to_vec()),
        Err(_) => Observed::Unreadable,
    }
}

impl BiosSettingsEvidence {
    /// Whether every recorded folder and file is unchanged (a recorded
    /// absence must still be absent). Anything unreadable is a change.
    pub fn is_current(&self) -> bool {
        self.observed.iter().all(|(path, then)| {
            let now = match then {
                Observed::File(..) => observe_file(path),
                _ => observe_folder(path),
            };
            now != Observed::Unreadable && now == *then
        })
    }
}

/// Reads the `bios` settings from MAME's own `-showconfig` output (global
/// value and `inipath`) and the `.ini` files in each `inipath` folder and its
/// `source/` folder. Only `bios` lines are read; nothing else is interpreted.
/// Fails closed (`Err`) when the output lacks `bios` or `inipath`, when an
/// `inipath` entry depends on the working directory or on a variable other
/// than `$HOME`, or when a folder or file cannot be read or is implausibly
/// large.
fn read_bios_settings(showconfig: &str, home: Option<&Path>) -> Result<BiosSettings, String> {
    let value = |key: &str| {
        showconfig.lines().find_map(|line| {
            let (k, v) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            (k == key).then(|| v.trim().to_string())
        })
    };
    let (Some(global), Some(inipath)) = (value("bios"), value("inipath")) else {
        return Err("MAME did not report its settings".into());
    };
    let mut settings = BiosSettings {
        global: ini_value(&global),
        files: std::collections::BTreeMap::new(),
        evidence: BiosSettingsEvidence::default(),
    };
    let mut checked = 0usize;
    for entry in inipath
        .trim_matches('"')
        .split(';')
        .filter(|entry| !entry.is_empty())
    {
        let folder = match entry.strip_prefix("$HOME") {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => match home {
                Some(home) => home.join(rest.trim_start_matches('/')),
                None => return Err("MAME's ini path uses $HOME, which is not set".into()),
            },
            _ => PathBuf::from(entry),
        };
        if !folder.is_absolute() || folder.to_string_lossy().contains('$') {
            return Err(format!(
                "MAME's ini path entry {entry} depends on the working directory or the environment"
            ));
        }
        for (dir, prefix) in [(folder.clone(), ""), (folder.join("source"), "source/")] {
            settings
                .evidence
                .observed
                .push((dir.clone(), observe_folder(&dir)));
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    return Err(format!(
                        "MAME's ini folder {} could not be read",
                        dir.display()
                    ));
                }
            };
            let mut names = Vec::new();
            for item in entries {
                let unreadable = || format!("an ini file in {} could not be read", dir.display());
                let path = item.map_err(|_| unreadable())?.path();
                if let Some(stem) = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.strip_suffix(".ini"))
                {
                    names.push((stem.to_string(), path));
                }
            }
            names.sort();
            for (stem, path) in names {
                // Recorded before parsing: a change in between reads as stale.
                settings
                    .evidence
                    .observed
                    .push((path.clone(), observe_file(&path)));
                checked += 1;
                if checked > 4096 {
                    return Err("MAME's ini folders hold too many files to check".into());
                }
                let key = format!("{prefix}{stem}");
                if settings.files.contains_key(&key) {
                    continue;
                }
                let meta = std::fs::metadata(&path)
                    .map_err(|_| format!("{} could not be read", path.display()))?;
                if meta.is_dir() {
                    continue;
                }
                if meta.len() > 1 << 20 {
                    return Err(format!("{} is too large to check", path.display()));
                }
                let text = std::fs::read(&path)
                    .map_err(|_| format!("{} could not be read", path.display()))?;
                let bios = String::from_utf8_lossy(&text)
                    .lines()
                    .filter_map(|line| {
                        let line = line.trim();
                        let (k, v) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
                        k.eq_ignore_ascii_case("bios").then(|| ini_value(v))
                    })
                    .next_back()
                    .unwrap_or_default();
                settings.files.insert(key, (path, bios));
            }
        }
    }
    Ok(settings)
}

/// The first token of an ini value without quotes (`"us" # note` -> `us`).
fn ini_value(raw: &str) -> String {
    raw.split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches('"')
        .to_string()
}

/// `Ok(())` when MAME will start `facts.default_bios` for this set, given
/// `settings`; otherwise why that is not confirmed.
///
/// Measured with MAME 0.264 (BIOS chosen with an empty rompath, no ROM read):
/// the set's own ini, its parent's, its BIOS root's (`neogeo.ini` for `aof`
/// and for the clone `kof98a`), `source/<driver>.ini`, `horizont.ini`,
/// `raster.ini` and mame.ini change the version; an unrelated set's ini,
/// another case of the name, `vertical.ini` for a horizontal set,
/// `debug.ini`, `arcade.ini` and `console.ini` did not. A name the set does
/// not declare makes MAME revert to the default; names match in any case, and
/// a number selects by position.
///
/// Files are visited from the highest priority down. A setting that names
/// another version, or a number, is unconfirmed. A file that certainly
/// applies ends the walk, because it overrides everything below it. Files
/// that only may apply (orientation, screen, system type, debug, the BIOS
/// root) never end it, so a lower setting still counts.
fn effective_bios_check(
    facts: &MameMachineFacts,
    providers: &[String],
    settings: &BiosSettings,
) -> Result<(), String> {
    let Some(default) = facts.default_bios.as_deref() else {
        return Err("the set declares no default BIOS version".into());
    };
    // MAME also reads the grandparent's ini. In 0.264 every BIOS machine has
    // at most two generations above it, and the grandparent is always the
    // BIOS root MAME names on the exempted lines. A clone whose lines name
    // nothing beyond itself and its parent leaves the grandparent unknown.
    if facts.clone_of.is_some()
        && providers
            .iter()
            .all(|p| *p == facts.name || Some(p) == facts.clone_of.as_ref())
    {
        return Err("the grandparent set whose settings MAME also reads is not known".into());
    }
    let file = |name: &str| {
        settings
            .files
            .get(name)
            .map(|(path, bios)| (path.display().to_string(), bios.clone()))
    };
    let screens = |want: &[&str]| {
        facts.displays.is_empty()
            || facts
                .displays
                .iter()
                .any(|(kind, _)| want.contains(&kind.as_str()))
    };
    let rotated = |want_vertical: bool| {
        facts.displays.is_empty()
            || facts
                .displays
                .iter()
                .any(|(_, rotate)| (rotate == "90" || rotate == "270") == want_vertical)
    };
    let source = Path::new(&facts.sourcefile)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| format!("source/{stem}"));
    // (where, value, certainly applies), highest priority first.
    let mut layers: Vec<(String, String, bool)> = Vec::new();
    let mut add = |name: Option<String>, certain: bool, applies: bool| {
        if let Some(name) = name
            && applies
            && let Some((path, bios)) = file(&name)
        {
            layers.push((path, bios, certain));
        }
    };
    add(Some(facts.name.clone()), true, true);
    add(facts.clone_of.clone(), true, true);
    let mut roots: Vec<String> = providers.to_vec();
    if let Some(rom_of) = &facts.rom_of {
        roots.push(rom_of.clone());
    }
    for root in roots {
        if Some(&root) != facts.clone_of.as_ref() && root != facts.name {
            add(Some(root), false, true);
        }
    }
    add(source, true, true);
    for (name, applies) in [
        ("raster", screens(&["raster"])),
        ("vector", screens(&["vector"])),
        ("lcd", screens(&["lcd"])),
        ("arcade", true),
        ("console", true),
        ("computer", true),
        ("othersys", true),
        ("horizont", rotated(false)),
        ("vertical", rotated(true)),
        ("debug", true),
    ] {
        add(Some(name.to_string()), false, applies);
    }
    layers.push((
        "MAME's global settings".into(),
        settings.global.clone(),
        true,
    ));
    for (place, bios, certain) in layers {
        if bios.is_empty() {
            continue;
        }
        if bios.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("{place} selects BIOS version number {bios}"));
        }
        if !bios.eq_ignore_ascii_case(default)
            && facts
                .bios_versions
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&bios))
        {
            return Err(format!("{place} selects BIOS version {bios}"));
        }
        if certain {
            return Ok(());
        }
    }
    Ok(())
}

/// Withdraws the default-BIOS exemption when the BIOS version MAME will start
/// for this set is not confirmed to be the default ([`effective_bios_check`]):
/// the other versions' files then count as required, exactly as without the
/// exemption. A user's BIOS choice is respected, never overridden, and no
/// `-bios` is added anywhere. `settings` runs only when a verdict that is not
/// blocked depends on the exemption.
fn confirm_effective_bios(
    set: &str,
    facts: Option<&MameMachineFacts>,
    verify: &VerifyReport,
    artwork_present: bool,
    verdict: ArcadeMameVerdict,
    settings: impl FnOnce() -> Result<BiosSettings, String>,
) -> (ArcadeMameVerdict, Option<BiosSettingsEvidence>) {
    let Some(facts) = facts else {
        return (verdict, None);
    };
    if verdict.readiness == ArcadeReadiness::Blocked {
        return (verdict, None);
    }
    let Some(providers) = exemption_providers(facts, verify) else {
        return (verdict, None);
    };
    let why = match settings() {
        Ok(read) => match effective_bios_check(facts, &providers, &read) {
            // Ready on the strength of these settings: the launch rechecks
            // that they are unchanged (see `preflight_mame_launch`).
            Ok(()) => return (verdict, Some(read.evidence)),
            Err(why) => why,
        },
        Err(why) => why,
    };
    let strict = MameMachineFacts {
        alternative_bios_files: Vec::new(),
        ..facts.clone()
    };
    let mut strict_verdict = evaluate_arcade_set(set, Some(&strict), verify, artwork_present);
    strict_verdict.notes.push(format!(
        "files for other BIOS versions count as required because MAME may not start the default BIOS version: {why}"
    ));
    (strict_verdict, None)
}

/// One read of MAME's BIOS settings shared by all probes, so a library-wide
/// check does not rescan the ini folders per set. It is reused only while its
/// evidence is unchanged (stat only), so a changed setting is never served.
type BiosSettingsCache = Option<(PathBuf, Option<std::ffi::OsString>, BiosSettings)>;
static BIOS_SETTINGS_CACHE: std::sync::Mutex<BiosSettingsCache> = std::sync::Mutex::new(None);

fn mame_bios_settings(
    executable: &Path,
    timeout: Duration,
    cancel: &AtomicBool,
    trace: &mut ProbeTrace,
) -> Result<BiosSettings, String> {
    let home = std::env::var_os("HOME");
    if let Ok(cache) = BIOS_SETTINGS_CACHE.lock()
        && let Some((exe, cached_home, settings)) = cache.as_ref()
        && exe == executable
        && *cached_home == home
        && settings.evidence.is_current()
    {
        return Ok(settings.clone());
    }
    // Read with the launch's own settings: no -noreadconfig here.
    let args = [
        OsString::from("-nowriteconfig"),
        OsString::from("-showconfig"),
    ];
    let output = match trace
        .showconfig_run
        .insert(run_bounded(executable, &args, timeout, cancel))
    {
        Ok(output) => output.text.clone(),
        Err(_) => return Err("MAME's settings could not be read (timed out or cancelled)".into()),
    };
    let settings = read_bios_settings(&output, home.as_deref().map(Path::new))?;
    if let Ok(mut cache) = BIOS_SETTINGS_CACHE.lock() {
        *cache = Some((executable.to_path_buf(), home, settings.clone()));
    }
    Ok(settings)
}

fn bad_verdict(
    set: &str,
    verify: &VerifyReport,
    alternative_bios_files: &[AlternativeBiosFile],
    device_refs: &[String],
) -> Result<String, ArcadeMameVerdict> {
    let mut deps: Vec<String> = Vec::new();
    let mut own_missing: Vec<String> = Vec::new();
    let mut wrong: Vec<String> = Vec::new();
    let mut wrong_files: Vec<&str> = Vec::new();
    let mut optional_bios_missing: Vec<&str> = Vec::new();
    // A problem this function does not classify (an unrecognised status, or
    // anything other than MAME's own "no good dump known") must never be
    // outweighed by the BIOS exemption.
    let mut unexplained = false;
    for problem in &verify.problems {
        // A missing file that only another BIOS version uses does not stop
        // MAME starting with its default BIOS (demonstrated: aof, kof98,
        // pc_mman3, cbtime start with these reported NOT FOUND).
        // Name AND listed size must match, and never for a disk image: a
        // device ROM or CHD that merely shares a name is not exempted. A line
        // MAME attributes to one of this machine's devices belongs to that
        // device, even when its name and size coincide.
        if is_alternative_bios_problem(problem, alternative_bios_files, device_refs) {
            optional_bios_missing.push(problem.file.as_str());
            continue;
        }
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
            VerifyProblemKind::NotFoundNoGoodDump => {}
            VerifyProblemKind::Other(_) => unexplained = true,
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
    if parts.is_empty() && !unexplained && !optional_bios_missing.is_empty() {
        return Ok(format!(
            "{set}: only files for other BIOS versions are missing ({}); MAME starts with its default BIOS",
            list(
                &optional_bios_missing
                    .iter()
                    .map(|f| f.to_string())
                    .collect::<Vec<_>>()
            )
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
    /// Present when the verdict is not blocked only because of the
    /// default-BIOS exemption: the BIOS settings it relied on. A launch must
    /// confirm [`BiosSettingsEvidence::is_current`] first.
    pub bios_evidence: Option<BiosSettingsEvidence>,
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

/// Why one bounded MAME run gave no output to parse. Production only needs
/// "it did not finish"; the detail is kept so a test failure can say whether
/// the program could not start (with the raw OS error), timed out, was
/// cancelled or could not be waited on.
#[derive(Debug)]
#[allow(
    dead_code,
    reason = "diagnostic detail, read only through Debug when a probe test fails (issue #479)"
)]
enum RunFailure {
    /// `Command::spawn` failed; the `io::Error` carries the raw OS error.
    Spawn(std::io::Error),
    /// The child started without the piped stdout/stderr it was given.
    PipeMissing,
    /// `try_wait` failed while the child was running.
    Wait(std::io::Error),
    TimedOut {
        after: Duration,
    },
    Cancelled {
        after: Duration,
    },
}

/// A run that finished on its own: exit status plus bounded stdout+stderr.
#[derive(Debug)]
#[allow(
    dead_code,
    reason = "diagnostic detail, read only through Debug when a probe test fails (issue #479)"
)]
struct RunOutput {
    status: std::process::ExitStatus,
    elapsed: Duration,
    text: String,
}

/// Runs the executable with a deadline and cancel flag; an error on spawn
/// failure, timeout or cancellation. Output (stdout+stderr) is bounded.
fn run_bounded(
    executable: &Path,
    args: &[OsString],
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<RunOutput, RunFailure> {
    let started = Instant::now();
    let mut child =
        std::os::unix::process::CommandExt::process_group(Command::new(executable).args(args), 0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(RunFailure::Spawn)?;
    let stdout = child.stdout.take().ok_or(RunFailure::PipeMissing)?;
    let stderr = child.stderr.take().ok_or(RunFailure::PipeMissing)?;
    let out = thread::spawn(|| read_bounded(stdout));
    let err = thread::spawn(|| read_bounded(stderr));
    let deadline = Instant::now() + timeout;
    let finished = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() >= deadline || cancel.load(Ordering::Relaxed) => {
                let after = started.elapsed();
                let why = if cancel.load(Ordering::Relaxed) {
                    RunFailure::Cancelled { after }
                } else {
                    RunFailure::TimedOut { after }
                };
                // Kill the whole group while the child is still unreaped, so
                // its pid (= group id) cannot have been reused yet.
                // SAFETY: the group id is our own just-spawned child's pid.
                unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                let _ = child.wait();
                break Err(why);
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => break Err(RunFailure::Wait(error)),
        }
    };
    // After a normal exit the child is already reaped, so there is a tiny pid
    // reuse window before this group kill; it only matters to reach a
    // grandchild that kept the pipes open (otherwise the readers would hang).
    // SAFETY: the group id is our own just-spawned child's pid (process_group(0)).
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    let mut bytes = out.join().unwrap_or_default();
    bytes.extend(err.join().unwrap_or_default());
    let status = finished?;
    Ok(RunOutput {
        status,
        elapsed: started.elapsed(),
        text: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

/// What each MAME run in one probe did. Not part of the verdict: production
/// drops it; tests print it so a failure shows its cause.
#[derive(Debug, Default)]
#[allow(
    dead_code,
    reason = "diagnostic detail, read only through Debug when a probe test fails (issue #479)"
)]
struct ProbeTrace {
    verify_run: Option<Result<RunOutput, RunFailure>>,
    listxml_run: Option<Result<RunOutput, RunFailure>>,
    showconfig_run: Option<Result<RunOutput, RunFailure>>,
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
/// When readiness depends on the default-BIOS exemption, MAME's settings are
/// read too (`-nowriteconfig -showconfig`, without `-noreadconfig`, plus the
/// `bios` lines of its ini files; see [`confirm_effective_bios`]).
pub fn probe_arcade_set(
    executable: &Path,
    set: &str,
    set_dir: &Path,
    extra_rompaths: &[PathBuf],
    timeout: Duration,
    cancel: &AtomicBool,
) -> ArcadeProbe {
    probe_arcade_set_traced(executable, set, set_dir, extra_rompaths, timeout, cancel).0
}

/// [`probe_arcade_set`] plus what each MAME run did. The probe it returns is
/// exactly the one `probe_arcade_set` returns.
fn probe_arcade_set_traced(
    executable: &Path,
    set: &str,
    set_dir: &Path,
    extra_rompaths: &[PathBuf],
    timeout: Duration,
    cancel: &AtomicBool,
) -> (ArcadeProbe, ProbeTrace) {
    let mut trace = ProbeTrace::default();
    let failed = |detail: &str| ArcadeProbe {
        facts: None,
        verify: VerifyReport {
            state: VerifyState::Unrecognised,
            problems: Vec::new(),
        },
        verdict: block(ArcadeBlockReason::CheckFailed, detail.to_string()),
        bios_evidence: None,
    };
    if !is_probe_safe_set_name(set) {
        return (
            failed("the set folder name is not a plain MAME set name"),
            trace,
        );
    }
    let Some(parent) = set_dir.parent() else {
        return (
            failed("the set folder has no parent collection folder"),
            trace,
        );
    };
    if parent.to_string_lossy().contains(';') || !parent.is_absolute() {
        return (
            failed(
                "the collection folder path contains ';' or is not absolute, so it cannot be a MAME ROM path",
            ),
            trace,
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
    let verify_run =
        trace
            .verify_run
            .insert(run_bounded(executable, &verify_args, timeout, cancel));
    let Ok(verify_output) = verify_run else {
        return (
            failed("MAME's ROM check did not finish (timed out or cancelled)"),
            trace,
        );
    };
    let verify = parse_verify_output(&verify_output.text, set);
    let xml_run = trace
        .listxml_run
        .insert(run_bounded(executable, &xml_args, timeout, cancel));
    let Ok(xml_output) = xml_run else {
        return (
            failed("MAME's machine data query did not finish (timed out or cancelled)"),
            trace,
        );
    };
    let facts = parse_machine_facts(&xml_output.text, set).unwrap_or(None);
    let artwork = artwork_located(set, set_dir, executable);
    let verdict = evaluate_arcade_set(set, facts.as_ref(), &verify, artwork);
    let (verdict, bios_evidence) =
        confirm_effective_bios(set, facts.as_ref(), &verify, artwork, verdict, || {
            mame_bios_settings(executable, timeout, cancel, &mut trace)
        });
    (
        ArcadeProbe {
            facts,
            verify,
            verdict,
            bios_evidence,
        },
        trace,
    )
}

#[cfg(test)]
mod tests;
