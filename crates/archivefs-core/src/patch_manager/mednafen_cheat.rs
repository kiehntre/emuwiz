//! Conservative native Mednafen `.cht` support.
//!
//! Mednafen's current cheat file is a per-system text file containing
//! MD5-keyed game sections.  The parser keeps the section identity separate
//! from display titles and never treats an unverified title as permission to
//! write a system-wide cheat file.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::cheat_compatibility::{
    CheatCompatibilityEntry, CheatCompatibilityReport, CheatMasterCodeRequirement,
    CheatRevisionEvidence, analyze_cheat_stack,
};
use super::cheat_ir::{CheatOperation, CheatPlatform};
use super::shared_preview::{
    PreviewAdapter, PreviewIdentity, PreviewIdentityKind, PreviewIdentityState,
    PreviewMatchStrength, PreviewSourceItem, SharedPreviewReport, SharedPreviewRequest,
    build_shared_preview,
};
use super::shared_transaction::{
    SharedApplyOptions, SharedApplyResult, SharedRollbackOptions, SharedRollbackResult,
    SharedTransactionPlan, build_shared_transaction_plan, execute_shared_apply,
    execute_shared_rollback, preview_shared_rollback,
};

pub const MEDNAFEN_CHEAT_SOURCE_MODE: &str = "mednafen_native_cheat";
pub const MEDNAFEN_CHEAT_MAX_BYTES: usize = 4 * 1024 * 1024;
pub const MEDNAFEN_CHEAT_MAX_LINES: usize = 32_768;
pub const MEDNAFEN_CHEAT_MAX_ENTRIES: usize = 8_192;
pub const MEDNAFEN_CHEAT_MAX_LINE_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MednafenCheatFile {
    pub system: String,
    pub entries: Vec<MednafenCheatEntry>,
    pub comments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MednafenCheatEntry {
    pub target: MednafenCheatTarget,
    pub name: String,
    pub state: MednafenCheatState,
    pub operations: Vec<MednafenCheatOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MednafenCheatTarget {
    pub system: String,
    pub md5: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MednafenCheatOperation {
    Direct {
        kind: char,
        memory_space: MednafenMemorySpace,
        address: u64,
        width_bytes: u8,
        value: u64,
        endian: MednafenEndian,
        normalized: Option<CheatOperation>,
        raw: String,
    },
    Conditional {
        kind: char,
        memory_space: MednafenMemorySpace,
        address: u64,
        width_bytes: u8,
        value: u64,
        compare: u64,
        endian: MednafenEndian,
        raw: String,
    },
    Opaque {
        raw: String,
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MednafenCheatState {
    Enabled,
    Disabled,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MednafenMemorySpace {
    SystemMemory,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MednafenEndian {
    Big,
    Little,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MednafenCheatParseIssue {
    Empty,
    TooLarge,
    TooManyLines,
    TooManyEntries,
    LineTooLong(usize),
    MissingTarget(usize),
    MalformedTarget(usize),
    MalformedOperation(usize),
    UnsupportedWidth(usize),
    UnsupportedKind(usize),
    InvalidState(usize),
    InvalidNumber(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MednafenCheatReadiness {
    Ready,
    ReadyWithOpaqueNativeOps,
    WrongIdentity,
    TitleOnly,
    UnsupportedSystem,
    PreviewOnly,
    Malformed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MednafenCheatPlan {
    pub report: SharedPreviewReport,
    pub transaction: SharedTransactionPlan,
    pub readiness: MednafenCheatReadiness,
    pub output_bytes: Vec<u8>,
    pub destination: PathBuf,
    pub system: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MednafenLoadabilityFacts {
    pub system: String,
    pub cheat_path: PathBuf,
    pub game_md5: String,
    pub persistent_state: bool,
    pub restart_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MednafenCheatError {
    Parse(MednafenCheatParseIssue),
    IdentityRequired,
    IdentityMismatch,
    UnsupportedSystem(String),
    UnsafePath,
    Preview(String),
}

impl std::fmt::Display for MednafenCheatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for MednafenCheatError {}

const SUPPORTED_SYSTEMS: &[&str] = &[
    "gb",
    "gg",
    "lynx",
    "md",
    "nes",
    "pce",
    "pce_fast",
    "pcfx",
    "psx",
    "sms",
    "snes",
    "snes_faust",
    "vb",
    "wswan",
];

pub fn mednafen_supported_systems() -> &'static [&'static str] {
    SUPPORTED_SYSTEMS
}

pub fn mednafen_system_for_platform(platform: &str) -> Option<&'static str> {
    let normalized = platform.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "game boy" | "gameboy" | "game boy color" | "gb" | "gbc" => Some("gb"),
        "game gear" | "gg" => Some("gg"),
        "atari lynx" | "lynx" => Some("lynx"),
        "mega drive" | "genesis" | "sega genesis" | "md" => Some("md"),
        "nes" | "nintendo entertainment system" | "famicom" => Some("nes"),
        "pc engine" | "turbografx 16" | "supergrafx" => Some("pce"),
        "pc engine cd" | "turbografx cd" => Some("pce"),
        "pc-fx" | "pcfx" => Some("pcfx"),
        "psx" | "ps1" | "playstation" => Some("psx"),
        "sms" | "sega master system" => Some("sms"),
        "snes" | "super nintendo" | "super famicom" => Some("snes"),
        "virtual boy" | "vb" => Some("vb"),
        "wonderswan" | "wonder swan" | "wonderswan color" => Some("wswan"),
        _ => None,
    }
}

fn parse_hex(value: &str, line: usize) -> Result<u64, MednafenCheatParseIssue> {
    let value = value.trim_start_matches("0x").trim_start_matches("0X");
    u64::from_str_radix(value, 16).map_err(|_| MednafenCheatParseIssue::InvalidNumber(line))
}

fn parse_target(
    system: &str,
    line: &str,
    line_number: usize,
) -> Result<MednafenCheatTarget, MednafenCheatParseIssue> {
    let Some((md5, title)) = line
        .trim()
        .strip_prefix('[')
        .and_then(|value| value.split_once(']'))
    else {
        return Err(MednafenCheatParseIssue::MalformedTarget(line_number));
    };
    if md5.len() != 32 || !md5.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(MednafenCheatParseIssue::MalformedTarget(line_number));
    }
    Ok(MednafenCheatTarget {
        system: system.to_ascii_lowercase(),
        md5: md5.to_ascii_lowercase(),
        title: title.trim().to_string(),
    })
}

fn normalized_direct(
    kind: char,
    width: u8,
    address: u64,
    value: u64,
    raw: &str,
) -> Option<CheatOperation> {
    match (kind, width, value) {
        ('R', 1, value) if value <= u8::MAX as u64 => Some(CheatOperation::OnFrameWrite8 {
            address,
            value: value as u8,
        }),
        ('R', 2, value) if value <= u16::MAX as u64 => Some(CheatOperation::OnFrameWrite16 {
            address,
            value: value as u16,
        }),
        ('R', 4, value) if value <= u32::MAX as u64 => Some(CheatOperation::OnFrameWrite32 {
            address,
            value: value as u32,
        }),
        ('S', 1, value) if value <= u8::MAX as u64 => Some(CheatOperation::Write8 {
            address,
            value: value as u8,
        }),
        ('S', 2, value) if value <= u16::MAX as u64 => Some(CheatOperation::Write16 {
            address,
            value: value as u16,
        }),
        ('S', 4, value) if value <= u32::MAX as u64 => Some(CheatOperation::Write32 {
            address,
            value: value as u32,
        }),
        _ => {
            let _ = raw;
            None
        }
    }
}

fn parse_operation(
    line: &str,
    line_number: usize,
    system: &str,
) -> Result<(String, MednafenCheatState, MednafenCheatOperation), MednafenCheatParseIssue> {
    let fields = line.split_whitespace().collect::<Vec<_>>();
    if fields.len() < 8 {
        return Err(MednafenCheatParseIssue::MalformedOperation(line_number));
    }
    let kind = fields[0]
        .chars()
        .next()
        .ok_or(MednafenCheatParseIssue::MalformedOperation(line_number))?;
    if !matches!(kind, 'S' | 'C' | 'R') {
        return Err(MednafenCheatParseIssue::UnsupportedKind(line_number));
    }
    let state = match fields[1] {
        "A" => MednafenCheatState::Enabled,
        "I" => MednafenCheatState::Disabled,
        _ => return Err(MednafenCheatParseIssue::InvalidState(line_number)),
    };
    let width = fields[2]
        .parse::<u8>()
        .map_err(|_| MednafenCheatParseIssue::UnsupportedWidth(line_number))?;
    if !(1..=8).contains(&width) {
        return Err(MednafenCheatParseIssue::UnsupportedWidth(line_number));
    }
    let endian = match fields[3] {
        "B" => MednafenEndian::Big,
        "L" => MednafenEndian::Little,
        _ => MednafenEndian::Unknown,
    };
    let address = parse_hex(fields[5], line_number)?;
    let value = parse_hex(fields[6], line_number)?;
    let compare = if kind == 'C' {
        Some(parse_hex(
            fields
                .get(7)
                .ok_or(MednafenCheatParseIssue::MalformedOperation(line_number))?,
            line_number,
        )?)
    } else {
        None
    };
    let name_index = if compare.is_some() { 8 } else { 7 };
    let name = fields
        .get(name_index..)
        .map(|parts| parts.join(" "))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("{system} cheat at 0x{address:x}"));
    let memory_space = MednafenMemorySpace::SystemMemory;
    let raw = line.to_string();
    let operation = if let Some(compare) = compare {
        MednafenCheatOperation::Conditional {
            kind,
            memory_space,
            address,
            width_bytes: width,
            value,
            compare,
            endian,
            raw,
        }
    } else {
        MednafenCheatOperation::Direct {
            kind,
            memory_space,
            address,
            width_bytes: width,
            value,
            endian,
            normalized: normalized_direct(kind, width, address, value, line),
            raw,
        }
    };
    Ok((name, state, operation))
}

pub fn parse_mednafen_cheat_file(
    system: &str,
    bytes: &[u8],
) -> Result<MednafenCheatFile, MednafenCheatParseIssue> {
    if bytes.is_empty() {
        return Err(MednafenCheatParseIssue::Empty);
    }
    if bytes.len() > MEDNAFEN_CHEAT_MAX_BYTES {
        return Err(MednafenCheatParseIssue::TooLarge);
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| MednafenCheatParseIssue::MalformedOperation(0))?;
    let lines = text.lines().collect::<Vec<_>>();
    if lines.len() > MEDNAFEN_CHEAT_MAX_LINES {
        return Err(MednafenCheatParseIssue::TooManyLines);
    }
    let mut current: Option<MednafenCheatTarget> = None;
    let mut entries = Vec::new();
    let mut comments = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let line_number = index + 1;
        if line.len() > MEDNAFEN_CHEAT_MAX_LINE_BYTES {
            return Err(MednafenCheatParseIssue::LineTooLong(line_number));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('#') || trimmed.starts_with(';') {
            comments.push((*line).to_string());
            continue;
        }
        if trimmed.starts_with('[') {
            current = Some(parse_target(system, trimmed, line_number)?);
            continue;
        }
        let target = current
            .clone()
            .ok_or(MednafenCheatParseIssue::MissingTarget(line_number))?;
        let (name, state, operation) = parse_operation(trimmed, line_number, system)?;
        if entries.len() >= MEDNAFEN_CHEAT_MAX_ENTRIES {
            return Err(MednafenCheatParseIssue::TooManyEntries);
        }
        entries.push(MednafenCheatEntry {
            target,
            name,
            state,
            operations: vec![operation],
        });
    }
    if entries.is_empty() {
        return Err(MednafenCheatParseIssue::Empty);
    }
    Ok(MednafenCheatFile {
        system: system.to_ascii_lowercase(),
        entries,
        comments,
    })
}

pub fn render_mednafen_cheat_file(file: &MednafenCheatFile) -> Vec<u8> {
    let mut out = String::new();
    for comment in &file.comments {
        out.push_str(comment);
        out.push('\n');
    }
    let mut last_target: Option<&MednafenCheatTarget> = None;
    for entry in &file.entries {
        if last_target != Some(&entry.target) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("[{}] {}\n", entry.target.md5, entry.target.title));
            last_target = Some(&entry.target);
        }
        for operation in &entry.operations {
            match operation {
                MednafenCheatOperation::Direct {
                    kind,
                    endian,
                    width_bytes,
                    address,
                    value,
                    ..
                } => out.push_str(&format!(
                    "{kind} {} {width_bytes} {} 0 {address:08x} {value:x} {}\n",
                    if entry.state == MednafenCheatState::Enabled {
                        'A'
                    } else {
                        'I'
                    },
                    match endian {
                        MednafenEndian::Big => 'B',
                        MednafenEndian::Little => 'L',
                        MednafenEndian::Unknown => 'L',
                    },
                    entry.name
                )),
                MednafenCheatOperation::Conditional {
                    kind,
                    endian,
                    width_bytes,
                    address,
                    value,
                    compare,
                    ..
                } => out.push_str(&format!(
                    "{kind} {} {width_bytes} {} 0 {address:08x} {value:x} {compare:x} {}\n",
                    if entry.state == MednafenCheatState::Enabled {
                        'A'
                    } else {
                        'I'
                    },
                    match endian {
                        MednafenEndian::Big => 'B',
                        MednafenEndian::Little => 'L',
                        MednafenEndian::Unknown => 'L',
                    },
                    entry.name
                )),
                MednafenCheatOperation::Opaque { raw, .. } => {
                    out.push_str(raw);
                    out.push('\n');
                }
            }
        }
    }
    out.into_bytes()
}

pub fn mednafen_compatibility(entries: &[MednafenCheatEntry]) -> CheatCompatibilityReport {
    let converted = entries
        .iter()
        .map(|entry| {
            let operations = entry
                .operations
                .iter()
                .map(|operation| match operation {
                    MednafenCheatOperation::Direct {
                        normalized: Some(operation),
                        ..
                    } => {
                        super::cheat_compatibility::CheatCompatibilityOperation::from_ir(operation)
                    }
                    MednafenCheatOperation::Conditional { raw, .. } => {
                        super::cheat_compatibility::CheatCompatibilityOperation::Unknown {
                            raw: raw.clone(),
                            reason: "Mednafen compare condition retained natively".into(),
                        }
                    }
                    MednafenCheatOperation::Opaque { raw, reason } => {
                        super::cheat_compatibility::CheatCompatibilityOperation::Unknown {
                            raw: raw.clone(),
                            reason: reason.clone(),
                        }
                    }
                    MednafenCheatOperation::Direct { raw, .. } => {
                        super::cheat_compatibility::CheatCompatibilityOperation::Unknown {
                            raw: raw.clone(),
                            reason: "unsupported width or native semantics".into(),
                        }
                    }
                })
                .collect();
            CheatCompatibilityEntry {
                id: entry.name.clone(),
                title: entry.name.clone(),
                platform: CheatPlatform::Other(format!(
                    "mednafen:{}:system-memory",
                    entry.target.system
                )),
                provider: "Mednafen".into(),
                source: "local .cht".into(),
                operations,
                revision: CheatRevisionEvidence::ExactHash {
                    hash: entry.target.md5.clone(),
                },
                master_code: CheatMasterCodeRequirement::None,
                original_code_id: Some(entry.name.clone()),
            }
        })
        .collect::<Vec<_>>();
    analyze_cheat_stack(&converted)
}

fn valid_md5(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Merge only the selected game's section into an existing system cheat file.
/// Other games' sections and comments remain intact, while a same-name entry
/// for the exact MD5 is replaced deterministically.
pub fn merge_mednafen_cheat_files(
    existing: Option<&MednafenCheatFile>,
    incoming: &MednafenCheatFile,
) -> Result<MednafenCheatFile, MednafenCheatError> {
    if let Some(existing) = existing {
        if existing.system != incoming.system {
            return Err(MednafenCheatError::IdentityMismatch);
        }
    }
    let mut merged = existing.cloned().unwrap_or_else(|| MednafenCheatFile {
        system: incoming.system.clone(),
        entries: Vec::new(),
        comments: Vec::new(),
    });
    for comment in &incoming.comments {
        if !merged.comments.contains(comment) {
            merged.comments.push(comment.clone());
        }
    }
    for new_entry in &incoming.entries {
        if let Some(old_entry) = merged.entries.iter_mut().find(|entry| {
            entry.target.system == new_entry.target.system
                && entry.target.md5 == new_entry.target.md5
                && entry.name == new_entry.name
        }) {
            *old_entry = new_entry.clone();
        } else {
            merged.entries.push(new_entry.clone());
        }
    }
    Ok(merged)
}

pub fn build_mednafen_cheat_plan(
    staging_root: &Path,
    profile_root: &Path,
    selected_archive: &Path,
    profile_id: &str,
    system: &str,
    game_md5: &str,
    identity_verified: bool,
    incoming: &MednafenCheatFile,
) -> Result<MednafenCheatPlan, MednafenCheatError> {
    if !SUPPORTED_SYSTEMS.contains(&system) {
        return Err(MednafenCheatError::UnsupportedSystem(system.into()));
    }
    if !identity_verified || !valid_md5(game_md5) {
        return Err(MednafenCheatError::IdentityRequired);
    }
    if incoming.system != system
        || incoming
            .entries
            .iter()
            .any(|entry| entry.target.md5 != game_md5.to_ascii_lowercase())
    {
        return Err(MednafenCheatError::IdentityMismatch);
    }
    if !staging_root.is_absolute() || !profile_root.is_absolute() {
        return Err(MednafenCheatError::UnsafePath);
    }
    let destination = profile_root.join("cheats").join(format!("{system}.cht"));
    if fs::symlink_metadata(&destination)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(MednafenCheatError::UnsafePath);
    }
    let existing = match fs::read(&destination) {
        Ok(bytes) => {
            Some(parse_mednafen_cheat_file(system, &bytes).map_err(MednafenCheatError::Parse)?)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(MednafenCheatError::Preview(error.to_string())),
    };
    let merged = merge_mednafen_cheat_files(existing.as_ref(), incoming)?;
    let output_bytes = render_mednafen_cheat_file(&merged);
    fs::create_dir_all(staging_root.join("cheats"))
        .map_err(|error| MednafenCheatError::Preview(error.to_string()))?;
    let source = staging_root.join("cheats").join(format!("{system}.cht"));
    fs::write(&source, &output_bytes)
        .map_err(|error| MednafenCheatError::Preview(error.to_string()))?;
    let reparsed =
        parse_mednafen_cheat_file(system, &output_bytes).map_err(MednafenCheatError::Parse)?;
    let report = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::Mednafen,
        selected_archive: selected_archive.to_path_buf(),
        platform: Some(system.into()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::MednafenMd5,
            state: PreviewIdentityState::Verified,
            value: Some(game_md5.to_ascii_lowercase()),
            archive_path: selected_archive.to_path_buf(),
            revision: None,
        },
        destination_root: profile_root.to_path_buf(),
        source_items: vec![PreviewSourceItem {
            adapter: PreviewAdapter::Mednafen,
            source_path: source.clone(),
            expected_source_digest: None,
            destination_relative_paths: vec![PathBuf::from("cheats").join(format!("{system}.cht"))],
            match_strength: PreviewMatchStrength::VerifiedExact,
        }],
    })
    .map_err(|error| MednafenCheatError::Preview(error.to_string()))?;
    let transaction = build_shared_transaction_plan(
        &report,
        profile_id,
        MEDNAFEN_CHEAT_SOURCE_MODE,
        staging_root,
    )
    .map_err(|error| MednafenCheatError::Preview(error.detail))?;
    let readiness = if reparsed.entries.iter().any(|entry| {
        entry.operations.iter().any(|operation| {
            !matches!(
                operation,
                MednafenCheatOperation::Direct {
                    normalized: Some(_),
                    ..
                }
            )
        })
    }) {
        MednafenCheatReadiness::ReadyWithOpaqueNativeOps
    } else {
        MednafenCheatReadiness::Ready
    };
    Ok(MednafenCheatPlan {
        report,
        transaction,
        readiness,
        output_bytes,
        destination,
        system: system.into(),
    })
}

pub fn apply_mednafen_cheat_plan(
    plan: &MednafenCheatPlan,
    options: &SharedApplyOptions,
) -> SharedApplyResult {
    execute_shared_apply(&plan.transaction, options)
}
pub fn preview_mednafen_cheat_rollback(
    journal: &Path,
    root: &Path,
    backup: &Path,
) -> super::shared_transaction::SharedRollbackPreview {
    preview_shared_rollback(journal, root, backup)
}
pub fn rollback_mednafen_cheat(
    preview: &super::shared_transaction::SharedRollbackPreview,
    options: &SharedRollbackOptions,
) -> SharedRollbackResult {
    execute_shared_rollback(preview, options)
}

pub fn mednafen_loadability_facts(
    profile_root: &Path,
    system: &str,
    game_md5: &str,
) -> Option<MednafenLoadabilityFacts> {
    (SUPPORTED_SYSTEMS.contains(&system) && valid_md5(game_md5)).then(|| MednafenLoadabilityFacts {
        system: system.into(),
        cheat_path: profile_root.join("cheats").join(format!("{system}.cht")),
        game_md5: game_md5.to_ascii_lowercase(),
        persistent_state: true,
        restart_required: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHT: &str = "# local fixture\n[0123456789abcdef0123456789abcdef] Example\nR A 1 L 0 001f006d 09 Infinite lives\nC I 2 B 0 001f0070 0003 0001 Conditional lives\nR A 8 L 0 001f0080 1234567890abcdef Opaque wide\n";

    #[test]
    fn parses_direct_compare_and_opaque_operations() {
        let file = parse_mednafen_cheat_file("pce", CHT.as_bytes()).unwrap();
        assert_eq!(file.entries.len(), 3);
        assert!(matches!(
            file.entries[0].operations[0],
            MednafenCheatOperation::Direct {
                normalized: Some(CheatOperation::OnFrameWrite8 { .. }),
                ..
            }
        ));
        assert!(matches!(
            file.entries[1].operations[0],
            MednafenCheatOperation::Conditional { .. }
        ));
        assert!(matches!(
            file.entries[2].operations[0],
            MednafenCheatOperation::Direct {
                normalized: None,
                ..
            }
        ));
    }

    #[test]
    fn identity_and_system_are_required_for_apply() {
        let file = parse_mednafen_cheat_file("pce", CHT.as_bytes()).unwrap();
        assert!(
            build_mednafen_cheat_plan(
                Path::new("/tmp/stage"),
                Path::new("/tmp/profile"),
                Path::new("/tmp/game"),
                "p",
                "pce",
                "bad",
                true,
                &file
            )
            .is_err()
        );
        assert!(
            build_mednafen_cheat_plan(
                Path::new("/tmp/stage"),
                Path::new("/tmp/profile"),
                Path::new("/tmp/game"),
                "p",
                "pce",
                "0123456789abcdef0123456789abcdef",
                false,
                &file
            )
            .is_err()
        );
    }

    #[test]
    fn different_systems_do_not_share_address_conflicts() {
        let pce = parse_mednafen_cheat_file("pce", CHT.as_bytes()).unwrap();
        let nes = parse_mednafen_cheat_file("nes", CHT.as_bytes()).unwrap();
        let pce_report = mednafen_compatibility(&pce.entries);
        let nes_report = mednafen_compatibility(&nes.entries);
        assert!(pce_report.conflicts.iter().all(|conflict| !matches!(
            conflict.kind,
            super::super::cheat_compatibility::CheatConflictKind::SameAddressDifferentValue
                | super::super::cheat_compatibility::CheatConflictKind::OverlappingRange
        )));
        assert_eq!(pce_report.operation_count, nes_report.operation_count);
    }

    #[test]
    fn rendering_is_deterministic_and_state_persists() {
        let file = parse_mednafen_cheat_file("pce", CHT.as_bytes()).unwrap();
        assert_eq!(
            render_mednafen_cheat_file(&file),
            render_mednafen_cheat_file(&file)
        );
        let round_trip =
            parse_mednafen_cheat_file("pce", &render_mednafen_cheat_file(&file)).unwrap();
        assert_eq!(round_trip.entries[0].state, MednafenCheatState::Enabled);
        assert_eq!(round_trip.entries[1].state, MednafenCheatState::Disabled);
    }

    #[test]
    fn merge_replaces_selected_entry_and_preserves_other_games() {
        let existing = parse_mednafen_cheat_file(
            "pce",
            b"[aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa] Other\nR A 1 L 0 10 01 Other\n[0123456789abcdef0123456789abcdef] Example\nR I 1 L 0 20 01 Infinite lives\n",
        )
        .unwrap();
        let incoming = parse_mednafen_cheat_file(
            "pce",
            b"[0123456789abcdef0123456789abcdef] Example\nR A 1 L 0 20 09 Infinite lives\n",
        )
        .unwrap();
        let merged = merge_mednafen_cheat_files(Some(&existing), &incoming).unwrap();
        assert_eq!(merged.entries.len(), 2);
        assert_eq!(
            merged.entries[0].target.md5,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(merged.entries[1].state, MednafenCheatState::Enabled);
        assert!(
            String::from_utf8(render_mednafen_cheat_file(&merged))
                .unwrap()
                .contains(" 9 Infinite lives")
        );
    }
}
