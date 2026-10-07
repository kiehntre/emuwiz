//! DuckStation native cheat adapter, v1.
//!
//! Scope: PlayStation 1, DuckStation standalone's own format, one verified
//! single-disc serial, the plain `SERIAL.cht` file and the matching per-game
//! settings file `<GameSettings>/SERIAL.ini`. Everything else is refused with a
//! typed [`DuckStationNativeRefusal`].
//!
//! # Verified DuckStation behaviour this relies on
//!
//! Read from upstream `cheats.cpp`, `cheats_private.h` and `settings.cpp`:
//!
//! * Cheats are enabled **only** through the per-game settings layer:
//!   `[Cheats] EnableCheats = true` plus one `Enable = <cheat name>` line per
//!   enabled cheat. There is no global switch to touch.
//! * Folders come from `settings.ini` `[Folders] Cheats` / `GameSettings`
//!   (defaults `cheats` / `gamesettings`). A relative value is joined to the
//!   data root, an absolute one is used as-is.
//! * Every file matching `<serial>*.cht` in the cheats folder is loaded, and a
//!   later cheat with the same name overwrites an earlier one, so any file other
//!   than exactly `<serial>.cht` makes the effective cheat set order-dependent
//!   and is refused here.
//! * Community-database cheats load from `cheats.zip` unless the game INI says
//!   `[Cheats] LoadCheatsFromDatabase = false`, and an on-disk cheat overwrites a
//!   database cheat of the same name. That shadowing cannot be seen offline, so
//!   it must be disproved or explicitly acknowledged.
//!
//! # Transaction model
//!
//! The `.cht` file and the game INI are one logical operation built from two
//! single-file shared transactions (they may live in different folders, which
//! one shared plan cannot express): preflight, stage and verify both outputs in
//! memory, publish the `.cht` (inert without the INI), then the INI, then verify
//! both from disk and write a receipt. If any step fails after the first
//! publication, the already-published side is rolled back through its shared
//! journal. Undo reverts the INI first (so a partial undo is inert) and then the
//! `.cht`. The same pattern is used by the PCSX2 and RetroArch adapters.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::duckstation_cheat::{
    DUCKSTATION_CHEAT_MAX_BYTES, DUCKSTATION_CHEAT_MAX_LINES, direct_code,
};
use super::{
    PreviewAdapter, PreviewIdentity, PreviewIdentityKind, PreviewIdentityState,
    PreviewMatchStrength, PreviewSourceItem, SharedApplyConfirmation, SharedApplyOptions,
    SharedApplyStatus, SharedMaterializedOutput, SharedPreviewRequest, SharedRollbackConfirmation,
    SharedRollbackOptions, SharedTransactionPlan, build_shared_preview,
    build_shared_transaction_plan, execute_shared_materialized_apply, execute_shared_rollback,
    preview_shared_rollback,
};

const SETTINGS_MAX_BYTES: u64 = 1024 * 1024;
const RECEIPT_MAX_BYTES: u64 = 256 * 1024;
const RECEIPT_SCHEMA_VERSION: u32 = 1;
const MAX_CHEAT_NAME_CHARS: usize = 128;
const MAX_CODE_LINES: usize = 512;

/// Why a request cannot be previewed or applied. No variant writes anything.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DuckStationNativeRefusal {
    /// No usable verified PS1 serial (title/filename guesses never count).
    MissingVerifiedSerial,
    /// More than one distinct verified serial was offered.
    AmbiguousSerial { serials: Vec<String> },
    /// The game belongs to a disc set; V1 does not route disc sets.
    UnsupportedMultiDisc,
    /// Single-disc was not established by the caller's identity evidence.
    DiscTopologyUnproven,
    /// Another `<serial>*.cht` file exists, so load order is uncertain.
    HashSpecificVariantPresent { files: Vec<String> },
    /// A same-named cheat exists with different content.
    CheatNameConflict { name: String },
    /// An update/remove/enable named a cheat that is not in the file.
    CheatNotFound { name: String },
    /// A destination changed between preview and apply.
    DestinationChangedAfterPreview { path: PathBuf },
    /// The DuckStation data root or its `settings.ini` is unusable.
    InvalidDuckStationProfile { reason: String },
    /// A configured Cheats/GameSettings folder is not safe to write to.
    UnsafeCustomFolder { setting: String, reason: String },
    /// Database cheats could shadow (or be shadowed by) this cheat unseen.
    DatabaseShadowingUnknown,
    /// The game INI cannot be edited unambiguously.
    IniUpdateConflict { reason: String },
    /// An existing destination is unreadable, oversized, not text, or no longer
    /// matches what the caller previewed.
    ExistingFileChanged { path: PathBuf, reason: String },
    /// An incoming code line is not a direct write this adapter can verify.
    UnsupportedCheatCode { name: String, line: String },
    /// The incoming cheat is malformed (name, metadata, comments).
    InvalidCheat { reason: String },
    /// The operator has not confirmed.
    ConfirmationRequired,
    /// The operation identifier is not a safe file-name component.
    InvalidOperationId,
}

impl DuckStationNativeRefusal {
    /// Plain-language explanation suitable for the UI.
    #[must_use]
    pub fn explain(&self) -> String {
        match self {
            Self::MissingVerifiedSerial => {
                "EmuWiz has no verified PlayStation serial for this game, so it will not guess a DuckStation cheat file.".into()
            }
            Self::AmbiguousSerial { serials } => format!(
                "This game matched more than one serial ({}); EmuWiz will not pick one.",
                serials.join(", ")
            ),
            Self::UnsupportedMultiDisc => {
                "This game is part of a multi-disc set. Native DuckStation cheats for disc sets are not supported yet.".into()
            }
            Self::DiscTopologyUnproven => {
                "EmuWiz could not confirm this is a single-disc game, so it will not write DuckStation cheats.".into()
            }
            Self::HashSpecificVariantPresent { files } => format!(
                "The cheats folder already holds other files for this serial ({}). DuckStation loads all of them, so the result would depend on load order.",
                files.join(", ")
            ),
            Self::CheatNameConflict { name } => format!(
                "A different cheat named \"{name}\" already exists in this game's cheat file. EmuWiz will not replace it silently."
            ),
            Self::CheatNotFound { name } => {
                format!("No cheat named \"{name}\" exists in this game's cheat file.")
            }
            Self::DestinationChangedAfterPreview { path } => format!(
                "{} changed after the preview. Nothing was written; preview again.",
                path.display()
            ),
            Self::InvalidDuckStationProfile { reason } => {
                format!("This DuckStation profile cannot be used: {reason}")
            }
            Self::UnsafeCustomFolder { setting, reason } => {
                format!("The DuckStation {setting} folder is not safe to write to: {reason}")
            }
            Self::DatabaseShadowingUnknown => {
                "DuckStation may also load community cheats for this game. EmuWiz cannot see them, so it cannot tell whether this cheat would replace one with the same name. Turn off \"Load cheats from database\" for this game, or acknowledge the risk.".into()
            }
            Self::IniUpdateConflict { reason } => {
                format!("The game's DuckStation settings file cannot be updated safely: {reason}")
            }
            Self::ExistingFileChanged { path, reason } => {
                format!("{} cannot be edited safely: {reason}", path.display())
            }
            Self::UnsupportedCheatCode { name, line } => format!(
                "Cheat \"{name}\" contains a code line EmuWiz cannot verify as a direct memory write: {line}"
            ),
            Self::InvalidCheat { reason } => format!("The cheat is not valid: {reason}"),
            Self::ConfirmationRequired => "Confirmation is required before anything is written.".into(),
            Self::InvalidOperationId => "The operation identifier is not valid.".into(),
        }
    }
}

/// What the caller's identity evidence says about disc sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DuckStationDiscTopology {
    SingleDisc,
    MultiDisc,
    Unknown,
}

/// One cheat to add or update. Only direct memory-write code lines
/// (`30` 8-bit, `80` 16-bit, `90` 32-bit) are accepted for writing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuckStationNativeCheat {
    pub name: String,
    /// Extra metadata (`Description`, `Author`, ...). `Type` defaults to
    /// `Gameshark` and `Activation` to `EndFrame`.
    pub metadata: BTreeMap<String, String>,
    /// Comment lines (each starting with `;` or `#`) kept under the header.
    pub comments: Vec<String>,
    pub code_lines: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DuckStationNativeOperation {
    /// Add a cheat; refused if a different cheat has the same name.
    Add {
        cheat: DuckStationNativeCheat,
        enable: bool,
    },
    /// Replace one cheat. `expected_existing_digest` (from
    /// [`duckstation_native_section_digest`]) proves the caller previewed the
    /// exact section being replaced.
    Update {
        cheat: DuckStationNativeCheat,
        expected_existing_digest: String,
    },
    Remove {
        name: String,
    },
    /// Change only the per-game enablement of an existing cheat.
    SetEnabled {
        name: String,
        enabled: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuckStationNativeRequest {
    /// The DuckStation data root (the folder holding `settings.ini`).
    pub profile_root: PathBuf,
    pub selected_game: PathBuf,
    /// Verified serials from identity evidence; normally exactly one.
    pub verified_serials: Vec<String>,
    pub disc_topology: DuckStationDiscTopology,
    pub operation: DuckStationNativeOperation,
    /// The caller has told the user that a same-named community cheat may be
    /// overwritten and the user accepted.
    pub acknowledge_database_shadowing: bool,
}

/// Resolved destinations, with whether each came from `settings.ini`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuckStationFolders {
    pub data_root: PathBuf,
    pub cheats: PathBuf,
    pub game_settings: PathBuf,
    pub cheats_custom: bool,
    pub game_settings_custom: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnableCheatsChange {
    /// No INI change involving `EnableCheats` is needed.
    NotRequired,
    /// Already `true`.
    AlreadyTrue,
    /// The `[Cheats]` section or key will be created.
    Created,
    /// An existing value will be changed to `true`.
    ChangedToTrue { from: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DuckStationNativeWarning {
    /// The cheat may overwrite a same-named community-database cheat.
    DatabaseShadowingAcknowledged,
    /// DuckStation must reload cheats (or restart the game) to see changes.
    ReloadRequired,
}

/// Everything a person needs to see before confirming. Building it writes
/// nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuckStationNativePreview {
    pub serial: String,
    pub cht_path: PathBuf,
    pub game_ini_path: PathBuf,
    pub folders: DuckStationFolders,
    pub cheats_added: Vec<String>,
    pub cheats_changed: Vec<String>,
    pub cheats_removed: Vec<String>,
    pub enable_cheats: EnableCheatsChange,
    pub enable_entries_added: Vec<String>,
    pub enable_entries_removed: Vec<String>,
    pub warnings: Vec<DuckStationNativeWarning>,
    pub cht_exists: bool,
    pub game_ini_exists: bool,
    pub cht_will_change: bool,
    pub game_ini_will_change: bool,
    /// An existing file will be replaced, so it is backed up first.
    pub backup_will_be_made: bool,
    /// Something changes, so it can be undone from the receipt.
    pub undo_available: bool,
    pub no_op: bool,
    pub cht_before_sha256: Option<String>,
    pub cht_after_sha256: Option<String>,
    pub game_ini_before_sha256: Option<String>,
    pub game_ini_after_sha256: Option<String>,
}

#[derive(Clone, Debug)]
struct FilePlan {
    destination: PathBuf,
    root: PathBuf,
    relative: String,
    before: Option<Vec<u8>>,
    before_sha256: Option<String>,
    after: Vec<u8>,
    after_sha256: String,
}

#[derive(Clone, Debug)]
struct Expectations {
    /// Cheat names that must exist in the published `.cht` (with section digests).
    present_sections: Vec<(String, String)>,
    absent_sections: Vec<String>,
    /// Digests of every section that must be unchanged.
    preserved_sections: Vec<(String, String)>,
    /// Lines before the first section header must be unchanged.
    preamble: Vec<String>,
    /// Original INI lines that must remain, in order.
    ini_kept: Vec<String>,
    enable_cheats_true: bool,
    enable_present: Vec<String>,
    enable_absent: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct DuckStationNativePlan {
    pub preview: DuckStationNativePreview,
    cht: Option<FilePlan>,
    ini: Option<FilePlan>,
    cht_final: Vec<u8>,
    ini_final: Vec<u8>,
    expectations: Expectations,
    cht_destination: PathBuf,
    ini_destination: PathBuf,
    selected_game: PathBuf,
}

// ---------------------------------------------------------------------------
// Hashing / small helpers
// ---------------------------------------------------------------------------

fn sha256_hex(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn has_parent_dir(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::ParentDir))
}

fn safe_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

// ---------------------------------------------------------------------------
// Line-preserving text model
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct Line {
    text: String,
    /// `"\n"`, `"\r\n"`, or `""` for an unterminated final line.
    eol: &'static str,
}

fn split_lines(text: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        match rest.find('\n') {
            Some(index) => {
                let (head, tail) = rest.split_at(index);
                let (content, eol) = match head.strip_suffix('\r') {
                    Some(content) => (content, "\r\n"),
                    None => (head, "\n"),
                };
                lines.push(Line {
                    text: content.to_string(),
                    eol,
                });
                rest = &tail[1..];
            }
            None => {
                lines.push(Line {
                    text: rest.to_string(),
                    eol: "",
                });
                break;
            }
        }
    }
    lines
}

fn join_lines(lines: &[Line]) -> String {
    let mut out = String::new();
    for line in lines {
        out.push_str(&line.text);
        out.push_str(line.eol);
    }
    out
}

fn dominant_eol(lines: &[Line]) -> &'static str {
    let crlf = lines.iter().filter(|line| line.eol == "\r\n").count();
    let lf = lines.iter().filter(|line| line.eol == "\n").count();
    if crlf > lf { "\r\n" } else { "\n" }
}

fn ensure_terminated(lines: &mut [Line], eol: &'static str) {
    if let Some(last) = lines.last_mut()
        && last.eol.is_empty()
    {
        last.eol = eol;
    }
}

fn is_blank(line: &Line) -> bool {
    line.text.trim().is_empty()
}

fn is_comment(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with(';') || trimmed.starts_with('#')
}

fn header_name(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (trimmed.len() >= 2 && trimmed.starts_with('[') && trimmed.ends_with(']'))
        .then(|| trimmed[1..trimmed.len() - 1].trim().to_string())
}

fn key_value(text: &str) -> Option<(&str, &str)> {
    if is_comment(text) || header_name(text).is_some() {
        return None;
    }
    let (key, value) = text.split_once('=')?;
    Some((key.trim(), value.trim()))
}

fn truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "yes" | "on"
    )
}

fn falsy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "false" | "0" | "no" | "off"
    )
}

/// The single value of `key` in `section`, or an error when it is repeated with
/// different values.
fn ini_value(text: &str, section: &str, key: &str) -> Result<Option<String>, String> {
    let mut current = false;
    let mut found: Option<String> = None;
    for line in split_lines(text) {
        if let Some(name) = header_name(&line.text) {
            current = name == section;
            continue;
        }
        if !current {
            continue;
        }
        if let Some((found_key, value)) = key_value(&line.text)
            && found_key == key
        {
            match &found {
                Some(existing) if existing != value => {
                    return Err(format!("[{section}] {key} is set more than once"));
                }
                _ => found = Some(value.to_string()),
            }
        }
    }
    Ok(found)
}

// ---------------------------------------------------------------------------
// `.cht` editing
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct ChtSection {
    name: String,
    start: usize,
    end: usize,
}

fn cht_sections(lines: &[Line]) -> Vec<ChtSection> {
    let mut sections: Vec<ChtSection> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if is_comment(&line.text) {
            continue;
        }
        if let Some(name) = header_name(&line.text) {
            if let Some(last) = sections.last_mut() {
                last.end = index;
            }
            sections.push(ChtSection {
                name,
                start: index,
                end: lines.len(),
            });
        }
    }
    sections
}

/// Digest of one section's canonical text (right-trimmed lines, no trailing
/// blank lines), independent of line endings and of the blank lines that merely
/// separate it from the next section.
fn section_digest_of(lines: &[Line]) -> String {
    let mut texts: Vec<&str> = lines.iter().map(|line| line.text.trim_end()).collect();
    while texts.last().is_some_and(|text| text.is_empty()) {
        texts.pop();
    }
    sha256_hex(texts.join("\n").as_bytes())
}

/// The digest [`DuckStationNativeOperation::Update`] must carry for `name` in
/// `cht_text`, or `None` when the name is absent or not unique.
#[must_use]
pub fn duckstation_native_section_digest(cht_text: &str, name: &str) -> Option<String> {
    let lines = split_lines(cht_text);
    let sections = cht_sections(&lines);
    let mut matching = sections.iter().filter(|section| section.name == name);
    let section = matching.next()?;
    if matching.next().is_some() {
        return None;
    }
    Some(section_digest_of(&lines[section.start..section.end]))
}

fn validate_cheat(cheat: &DuckStationNativeCheat) -> Result<Vec<String>, DuckStationNativeRefusal> {
    let invalid = |reason: &str| DuckStationNativeRefusal::InvalidCheat {
        reason: reason.to_string(),
    };
    let name = &cheat.name;
    if name.is_empty()
        || name.trim() != name
        || name.chars().count() > MAX_CHEAT_NAME_CHARS
        || name.chars().any(|c| c.is_control() || c == '[' || c == ']')
    {
        return Err(invalid(
            "the name must be 1-128 characters with no brackets, control characters or edge spaces",
        ));
    }
    for comment in &cheat.comments {
        if !(comment.starts_with(';') || comment.starts_with('#')) || comment.contains(['\r', '\n'])
        {
            return Err(invalid(
                "comments must be single lines starting with ; or #",
            ));
        }
    }
    for (key, value) in &cheat.metadata {
        if key.trim() != key
            || key.is_empty()
            || key.contains(['=', '[', ']', '\r', '\n'])
            || value.contains(['\r', '\n'])
        {
            return Err(invalid(
                "metadata keys and values must be single plain lines",
            ));
        }
    }
    if let Some(kind) = cheat.metadata.get("Type")
        && kind.trim() != "Gameshark"
    {
        return Err(DuckStationNativeRefusal::UnsupportedCheatCode {
            name: name.clone(),
            line: format!("Type = {kind}"),
        });
    }
    if let Some(activation) = cheat.metadata.get("Activation")
        && !matches!(activation.trim(), "EndFrame" | "Manual")
    {
        return Err(invalid("Activation must be EndFrame or Manual"));
    }
    if cheat.code_lines.is_empty() || cheat.code_lines.len() > MAX_CODE_LINES {
        return Err(invalid("a cheat needs between 1 and 512 code lines"));
    }
    let mut canonical = Vec::with_capacity(cheat.code_lines.len());
    for line in &cheat.code_lines {
        if direct_code(line.trim()).is_none() {
            return Err(DuckStationNativeRefusal::UnsupportedCheatCode {
                name: name.clone(),
                line: line.clone(),
            });
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        canonical.push(format!(
            "{} {}",
            fields[0].to_ascii_uppercase(),
            fields[1].to_ascii_uppercase()
        ));
    }
    Ok(canonical)
}

fn render_cheat_lines(
    cheat: &DuckStationNativeCheat,
    codes: &[String],
    eol: &'static str,
) -> Vec<Line> {
    let mut texts = vec![format!("[{}]", cheat.name)];
    texts.extend(cheat.comments.iter().cloned());
    let kind = cheat
        .metadata
        .get("Type")
        .map_or("Gameshark", |value| value.trim());
    let activation = cheat
        .metadata
        .get("Activation")
        .map_or("EndFrame", |value| value.trim());
    texts.push(format!("Type = {kind}"));
    texts.push(format!("Activation = {activation}"));
    for (key, value) in &cheat.metadata {
        if key != "Type" && key != "Activation" {
            texts.push(format!("{key} = {value}"));
        }
    }
    texts.extend(codes.iter().cloned());
    texts.into_iter().map(|text| Line { text, eol }).collect()
}

#[derive(Debug)]
struct ChtEdit {
    after: Vec<Line>,
    added: Vec<String>,
    changed: Vec<String>,
    removed: Vec<String>,
}

fn unique_section<'a>(
    sections: &'a [ChtSection],
    name: &str,
    path: &Path,
) -> Result<Option<&'a ChtSection>, DuckStationNativeRefusal> {
    let mut matching = sections.iter().filter(|section| section.name == name);
    let first = matching.next();
    if first.is_some() && matching.next().is_some() {
        return Err(DuckStationNativeRefusal::ExistingFileChanged {
            path: path.to_path_buf(),
            reason: format!("the cheat file defines \"{name}\" more than once"),
        });
    }
    Ok(first)
}

fn edit_cht(
    before: Option<&str>,
    operation: &DuckStationNativeOperation,
    path: &Path,
) -> Result<ChtEdit, DuckStationNativeRefusal> {
    let mut lines = before.map(split_lines).unwrap_or_default();
    let eol = if lines.is_empty() {
        "\n"
    } else {
        dominant_eol(&lines)
    };
    let sections = cht_sections(&lines);
    let mut edit = ChtEdit {
        after: Vec::new(),
        added: Vec::new(),
        changed: Vec::new(),
        removed: Vec::new(),
    };
    match operation {
        DuckStationNativeOperation::Add { cheat, .. } => {
            let codes = validate_cheat(cheat)?;
            let rendered = render_cheat_lines(cheat, &codes, eol);
            match unique_section(&sections, &cheat.name, path)? {
                Some(existing) => {
                    if section_digest_of(&lines[existing.start..existing.end])
                        != section_digest_of(&rendered)
                    {
                        return Err(DuckStationNativeRefusal::CheatNameConflict {
                            name: cheat.name.clone(),
                        });
                    }
                }
                None => {
                    ensure_terminated(&mut lines, eol);
                    if lines.last().is_some_and(|last| !is_blank(last)) {
                        lines.push(Line {
                            text: String::new(),
                            eol,
                        });
                    }
                    lines.extend(rendered);
                    edit.added.push(cheat.name.clone());
                }
            }
        }
        DuckStationNativeOperation::Update {
            cheat,
            expected_existing_digest,
        } => {
            let codes = validate_cheat(cheat)?;
            let existing = unique_section(&sections, &cheat.name, path)?.ok_or_else(|| {
                DuckStationNativeRefusal::CheatNotFound {
                    name: cheat.name.clone(),
                }
            })?;
            let old = &lines[existing.start..existing.end];
            if section_digest_of(old) != *expected_existing_digest {
                return Err(DuckStationNativeRefusal::ExistingFileChanged {
                    path: path.to_path_buf(),
                    reason: format!(
                        "\"{}\" is not the section that was previewed for replacement",
                        cheat.name
                    ),
                });
            }
            let trailing_blanks = old.iter().rev().take_while(|line| is_blank(line)).count();
            let final_eol = old.last().map_or(eol, |line| line.eol);
            let mut replacement = render_cheat_lines(cheat, &codes, eol);
            if section_digest_of(&replacement) == section_digest_of(old) {
                // Identical canonical content: nothing to change.
            } else {
                for _ in 0..trailing_blanks {
                    replacement.push(Line {
                        text: String::new(),
                        eol,
                    });
                }
                if let Some(last) = replacement.last_mut() {
                    last.eol = final_eol;
                }
                lines.splice(existing.start..existing.end, replacement);
                edit.changed.push(cheat.name.clone());
            }
        }
        DuckStationNativeOperation::Remove { name } => {
            let existing = unique_section(&sections, name, path)?
                .ok_or_else(|| DuckStationNativeRefusal::CheatNotFound { name: name.clone() })?;
            lines.drain(existing.start..existing.end);
            edit.removed.push(name.clone());
        }
        DuckStationNativeOperation::SetEnabled { name, enabled } => {
            if *enabled && unique_section(&sections, name, path)?.is_none() {
                return Err(DuckStationNativeRefusal::CheatNotFound { name: name.clone() });
            }
        }
    }
    edit.after = lines;
    Ok(edit)
}

// ---------------------------------------------------------------------------
// Game INI editing
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
enum IniChange {
    None,
    Enable(String),
    Disable(String),
}

#[derive(Debug)]
struct IniEdit {
    after: Vec<Line>,
    enable_cheats: EnableCheatsChange,
    entries_added: Vec<String>,
    entries_removed: Vec<String>,
    /// Original line texts that must survive, in order (everything except the
    /// lines this edit deliberately removed or rewrote).
    kept: Vec<String>,
}

fn edit_game_ini(
    before: Option<&str>,
    change: &IniChange,
    path: &Path,
) -> Result<IniEdit, DuckStationNativeRefusal> {
    let conflict = |reason: &str| DuckStationNativeRefusal::IniUpdateConflict {
        reason: format!("{} - {reason}", path.display()),
    };
    let mut lines = before.map(split_lines).unwrap_or_default();
    let eol = if lines.is_empty() {
        "\n"
    } else {
        dominant_eol(&lines)
    };
    let original: Vec<String> = lines.iter().map(|line| line.text.clone()).collect();
    let mut dropped: BTreeSet<usize> = BTreeSet::new();
    let mut edit = IniEdit {
        after: Vec::new(),
        enable_cheats: EnableCheatsChange::NotRequired,
        entries_added: Vec::new(),
        entries_removed: Vec::new(),
        kept: Vec::new(),
    };
    // Header positions: (index, name).
    let headers: Vec<(usize, String)> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !is_comment(&line.text))
        .filter_map(|(index, line)| header_name(&line.text).map(|name| (index, name)))
        .collect();
    let cheats: Vec<usize> = headers
        .iter()
        .enumerate()
        .filter(|(_, (_, name))| name == "Cheats")
        .map(|(position, _)| position)
        .collect();
    let section_range = |position: usize| -> (usize, usize) {
        let start = headers[position].0;
        let end = headers
            .get(position + 1)
            .map_or(lines.len(), |(index, _)| *index);
        (start, end)
    };
    match change {
        IniChange::None => {}
        IniChange::Enable(name) => {
            if name.trim().is_empty() || name.contains(['\r', '\n']) {
                return Err(conflict("the cheat name is not a valid settings value"));
            }
            if cheats.len() > 1 {
                return Err(conflict("there is more than one [Cheats] section"));
            }
            if let Some(&position) = cheats.first() {
                let (start, end) = section_range(position);
                let mut enable_cheats: Vec<(usize, String)> = Vec::new();
                let mut enables: Vec<(usize, String)> = Vec::new();
                for index in start + 1..end {
                    if let Some((key, value)) = key_value(&lines[index].text) {
                        match key {
                            "EnableCheats" => enable_cheats.push((index, value.to_string())),
                            "Enable" => enables.push((index, value.to_string())),
                            _ => {}
                        }
                    }
                }
                if enable_cheats.len() > 1 {
                    return Err(conflict("EnableCheats is set more than once"));
                }
                // 1. Rewrite an existing non-true EnableCheats in place (no index shift).
                match enable_cheats.first() {
                    Some((_, value)) if truthy(value) => {
                        edit.enable_cheats = EnableCheatsChange::AlreadyTrue;
                    }
                    Some((index, value)) => {
                        let line_eol = lines[*index].eol;
                        lines[*index] = Line {
                            text: "EnableCheats = true".into(),
                            eol: line_eol,
                        };
                        dropped.insert(*index);
                        edit.enable_cheats = EnableCheatsChange::ChangedToTrue {
                            from: value.clone(),
                        };
                    }
                    None => {}
                }
                // 2. Add the `Enable = name` entry after the last Enable line,
                //    else after EnableCheats, else at the end of the section.
                if !enables.iter().any(|(_, value)| value == name) {
                    let insert_at = if let Some((index, _)) = enables.last() {
                        index + 1
                    } else if let Some((index, _)) = enable_cheats.first() {
                        index + 1
                    } else {
                        let mut at = end;
                        while at > start + 1 && is_blank(&lines[at - 1]) {
                            at -= 1;
                        }
                        at
                    };
                    if insert_at > 0 && lines[insert_at - 1].eol.is_empty() {
                        lines[insert_at - 1].eol = eol;
                    }
                    lines.insert(
                        insert_at,
                        Line {
                            text: format!("Enable = {name}"),
                            eol,
                        },
                    );
                    edit.entries_added.push(name.clone());
                }
                // 3. Create a missing EnableCheats right under the header.
                if enable_cheats.is_empty() {
                    lines.insert(
                        start + 1,
                        Line {
                            text: "EnableCheats = true".into(),
                            eol,
                        },
                    );
                    edit.enable_cheats = EnableCheatsChange::Created;
                }
            } else {
                ensure_terminated(&mut lines, eol);
                if lines.last().is_some_and(|last| !is_blank(last)) {
                    lines.push(Line {
                        text: String::new(),
                        eol,
                    });
                }
                for text in [
                    "[Cheats]".to_string(),
                    "EnableCheats = true".to_string(),
                    format!("Enable = {name}"),
                ] {
                    lines.push(Line { text, eol });
                }
                edit.enable_cheats = EnableCheatsChange::Created;
                edit.entries_added.push(name.clone());
            }
        }
        IniChange::Disable(name) => {
            let mut remove: Vec<usize> = Vec::new();
            for &position in &cheats {
                let (start, end) = section_range(position);
                for index in start + 1..end {
                    if let Some((key, value)) = key_value(&lines[index].text)
                        && key == "Enable"
                        && value == name
                    {
                        remove.push(index);
                    }
                }
            }
            for index in remove.iter().rev() {
                lines.remove(*index);
                dropped.insert(*index);
            }
            if !remove.is_empty() {
                edit.entries_removed.push(name.clone());
            }
        }
    }
    edit.kept = original
        .into_iter()
        .enumerate()
        .filter(|(index, _)| !dropped.contains(index))
        .map(|(_, text)| text)
        .collect();
    edit.after = lines;
    Ok(edit)
}

/// Enables or disables one cheat in a game settings file's text, creating the
/// `[Cheats]` section and `EnableCheats` when enabling. Shared with the legacy
/// `update_duckstation_enablement` helper so there is one INI editor.
pub(super) fn update_game_ini_text(
    input: &str,
    cheat_name: &str,
    enable: bool,
) -> Result<String, String> {
    if input.len() > DUCKSTATION_CHEAT_MAX_BYTES {
        return Err("DuckStation settings file exceeds the bounded size".into());
    }
    if cheat_name.trim().is_empty() || cheat_name.contains(['\r', '\n']) {
        return Err("cheat name is not a valid settings value".into());
    }
    let change = if enable {
        IniChange::Enable(cheat_name.to_string())
    } else {
        IniChange::Disable(cheat_name.to_string())
    };
    edit_game_ini(Some(input), &change, Path::new("settings"))
        .map(|edit| join_lines(&edit.after))
        .map_err(|refusal| refusal.explain())
}

// ---------------------------------------------------------------------------
// Profile folders
// ---------------------------------------------------------------------------

fn resolve_folder(
    data_root: &Path,
    configured: Option<&str>,
    default: &str,
    setting: &str,
) -> Result<(PathBuf, bool), DuckStationNativeRefusal> {
    let unsafe_folder = |reason: &str| DuckStationNativeRefusal::UnsafeCustomFolder {
        setting: setting.to_string(),
        reason: reason.to_string(),
    };
    let (value, custom) = match configured.map(str::trim) {
        Some(value) if !value.is_empty() => (value.to_string(), true),
        _ => (default.to_string(), false),
    };
    if value.contains(['\0', '\r', '\n']) {
        return Err(unsafe_folder("the path contains control characters"));
    }
    let candidate = PathBuf::from(&value);
    if has_parent_dir(&candidate) {
        return Err(unsafe_folder("the path contains `..`"));
    }
    let absolute = candidate.is_absolute();
    let folder = if absolute {
        candidate.clone()
    } else {
        data_root.join(&candidate)
    };
    let depth = folder
        .components()
        .filter(|component| matches!(component, Component::Normal(_)))
        .count();
    if depth < 2 {
        return Err(unsafe_folder(
            "the folder is a filesystem root or a top-level directory",
        ));
    }
    // A relative folder must stay below the data root and may not pass through
    // a symlink; an absolute folder may not itself be a symlink.
    if !absolute {
        let mut walk = data_root.to_path_buf();
        for component in candidate.components() {
            if let Component::Normal(part) = component {
                walk.push(part);
                if let Ok(metadata) = fs::symlink_metadata(&walk)
                    && metadata.file_type().is_symlink()
                {
                    return Err(unsafe_folder("a path component is a symlink"));
                }
            }
        }
    }
    match fs::symlink_metadata(&folder) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(unsafe_folder("the folder is a symlink"));
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(unsafe_folder("the path exists but is not a directory"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(unsafe_folder(&format!("cannot be inspected: {error}"))),
    }
    Ok((folder, custom))
}

/// Resolves DuckStation's Cheats and GameSettings folders from the profile's
/// `settings.ini` (defaults `cheats` and `gamesettings`).
pub fn resolve_duckstation_folders(
    profile_root: &Path,
) -> Result<DuckStationFolders, DuckStationNativeRefusal> {
    let invalid = |reason: String| DuckStationNativeRefusal::InvalidDuckStationProfile { reason };
    if !profile_root.is_absolute() || has_parent_dir(profile_root) {
        return Err(invalid("the profile root must be an absolute path".into()));
    }
    match fs::metadata(profile_root) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(invalid("the profile root is not a directory".into())),
        Err(error) => {
            return Err(invalid(format!("the profile root cannot be read: {error}")));
        }
    }
    let settings_path = profile_root.join("settings.ini");
    let (cheats_value, game_settings_value) = match fs::symlink_metadata(&settings_path) {
        Ok(metadata) if metadata.is_file() => {
            if metadata.len() > SETTINGS_MAX_BYTES {
                return Err(invalid("settings.ini is too large".into()));
            }
            let mut text = String::new();
            fs::File::open(&settings_path)
                .and_then(|file| file.take(SETTINGS_MAX_BYTES).read_to_string(&mut text))
                .map_err(|error| {
                    invalid(format!("settings.ini cannot be read as text: {error}"))
                })?;
            let cheats = ini_value(&text, "Folders", "Cheats").map_err(|reason| invalid(reason))?;
            let game_settings =
                ini_value(&text, "Folders", "GameSettings").map_err(|reason| invalid(reason))?;
            (cheats, game_settings)
        }
        Ok(_) => return Err(invalid("settings.ini is not a regular file".into())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (None, None),
        Err(error) => {
            return Err(invalid(format!(
                "settings.ini cannot be inspected: {error}"
            )));
        }
    };
    let (cheats, cheats_custom) =
        resolve_folder(profile_root, cheats_value.as_deref(), "cheats", "Cheats")?;
    let (game_settings, game_settings_custom) = resolve_folder(
        profile_root,
        game_settings_value.as_deref(),
        "gamesettings",
        "GameSettings",
    )?;
    Ok(DuckStationFolders {
        data_root: profile_root.to_path_buf(),
        cheats,
        game_settings,
        cheats_custom,
        game_settings_custom,
    })
}

// ---------------------------------------------------------------------------
// Identity and file reading
// ---------------------------------------------------------------------------

fn is_ps1_serial(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[..4].iter().all(u8::is_ascii_uppercase)
        && bytes[4] == b'-'
        && bytes[5..].iter().all(u8::is_ascii_digit)
}

fn resolve_serial(serials: &[String]) -> Result<String, DuckStationNativeRefusal> {
    let distinct: BTreeSet<String> = serials
        .iter()
        .map(|serial| serial.trim().to_ascii_uppercase())
        .filter(|serial| !serial.is_empty())
        .collect();
    let valid: Vec<&String> = distinct
        .iter()
        .filter(|serial| is_ps1_serial(serial))
        .collect();
    match (distinct.len(), valid.len()) {
        (0, _) | (_, 0) => Err(DuckStationNativeRefusal::MissingVerifiedSerial),
        (1, 1) => Ok(valid[0].clone()),
        _ => Err(DuckStationNativeRefusal::AmbiguousSerial {
            serials: distinct.into_iter().collect(),
        }),
    }
}

fn read_existing_text(path: &Path) -> Result<Option<(String, Vec<u8>)>, DuckStationNativeRefusal> {
    let changed = |reason: String| DuckStationNativeRefusal::ExistingFileChanged {
        path: path.to_path_buf(),
        reason,
    };
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(changed(format!("cannot be inspected: {error}"))),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(changed("it is not a regular file".into()));
    }
    if metadata.len() > DUCKSTATION_CHEAT_MAX_BYTES as u64 {
        return Err(changed("it is larger than the supported size".into()));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| {
            file.take(DUCKSTATION_CHEAT_MAX_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|error| changed(format!("cannot be read: {error}")))?;
    if bytes.len() > DUCKSTATION_CHEAT_MAX_BYTES {
        return Err(changed("it is larger than the supported size".into()));
    }
    let text =
        String::from_utf8(bytes.clone()).map_err(|_| changed("it is not UTF-8 text".into()))?;
    if text.lines().count() > DUCKSTATION_CHEAT_MAX_LINES {
        return Err(changed("it has more lines than the supported limit".into()));
    }
    Ok(Some((text, bytes)))
}

fn other_serial_files(
    folder: &Path,
    serial: &str,
) -> Result<Vec<String>, DuckStationNativeRefusal> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(DuckStationNativeRefusal::InvalidDuckStationProfile {
                reason: format!("the cheats folder cannot be listed: {error}"),
            });
        }
    };
    let exact = format!("{serial}.cht");
    let prefix = serial.to_ascii_lowercase();
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        if lower.starts_with(&prefix) && lower.ends_with(".cht") && name != exact {
            found.push(name);
        }
    }
    found.sort();
    Ok(found)
}

// ---------------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------------

/// The shared preview wants a destination of exactly `<directory>/<file>` below
/// a root, so each file's transaction is rooted at its folder's parent. That
/// also lets the Cheats and GameSettings folders live anywhere independently.
fn file_plan(
    folder: &Path,
    file_name: &str,
    setting: &str,
    before: Option<Vec<u8>>,
    after: Vec<u8>,
) -> Result<FilePlan, DuckStationNativeRefusal> {
    let unsafe_folder = |reason: &str| DuckStationNativeRefusal::UnsafeCustomFolder {
        setting: setting.to_string(),
        reason: reason.to_string(),
    };
    let root = folder
        .parent()
        .ok_or_else(|| unsafe_folder("the folder has no parent directory"))?;
    let name = folder
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| unsafe_folder("the folder name is not valid text"))?;
    Ok(FilePlan {
        destination: folder.join(file_name),
        root: root.to_path_buf(),
        relative: format!("{name}/{file_name}"),
        before_sha256: before.as_deref().map(sha256_hex),
        before,
        after_sha256: sha256_hex(&after),
        after,
    })
}

/// Stages the exact bytes in a private directory and builds the shared
/// single-file transaction plan for them. The shared preview needs the staged
/// source to exist, so this happens at apply time, never during preview.
fn stage_and_plan(
    file: &FilePlan,
    serial: &str,
    selected_game: &Path,
    staging_root: &Path,
    profile_label: &'static str,
) -> Result<SharedTransactionPlan, DuckStationNativeRefusal> {
    let blocked = |reason: String| DuckStationNativeRefusal::ExistingFileChanged {
        path: file.destination.clone(),
        reason,
    };
    let staged = staging_root.join(&file.relative);
    fs::create_dir_all(staged.parent().unwrap_or(staging_root))
        .and_then(|()| fs::write(&staged, &file.after))
        .map_err(|error| blocked(format!("the output could not be staged: {error}")))?;
    let preview = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::DuckStation,
        selected_archive: selected_game.to_path_buf(),
        platform: Some("ps1".into()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::DuckStationSerial,
            state: PreviewIdentityState::Verified,
            value: Some(serial.to_string()),
            archive_path: selected_game.to_path_buf(),
            revision: None,
        },
        destination_root: file.root.clone(),
        source_items: vec![PreviewSourceItem {
            adapter: PreviewAdapter::DuckStation,
            source_path: staged.clone(),
            expected_source_digest: Some(file.after_sha256.clone()),
            destination_relative_paths: vec![PathBuf::from(&file.relative)],
            match_strength: PreviewMatchStrength::VerifiedExact,
        }],
    })
    .map_err(|error| blocked(format!("the shared preview was refused: {error:?}")))?;
    if let Some(blocker) = preview
        .entries
        .iter()
        .find_map(|entry| entry.blockers.first())
    {
        return Err(blocked(format!("the destination is blocked: {blocker:?}")));
    }
    build_shared_transaction_plan(&preview, profile_label, "duckstation_native", staging_root)
        .map_err(|error| blocked(format!("the transaction plan was refused: {error:?}")))
}

/// Builds a preview and the exact staged outputs. Reads the profile, the cheats
/// folder and the two destination files; writes nothing.
pub fn plan_duckstation_native(
    request: &DuckStationNativeRequest,
) -> Result<DuckStationNativePlan, DuckStationNativeRefusal> {
    // IDENTIFY
    let serial = resolve_serial(&request.verified_serials)?;
    match request.disc_topology {
        DuckStationDiscTopology::SingleDisc => {}
        DuckStationDiscTopology::MultiDisc => {
            return Err(DuckStationNativeRefusal::UnsupportedMultiDisc);
        }
        DuckStationDiscTopology::Unknown => {
            return Err(DuckStationNativeRefusal::DiscTopologyUnproven);
        }
    }
    // PREFLIGHT
    let folders = resolve_duckstation_folders(&request.profile_root)?;
    let variants = other_serial_files(&folders.cheats, &serial)?;
    if !variants.is_empty() {
        return Err(DuckStationNativeRefusal::HashSpecificVariantPresent { files: variants });
    }
    let cht_name = format!("{serial}.cht");
    let ini_name = format!("{serial}.ini");
    let cht_path = folders.cheats.join(&cht_name);
    let ini_path = folders.game_settings.join(&ini_name);
    let cht_before = read_existing_text(&cht_path)?;
    let ini_before = read_existing_text(&ini_path)?;
    let cht_text = cht_before.as_ref().map(|(text, _)| text.as_str());
    let ini_text = ini_before.as_ref().map(|(text, _)| text.as_str());

    // Database shadowing only matters when we are about to write a cheat body.
    let writes_cheat_body = matches!(
        request.operation,
        DuckStationNativeOperation::Add { .. } | DuckStationNativeOperation::Update { .. }
    );
    let mut warnings = vec![DuckStationNativeWarning::ReloadRequired];
    if writes_cheat_body {
        let database_disabled = match ini_text {
            Some(text) => ini_value(text, "Cheats", "LoadCheatsFromDatabase")
                .map_err(|reason| DuckStationNativeRefusal::IniUpdateConflict { reason })?
                .is_some_and(|value| falsy(&value)),
            None => false,
        };
        if !database_disabled {
            if request.acknowledge_database_shadowing {
                warnings.push(DuckStationNativeWarning::DatabaseShadowingAcknowledged);
            } else {
                return Err(DuckStationNativeRefusal::DatabaseShadowingUnknown);
            }
        }
    }

    // STAGE: edit both files in memory.
    let cht_edit = edit_cht(cht_text, &request.operation, &cht_path)?;
    let ini_change = match &request.operation {
        DuckStationNativeOperation::Add { cheat, enable } => {
            if *enable {
                IniChange::Enable(cheat.name.clone())
            } else {
                IniChange::None
            }
        }
        DuckStationNativeOperation::Update { .. } => IniChange::None,
        DuckStationNativeOperation::Remove { name } => IniChange::Disable(name.clone()),
        DuckStationNativeOperation::SetEnabled { name, enabled } => {
            if *enabled {
                IniChange::Enable(name.clone())
            } else {
                IniChange::Disable(name.clone())
            }
        }
    };
    let ini_edit = edit_game_ini(ini_text, &ini_change, &ini_path)?;

    let cht_after_text = join_lines(&cht_edit.after);
    let ini_after_text = join_lines(&ini_edit.after);
    let cht_before_bytes = cht_before.as_ref().map(|(_, bytes)| bytes.clone());
    let ini_before_bytes = ini_before.as_ref().map(|(_, bytes)| bytes.clone());
    // An absent file that would stay empty is not a change.
    let cht_will_change = match &cht_before_bytes {
        Some(bytes) => bytes.as_slice() != cht_after_text.as_bytes(),
        None => !cht_after_text.is_empty(),
    };
    let ini_will_change = match &ini_before_bytes {
        Some(bytes) => bytes.as_slice() != ini_after_text.as_bytes(),
        None => !ini_after_text.is_empty(),
    };

    // Semantic expectations, used to verify both the staged and published bytes.
    let before_lines = cht_text.map(split_lines).unwrap_or_default();
    let before_sections = cht_sections(&before_lines);
    let touched: BTreeSet<&str> = cht_edit
        .added
        .iter()
        .chain(&cht_edit.changed)
        .chain(&cht_edit.removed)
        .map(String::as_str)
        .collect();
    let preserved_sections: Vec<(String, String)> = before_sections
        .iter()
        .filter(|section| !touched.contains(section.name.as_str()))
        .map(|section| {
            (
                section.name.clone(),
                section_digest_of(&before_lines[section.start..section.end]),
            )
        })
        .collect();
    let after_sections = cht_sections(&cht_edit.after);
    let present_sections: Vec<(String, String)> = cht_edit
        .added
        .iter()
        .chain(&cht_edit.changed)
        .filter_map(|name| {
            after_sections
                .iter()
                .find(|section| &section.name == name)
                .map(|section| {
                    (
                        name.clone(),
                        section_digest_of(&cht_edit.after[section.start..section.end]),
                    )
                })
        })
        .collect();
    let (enable_present, enable_absent, enable_cheats_true) = match &ini_change {
        IniChange::Enable(name) => (vec![name.clone()], Vec::new(), true),
        IniChange::Disable(name) => (Vec::new(), vec![name.clone()], false),
        IniChange::None => (Vec::new(), Vec::new(), false),
    };
    let preamble: Vec<String> = before_lines
        .iter()
        .take(
            before_sections
                .first()
                .map_or(before_lines.len(), |s| s.start),
        )
        .map(|line| line.text.clone())
        .collect();
    let expectations = Expectations {
        preamble,
        ini_kept: ini_edit.kept.clone(),
        present_sections,
        absent_sections: cht_edit.removed.clone(),
        preserved_sections,
        enable_cheats_true,
        enable_present,
        enable_absent,
    };
    // VERIFY STAGED OUTPUT
    verify_state(
        &expectations,
        Some(&cht_after_text),
        Some(&ini_after_text),
        &cht_path,
        &ini_path,
    )?;

    let no_op = !cht_will_change && !ini_will_change;
    let cht_plan = if cht_will_change {
        Some(file_plan(
            &folders.cheats,
            &cht_name,
            "Cheats",
            cht_before_bytes.clone(),
            cht_after_text.clone().into_bytes(),
        )?)
    } else {
        None
    };
    let ini_plan = if ini_will_change {
        Some(file_plan(
            &folders.game_settings,
            &ini_name,
            "GameSettings",
            ini_before_bytes.clone(),
            ini_after_text.clone().into_bytes(),
        )?)
    } else {
        None
    };
    let preview = DuckStationNativePreview {
        serial,
        cht_path: cht_path.clone(),
        game_ini_path: ini_path.clone(),
        folders,
        cheats_added: cht_edit.added,
        cheats_changed: cht_edit.changed,
        cheats_removed: cht_edit.removed,
        enable_cheats: if ini_will_change || ini_change != IniChange::None {
            ini_edit.enable_cheats
        } else {
            EnableCheatsChange::NotRequired
        },
        enable_entries_added: ini_edit.entries_added,
        enable_entries_removed: ini_edit.entries_removed,
        warnings,
        cht_exists: cht_before_bytes.is_some(),
        game_ini_exists: ini_before_bytes.is_some(),
        cht_will_change,
        game_ini_will_change: ini_will_change,
        backup_will_be_made: (cht_will_change && cht_before_bytes.is_some())
            || (ini_will_change && ini_before_bytes.is_some()),
        undo_available: !no_op,
        no_op,
        cht_before_sha256: cht_before_bytes.as_deref().map(sha256_hex),
        cht_after_sha256: cht_will_change.then(|| sha256_hex(cht_after_text.as_bytes())),
        game_ini_before_sha256: ini_before_bytes.as_deref().map(sha256_hex),
        game_ini_after_sha256: ini_will_change.then(|| sha256_hex(ini_after_text.as_bytes())),
    };
    Ok(DuckStationNativePlan {
        preview,
        cht: cht_plan,
        ini: ini_plan,
        cht_final: cht_after_text.into_bytes(),
        ini_final: ini_after_text.into_bytes(),
        expectations,
        cht_destination: cht_path,
        ini_destination: ini_path,
        selected_game: request.selected_game.clone(),
    })
}

/// Checks `.cht`/INI text against what the plan promised: intended sections
/// exist exactly as built, removed ones are gone, every other section is byte
/// for byte (canonically) unchanged, and the INI enablement is as required.
fn verify_state(
    expectations: &Expectations,
    cht_text: Option<&str>,
    ini_text: Option<&str>,
    cht_path: &Path,
    ini_path: &Path,
) -> Result<(), DuckStationNativeRefusal> {
    let failed = |path: &Path, reason: String| DuckStationNativeRefusal::ExistingFileChanged {
        path: path.to_path_buf(),
        reason,
    };
    let lines = cht_text.map(split_lines).unwrap_or_default();
    let sections = cht_sections(&lines);
    for (name, digest) in &expectations.present_sections {
        let matching: Vec<_> = sections.iter().filter(|s| &s.name == name).collect();
        let [section] = matching.as_slice() else {
            return Err(failed(
                cht_path,
                format!("\"{name}\" is not present exactly once after the change"),
            ));
        };
        if section_digest_of(&lines[section.start..section.end]) != *digest {
            return Err(failed(
                cht_path,
                format!("\"{name}\" does not match the plan"),
            ));
        }
    }
    for name in &expectations.absent_sections {
        if sections.iter().any(|section| &section.name == name) {
            return Err(failed(cht_path, format!("\"{name}\" was not removed")));
        }
    }
    for (name, digest) in &expectations.preserved_sections {
        let matching: Vec<_> = sections.iter().filter(|s| &s.name == name).collect();
        let [section] = matching.as_slice() else {
            return Err(failed(
                cht_path,
                format!("unrelated section \"{name}\" was lost"),
            ));
        };
        if section_digest_of(&lines[section.start..section.end]) != *digest {
            return Err(failed(
                cht_path,
                format!("unrelated section \"{name}\" changed"),
            ));
        }
    }
    let preamble_len = expectations.preamble.len();
    if lines.len() < preamble_len
        || lines[..preamble_len]
            .iter()
            .map(|line| line.text.as_str())
            .ne(expectations.preamble.iter().map(String::as_str))
    {
        return Err(failed(
            cht_path,
            "the lines before the first cheat changed".into(),
        ));
    }
    let ini = ini_text.unwrap_or_default();
    let ini_lines = split_lines(ini);
    {
        let mut remaining = ini_lines.iter().map(|line| line.text.as_str());
        for kept in &expectations.ini_kept {
            if !remaining.any(|text| text == kept) {
                return Err(failed(
                    ini_path,
                    format!("an unrelated settings line was lost or reordered: {kept}"),
                ));
            }
        }
    }
    let mut in_cheats = false;
    let mut enable_cheats: Option<String> = None;
    let mut enables: Vec<String> = Vec::new();
    for line in &ini_lines {
        if let Some(name) = header_name(&line.text)
            && !is_comment(&line.text)
        {
            in_cheats = name == "Cheats";
            continue;
        }
        if in_cheats && let Some((key, value)) = key_value(&line.text) {
            match key {
                "EnableCheats" => enable_cheats = Some(value.to_string()),
                "Enable" => enables.push(value.to_string()),
                _ => {}
            }
        }
    }
    if expectations.enable_cheats_true && !enable_cheats.as_deref().is_some_and(truthy) {
        return Err(failed(ini_path, "EnableCheats is not true".into()));
    }
    for name in &expectations.enable_present {
        if enables.iter().filter(|value| *value == name).count() != 1 {
            return Err(failed(
                ini_path,
                format!("\"{name}\" is not enabled exactly once"),
            ));
        }
    }
    for name in &expectations.enable_absent {
        if enables.iter().any(|value| value == name) {
            return Err(failed(ini_path, format!("\"{name}\" is still enabled")));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Apply, receipt and undo
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct DuckStationNativeApplyOptions {
    pub general_approved: bool,
    pub replacement_approved: bool,
    pub operation_id: String,
    pub timestamp_unix_seconds: u64,
    pub history_root: PathBuf,
    pub backup_root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuckStationNativeFileReceipt {
    pub destination_root: PathBuf,
    pub relative_path: String,
    pub before_sha256: Option<String>,
    pub after_sha256: String,
    pub operation_id: String,
    pub journal_path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuckStationNativeReceipt {
    pub schema_version: u32,
    pub operation_id: String,
    pub serial: String,
    pub timestamp_unix_seconds: u64,
    pub no_op: bool,
    pub cheats_added: Vec<String>,
    pub cheats_changed: Vec<String>,
    pub cheats_removed: Vec<String>,
    pub cht: Option<DuckStationNativeFileReceipt>,
    pub game_ini: Option<DuckStationNativeFileReceipt>,
    /// The preview's warnings, so an acknowledged database-shadowing risk is on
    /// the record with the change it applied to.
    #[serde(default)]
    pub warnings: Vec<DuckStationNativeWarning>,
    /// Where this receipt was written (not part of the stored JSON's meaning).
    #[serde(default)]
    pub receipt_path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DuckStationNativeApplyFailure {
    /// Nothing was written.
    Refused(DuckStationNativeRefusal),
    /// A step failed after publication began; every published change was rolled
    /// back and verified restored.
    PublishFailedRolledBack { stage: &'static str, detail: String },
    /// A step failed and the rollback could not be completed. The files may
    /// disagree and need attention.
    RollbackIncomplete { stage: &'static str, detail: String },
}

/// Deterministic, test-only failure points for the apply path. They exist only
/// in test builds; a normal build has no way to trigger them.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TestFault {
    /// Rewrite `path` with `content` after the `.cht` is published and before
    /// the INI is, as a concurrent editor would.
    MutateAfterFirstPublish { path: PathBuf, content: String },
    /// Rewrite `path` with `content` after both files are published and
    /// before they are verified.
    TamperAfterPublish { path: PathBuf, content: String },
    /// Make post-publication verification fail without touching any file.
    ForceVerifyFailure,
    /// Make the receipt write fail.
    FailReceiptWrite,
}

#[cfg(test)]
thread_local! {
    static TEST_FAULT: std::cell::RefCell<Option<TestFault>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn set_test_fault(fault: Option<TestFault>) {
    TEST_FAULT.with(|slot| *slot.borrow_mut() = fault);
}

#[cfg(test)]
fn test_fault() -> Option<TestFault> {
    TEST_FAULT.with(|slot| slot.borrow().clone())
}

fn read_bytes_if_exists(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err("not a regular file".into())
        }
        Ok(_) => fs::read(path).map(Some).map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn shared_options(
    plan: &SharedTransactionPlan,
    options: &DuckStationNativeApplyOptions,
    operation_id: String,
) -> SharedApplyOptions {
    SharedApplyOptions {
        dry_run: false,
        confirmation: Some(SharedApplyConfirmation {
            plan_id: plan.plan_id.clone(),
            general_approved: options.general_approved,
            replacement_approved: options.replacement_approved,
        }),
        operation_id,
        timestamp_unix_seconds: options.timestamp_unix_seconds,
        current_context: plan.context.clone(),
        history_root: options.history_root.clone(),
        backup_root: options.backup_root.clone(),
    }
}

fn publish(
    file: &FilePlan,
    shared: &SharedTransactionPlan,
    options: &DuckStationNativeApplyOptions,
    operation_id: String,
) -> (SharedApplyStatus, Option<PathBuf>, String) {
    let digest = file.after_sha256.clone();
    let source_path = shared
        .entries
        .first()
        .and_then(|entry| entry.source_path.to_path_buf().ok())
        .unwrap_or_default();
    let result = execute_shared_materialized_apply(
        shared,
        &shared_options(shared, options, operation_id),
        &SharedMaterializedOutput {
            source_path,
            source_digest: digest.clone(),
            output_digest: digest,
            bytes: file.after.clone(),
            guard_paths: Vec::new(),
        },
    );
    let detail = result
        .journal
        .entries
        .iter()
        .map(|entry| format!("{:?}: {:?}", entry.outcome, entry.failures))
        .collect::<Vec<_>>()
        .join("; ");
    (result.journal.status, result.journal_path, detail)
}

fn roll_back(
    journal_path: &Path,
    root: &Path,
    options: &DuckStationNativeApplyOptions,
    rollback_operation_id: String,
) -> Result<(), String> {
    let preview = preview_shared_rollback(journal_path, root, &options.backup_root);
    if !preview.available {
        return Err("the shared rollback preview reports it is not available".into());
    }
    let result = execute_shared_rollback(
        &preview,
        &SharedRollbackOptions {
            confirmation: SharedRollbackConfirmation {
                preview_id: preview.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id,
            timestamp_unix_seconds: options.timestamp_unix_seconds,
            history_root: options.history_root.clone(),
            backup_root: options.backup_root.clone(),
        },
    );
    if result.status == SharedApplyStatus::Success {
        Ok(())
    } else {
        Err(format!("the rollback finished as {:?}", result.status))
    }
}

/// Whether the destination now holds exactly the bytes this operation would
/// have written (and that differ from what was there before).
fn holds_our_output(file: &FilePlan) -> bool {
    file.before_sha256.as_deref() != Some(file.after_sha256.as_str())
        && matches!(
            read_bytes_if_exists(&file.destination),
            Ok(Some(current)) if sha256_hex(&current) == file.after_sha256
        )
}

fn restored(file: &FilePlan) -> bool {
    match read_bytes_if_exists(&file.destination) {
        Ok(current) => current.as_deref().map(sha256_hex) == file.before_sha256,
        Err(_) => false,
    }
}

/// Publishes the plan: re-checks both destinations, re-verifies the staged
/// outputs, publishes `.cht` then INI, verifies both from disk and writes a
/// receipt. On any failure after the first publication it rolls back.
pub fn apply_duckstation_native_plan(
    plan: &DuckStationNativePlan,
    options: &DuckStationNativeApplyOptions,
) -> Result<DuckStationNativeReceipt, DuckStationNativeApplyFailure> {
    use DuckStationNativeApplyFailure::{PublishFailedRolledBack, Refused, RollbackIncomplete};
    if !options.general_approved {
        return Err(Refused(DuckStationNativeRefusal::ConfirmationRequired));
    }
    if !safe_identifier(&options.operation_id) {
        return Err(Refused(DuckStationNativeRefusal::InvalidOperationId));
    }
    let preview = &plan.preview;
    let mut receipt = DuckStationNativeReceipt {
        schema_version: RECEIPT_SCHEMA_VERSION,
        operation_id: options.operation_id.clone(),
        serial: preview.serial.clone(),
        timestamp_unix_seconds: options.timestamp_unix_seconds,
        no_op: preview.no_op,
        cheats_added: preview.cheats_added.clone(),
        cheats_changed: preview.cheats_changed.clone(),
        cheats_removed: preview.cheats_removed.clone(),
        cht: None,
        game_ini: None,
        warnings: preview.warnings.clone(),
        receipt_path: None,
    };
    if preview.no_op {
        return Ok(receipt);
    }
    // Stage the exact bytes in a private temporary directory (removed on every
    // exit) and build the shared plans. It must sit outside the managed
    // history/backup roots and both destination folders.
    let staging = tempfile::Builder::new()
        .prefix("emuwiz-duckstation-")
        .tempdir()
        .map_err(|error| {
            Refused(DuckStationNativeRefusal::ExistingFileChanged {
                path: plan.cht_destination.clone(),
                reason: format!("a private staging directory could not be created: {error}"),
            })
        })?;
    let staging_root = staging.path().to_path_buf();
    let mut shared_cht = None;
    let mut shared_ini = None;
    if let Some(file) = &plan.cht {
        shared_cht = Some(
            stage_and_plan(
                file,
                &preview.serial,
                &plan.selected_game,
                &staging_root.join("cht"),
                "duckstation-native-cheats",
            )
            .map_err(Refused)?,
        );
    }
    if let Some(file) = &plan.ini {
        shared_ini = Some(
            stage_and_plan(
                file,
                &preview.serial,
                &plan.selected_game,
                &staging_root.join("ini"),
                "duckstation-native-settings",
            )
            .map_err(Refused)?,
        );
    }
    // PREFLIGHT: both destinations must still be exactly what was previewed.
    for (destination, expected_before) in [
        (&plan.cht_destination, &preview.cht_before_sha256),
        (&plan.ini_destination, &preview.game_ini_before_sha256),
    ] {
        let current = read_bytes_if_exists(destination).map_err(|_| {
            Refused(DuckStationNativeRefusal::DestinationChangedAfterPreview {
                path: destination.clone(),
            })
        })?;
        if current.as_deref().map(sha256_hex) != *expected_before {
            return Err(Refused(
                DuckStationNativeRefusal::DestinationChangedAfterPreview {
                    path: destination.clone(),
                },
            ));
        }
    }
    // VERIFY STAGED OUTPUT again, from the bytes that will actually be written.
    let staged_cht = String::from_utf8_lossy(&plan.cht_final).into_owned();
    let staged_ini = String::from_utf8_lossy(&plan.ini_final).into_owned();
    verify_state(
        &plan.expectations,
        Some(&staged_cht),
        Some(&staged_ini),
        &plan.cht_destination,
        &plan.ini_destination,
    )
    .map_err(Refused)?;

    // PUBLISH: .cht first (inert without the INI), then the INI.
    let mut published: Vec<(&FilePlan, PathBuf, String)> = Vec::new();
    let mut failure: Option<(&'static str, String)> = None;
    // The file whose publication itself failed. It was never published by this
    // operation, so rollback only has to prove it does not hold our output.
    let mut failed_file: Option<&FilePlan> = None;
    if let (Some(file), Some(shared)) = (&plan.cht, &shared_cht) {
        let operation_id = format!("{}-cht", options.operation_id);
        let (status, journal, detail) = publish(file, shared, options, operation_id.clone());
        match (status, journal) {
            (SharedApplyStatus::Success, Some(journal)) => {
                receipt.cht = Some(DuckStationNativeFileReceipt {
                    destination_root: file.root.clone(),
                    relative_path: file.relative.clone(),
                    before_sha256: file.before_sha256.clone(),
                    after_sha256: file.after_sha256.clone(),
                    operation_id: operation_id.clone(),
                    journal_path: journal.clone(),
                });
                published.push((file, journal, operation_id));
                #[cfg(test)]
                if let Some(TestFault::MutateAfterFirstPublish { path, content }) = test_fault() {
                    let _ = fs::write(path, content);
                }
            }
            (_, journal) => {
                // A partial first publication is rolled back by the shared layer
                // when it can; make sure the file is as it was.
                if let Some(journal) = journal
                    && !restored(file)
                {
                    let _ = roll_back(
                        &journal,
                        &file.root,
                        options,
                        format!("{operation_id}-rollback"),
                    );
                }
                failed_file = Some(file);
                failure = Some(("publish cheat file", format!("status {status:?}: {detail}")));
            }
        }
    }
    if failure.is_none()
        && let (Some(file), Some(shared)) = (&plan.ini, &shared_ini)
    {
        let operation_id = format!("{}-ini", options.operation_id);
        let (status, journal, detail) = publish(file, shared, options, operation_id.clone());
        match (status, journal) {
            (SharedApplyStatus::Success, Some(journal)) => {
                receipt.game_ini = Some(DuckStationNativeFileReceipt {
                    destination_root: file.root.clone(),
                    relative_path: file.relative.clone(),
                    before_sha256: file.before_sha256.clone(),
                    after_sha256: file.after_sha256.clone(),
                    operation_id: operation_id.clone(),
                    journal_path: journal.clone(),
                });
                published.push((file, journal, operation_id));
            }
            (_, journal) => {
                if let Some(journal) = journal
                    && !restored(file)
                {
                    let _ = roll_back(
                        &journal,
                        &file.root,
                        options,
                        format!("{operation_id}-rollback"),
                    );
                }
                failure = Some((
                    "publish settings file",
                    format!("status {status:?}: {detail}"),
                ));
            }
        }
    }
    #[cfg(test)]
    if failure.is_none()
        && let Some(TestFault::TamperAfterPublish { path, content }) = test_fault()
    {
        let _ = fs::write(path, content);
    }
    // VERIFY PUBLISHED OUTPUT.
    if failure.is_none() {
        let verify = || -> Result<(), String> {
            #[cfg(test)]
            if test_fault() == Some(TestFault::ForceVerifyFailure) {
                return Err("injected verification failure".into());
            }
            let cht = read_bytes_if_exists(&plan.cht_destination)?;
            let ini = read_bytes_if_exists(&plan.ini_destination)?;
            if preview.cht_will_change && cht.as_deref() != Some(plan.cht_final.as_slice()) {
                return Err("the published cheat file differs from the plan".into());
            }
            if preview.game_ini_will_change && ini.as_deref() != Some(plan.ini_final.as_slice()) {
                return Err("the published settings file differs from the plan".into());
            }
            let cht_text = cht.map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
            let ini_text = ini.map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
            verify_state(
                &plan.expectations,
                cht_text.as_deref(),
                ini_text.as_deref(),
                &plan.cht_destination,
                &plan.ini_destination,
            )
            .map_err(|refusal| refusal.explain())
        };
        if let Err(detail) = verify() {
            failure = Some(("verify published files", detail));
        }
    }
    // RECEIPT.
    if failure.is_none() {
        match write_receipt(&mut receipt, options) {
            Ok(()) => return Ok(receipt),
            Err(detail) => failure = Some(("write receipt", detail)),
        }
    }
    // Roll back whatever was published, newest first.
    let (stage, detail) = failure.expect("a failure is recorded before rolling back");
    let mut incomplete: Option<String> = None;
    for (file, journal, operation_id) in published.iter().rev() {
        if let Err(error) = roll_back(
            journal,
            &file.root,
            options,
            format!("{operation_id}-rollback"),
        ) {
            incomplete = Some(error);
        }
    }
    // Every file this operation published must be back to its previous content;
    // the file whose publication failed was never ours, so it only must not hold
    // our output (someone else may have legitimately changed it meanwhile).
    let published_restored = published.iter().all(|(file, _, _)| restored(file));
    let failed_clean = failed_file.is_none_or(|file| !holds_our_output(file));
    let all_restored = published_restored && failed_clean;
    match (incomplete, all_restored) {
        (None, true) => Err(PublishFailedRolledBack { stage, detail }),
        (error, _) => Err(RollbackIncomplete {
            stage,
            detail: format!(
                "{detail}; rollback: {}",
                error.unwrap_or_else(|| "files did not return to their previous content".into())
            ),
        }),
    }
}

fn write_receipt(
    receipt: &mut DuckStationNativeReceipt,
    options: &DuckStationNativeApplyOptions,
) -> Result<(), String> {
    #[cfg(test)]
    if test_fault() == Some(TestFault::FailReceiptWrite) {
        return Err("injected receipt failure".into());
    }
    fs::create_dir_all(&options.history_root).map_err(|error| error.to_string())?;
    let path = options.history_root.join(format!(
        "duckstation-native-{}.receipt.json",
        receipt.operation_id
    ));
    receipt.receipt_path = Some(path.clone());
    let bytes = serde_json::to_vec_pretty(&*receipt).map_err(|error| error.to_string())?;
    let temporary = options.history_root.join(format!(
        ".duckstation-native-{}.receipt.tmp",
        receipt.operation_id
    ));
    fs::write(&temporary, &bytes).map_err(|error| error.to_string())?;
    fs::rename(&temporary, &path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        error.to_string()
    })
}

/// Loads a receipt written by [`apply_duckstation_native_plan`].
pub fn read_duckstation_native_receipt(path: &Path) -> Result<DuckStationNativeReceipt, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > RECEIPT_MAX_BYTES {
        return Err("the receipt is too large".into());
    }
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let mut receipt: DuckStationNativeReceipt =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if receipt.schema_version != RECEIPT_SCHEMA_VERSION {
        return Err("the receipt schema is not supported".into());
    }
    receipt.receipt_path = Some(path.to_path_buf());
    Ok(receipt)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuckStationNativeUndoPreview {
    /// Both sides can be restored right now.
    pub available: bool,
    /// Why not, when unavailable (a destination changed since apply, a backup
    /// is gone, or it was already undone).
    pub reasons: Vec<String>,
}

fn undo_sides(receipt: &DuckStationNativeReceipt) -> Vec<&DuckStationNativeFileReceipt> {
    // INI first: a partial undo then leaves an inert `.cht`, never an INI that
    // enables a cheat that is gone.
    [receipt.game_ini.as_ref(), receipt.cht.as_ref()]
        .into_iter()
        .flatten()
        .collect()
}

#[must_use]
pub fn preview_duckstation_native_undo(
    receipt: &DuckStationNativeReceipt,
    backup_root: &Path,
) -> DuckStationNativeUndoPreview {
    let mut reasons = Vec::new();
    if receipt.no_op || (receipt.cht.is_none() && receipt.game_ini.is_none()) {
        reasons.push("this operation changed nothing, so there is nothing to undo".into());
    }
    for side in undo_sides(receipt) {
        let preview =
            preview_shared_rollback(&side.journal_path, &side.destination_root, backup_root);
        if !preview.available {
            reasons.push(format!(
                "{} cannot be restored (it changed since the cheat was applied, was already undone, or its backup is missing)",
                side.relative_path
            ));
        }
    }
    DuckStationNativeUndoPreview {
        available: reasons.is_empty(),
        reasons,
    }
}

/// Restores both files from their journals, INI first, then verifies each is
/// byte-identical to its pre-apply content (or absent if it did not exist).
pub fn undo_duckstation_native(
    receipt: &DuckStationNativeReceipt,
    options: &DuckStationNativeApplyOptions,
) -> Result<(), DuckStationNativeApplyFailure> {
    use DuckStationNativeApplyFailure::{Refused, RollbackIncomplete};
    if !options.general_approved {
        return Err(Refused(DuckStationNativeRefusal::ConfirmationRequired));
    }
    if !safe_identifier(&options.operation_id) {
        return Err(Refused(DuckStationNativeRefusal::InvalidOperationId));
    }
    let preview = preview_duckstation_native_undo(receipt, &options.backup_root);
    if !preview.available {
        return Err(RollbackIncomplete {
            stage: "undo preview",
            detail: preview.reasons.join("; "),
        });
    }
    for side in undo_sides(receipt) {
        roll_back(
            &side.journal_path,
            &side.destination_root,
            options,
            format!(
                "{}-{}",
                options.operation_id,
                side.relative_path.replace('.', "_")
            ),
        )
        .map_err(|detail| RollbackIncomplete {
            stage: "undo",
            detail: format!("{}: {detail}", side.relative_path),
        })?;
        let current = read_bytes_if_exists(&side.destination_root.join(&side.relative_path))
            .map_err(|detail| RollbackIncomplete {
                stage: "verify undo",
                detail,
            })?;
        if current.as_deref().map(sha256_hex) != side.before_sha256 {
            return Err(RollbackIncomplete {
                stage: "verify undo",
                detail: format!(
                    "{} did not return to its previous content",
                    side.relative_path
                ),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
