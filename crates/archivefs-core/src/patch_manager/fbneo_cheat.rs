//! Conservative native FinalBurn Neo cheat support.
//!
//! FBNeo's documented native format is a per-set FB Alpha style INI file.
//! This module deliberately understands only the documented byte-write
//! option grammar. Includes and malformed/unknown records remain visible as
//! opaque issues; they are never silently interpreted or applied.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::cheat_ir::CheatOperation;
use super::destination_safety::{DestinationState, assess_destination};
use super::shared_preview::{
    PreviewAdapter, PreviewIdentity, PreviewIdentityKind, PreviewIdentityState,
    PreviewMatchStrength, PreviewSourceItem, SharedPreviewRequest, build_shared_preview,
};
use super::shared_transaction::{
    SharedApplyConfirmation, SharedApplyOptions, SharedApplyResult, build_shared_transaction_plan,
    execute_shared_apply,
};

pub const FBNEO_MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
pub const FBNEO_MAX_LINES: usize = 32_768;
pub const FBNEO_MAX_CHEATS: usize = 4_096;
pub const FBNEO_MAX_OPTIONS_PER_CHEAT: usize = 96;
pub const FBNEO_MAX_OPERATIONS_PER_OPTION: usize = 32;
pub const FBNEO_MAX_LINE_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FbneoCheatFormat {
    FbAlphaIni,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FbneoCheatTarget {
    pub shortname: String,
    pub system: Option<String>,
    pub identity_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FbneoCheatOperation {
    Write8 { cpu: u8, address: u64, value: u8 },
    Opaque { raw: String, reason: String },
}

impl FbneoCheatOperation {
    pub fn as_neutral(&self) -> Option<CheatOperation> {
        match self {
            Self::Write8 { address, value, .. } => Some(CheatOperation::Write8 {
                address: *address,
                value: *value,
            }),
            Self::Opaque { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FbneoCheatState {
    Enabled,
    Disabled,
    RuntimeEnableRequired,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FbneoCheatReadiness {
    Ready,
    RuntimeEnableRequired,
    NotReady,
    Unsupported,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FbneoCheatIssue {
    IncludeUnsupported {
        line: usize,
        raw: String,
    },
    Malformed {
        line: usize,
        detail: String,
        raw: String,
    },
    UnknownLine {
        line: usize,
        raw: String,
    },
    IdentityUnverified,
    IdentityMismatch {
        expected: String,
        actual: String,
    },
    DuplicateCheatName {
        name: String,
    },
    UnsupportedOperation {
        line: usize,
        raw: String,
    },
    FileTooLarge,
    TooManyLines,
    TooManyCheats,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FbneoCheatOption {
    pub number: u8,
    pub label: String,
    pub operations: Vec<FbneoCheatOperation>,
    pub raw_lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FbneoCheatEntry {
    pub name: String,
    pub cheat_type: u8,
    pub default_option: u8,
    pub state: FbneoCheatState,
    pub options: Vec<FbneoCheatOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FbneoCheatFile {
    pub format: FbneoCheatFormat,
    pub target: FbneoCheatTarget,
    pub comments: Vec<String>,
    pub entries: Vec<FbneoCheatEntry>,
    pub trailing_raw_lines: Vec<String>,
    pub issues: Vec<FbneoCheatIssue>,
    pub source_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FbneoCheatLoadabilityFacts {
    pub profile: String,
    pub set_shortname: String,
    pub destination: String,
    pub format: FbneoCheatFormat,
    pub state: FbneoCheatState,
    pub restart_required: bool,
    pub readiness: FbneoCheatReadiness,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FbneoCheatDestination {
    pub cheat_root: PathBuf,
    pub set_shortname: String,
    pub file_name: String,
    pub path: PathBuf,
    pub profile: String,
    pub identity_evidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FbneoCheatApplyPlan {
    pub destination: FbneoCheatDestination,
    pub output_sha256: String,
    pub existing_sha256: Option<String>,
    pub output_bytes: Vec<u8>,
    pub readiness: FbneoCheatReadiness,
}

#[derive(Debug, Clone)]
pub struct FbneoCheatApplyOptions {
    pub general_approved: bool,
    pub replacement_approved: bool,
    pub operation_id: String,
    pub timestamp_unix_seconds: u64,
    pub history_root: PathBuf,
    pub backup_root: PathBuf,
}

pub fn build_fbneo_cheat_apply_plan(
    cheat_root: &Path,
    profile: &str,
    file: &FbneoCheatFile,
    output_bytes: Vec<u8>,
) -> Result<FbneoCheatApplyPlan, String> {
    if !file.target.identity_verified {
        return Err("exact FBNeo set shortname identity is required".into());
    }
    if file.target.shortname.is_empty()
        || file.target.shortname == "."
        || file.target.shortname == ".."
        || file.target.shortname.contains(['/', '\\'])
    {
        return Err("FBNeo set shortname is not a safe path component".into());
    }
    let parent = cheat_root
        .parent()
        .ok_or_else(|| "FBNeo cheat root has no safe parent".to_string())?;
    let directory = cheat_root
        .file_name()
        .ok_or_else(|| "FBNeo cheat root has no directory name".to_string())?;
    let file_name = format!("{}.ini", file.target.shortname);
    let assessment = assess_destination(parent, directory, std::ffi::OsStr::new(&file_name))
        .map_err(|error| error.to_string())?;
    let existing_sha256 = if assessment.destination_state == DestinationState::RegularFile {
        Some(sha256(
            &fs::read(assessment.proposed_destination.path()).map_err(|error| error.to_string())?,
        ))
    } else {
        None
    };
    Ok(FbneoCheatApplyPlan {
        destination: FbneoCheatDestination {
            cheat_root: cheat_root.to_path_buf(),
            set_shortname: file.target.shortname.clone(),
            file_name,
            path: assessment.proposed_destination.path().to_path_buf(),
            profile: profile.into(),
            identity_evidence: "verified FBNeo set shortname".into(),
        },
        output_sha256: sha256(&output_bytes),
        existing_sha256,
        output_bytes,
        readiness: fbneo_readiness(file),
    })
}

/// Applies a reviewed native INI through the shared atomic transaction and
/// history machinery. The original ROM/set files are never opened or changed.
pub fn apply_fbneo_cheat_plan(
    plan: &FbneoCheatApplyPlan,
    options: &FbneoCheatApplyOptions,
) -> Result<SharedApplyResult, String> {
    if !matches!(
        plan.readiness,
        FbneoCheatReadiness::Ready | FbneoCheatReadiness::RuntimeEnableRequired
    ) {
        return Err("FBNeo cheat plan is not ready for apply".into());
    }
    let stage = tempfile::tempdir().map_err(|error| error.to_string())?;
    let staged = stage.path().join(&plan.destination.file_name);
    fs::write(&staged, &plan.output_bytes).map_err(|error| error.to_string())?;
    let destination_root = plan
        .destination
        .cheat_root
        .parent()
        .ok_or_else(|| "FBNeo destination root has no parent".to_string())?;
    let relative = plan
        .destination
        .path
        .strip_prefix(destination_root)
        .map_err(|_| "FBNeo destination escaped its configured root".to_string())?
        .to_path_buf();
    let preview = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::Fbneo,
        selected_archive: PathBuf::from(&plan.destination.set_shortname),
        platform: Some("Arcade".into()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::MameMachineShortname,
            state: PreviewIdentityState::Verified,
            value: Some(plan.destination.set_shortname.clone()),
            archive_path: PathBuf::from(&plan.destination.set_shortname),
            revision: None,
        },
        destination_root: destination_root.to_path_buf(),
        source_items: vec![PreviewSourceItem {
            adapter: PreviewAdapter::Fbneo,
            source_path: staged,
            expected_source_digest: Some(plan.output_sha256.clone()),
            destination_relative_paths: vec![relative],
            match_strength: PreviewMatchStrength::VerifiedExact,
        }],
    })
    .map_err(|error| error.to_string())?;
    if preview
        .entries
        .iter()
        .any(|entry| !entry.blockers.is_empty())
    {
        return Err("FBNeo destination or source changed since preview".into());
    }
    let transaction =
        build_shared_transaction_plan(&preview, "fbneo", "fbneo_native_cheat", stage.path())
            .map_err(|error| format!("{error:?}"))?;
    let context = transaction.context.clone();
    Ok(execute_shared_apply(
        &transaction,
        &SharedApplyOptions {
            dry_run: false,
            confirmation: Some(SharedApplyConfirmation {
                plan_id: transaction.plan_id.clone(),
                general_approved: options.general_approved,
                replacement_approved: options.replacement_approved,
            }),
            operation_id: options.operation_id.clone(),
            timestamp_unix_seconds: options.timestamp_unix_seconds,
            current_context: context,
            history_root: options.history_root.clone(),
            backup_root: options.backup_root.clone(),
        },
    ))
}

pub fn parse_fbneo_cheat_file(bytes: &[u8], target: FbneoCheatTarget) -> FbneoCheatFile {
    let source_sha256 = (bytes.len() <= FBNEO_MAX_FILE_BYTES).then(|| sha256(bytes));
    let text = String::from_utf8_lossy(bytes);
    let mut file = FbneoCheatFile {
        format: FbneoCheatFormat::FbAlphaIni,
        target,
        comments: Vec::new(),
        entries: Vec::new(),
        trailing_raw_lines: Vec::new(),
        issues: Vec::new(),
        source_sha256,
    };
    if bytes.len() > FBNEO_MAX_FILE_BYTES {
        file.issues.push(FbneoCheatIssue::FileTooLarge);
        return file;
    }

    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > FBNEO_MAX_LINES {
        file.issues.push(FbneoCheatIssue::TooManyLines);
        return file;
    }
    let mut index = 0;
    while index < lines.len() {
        let raw = lines[index].trim();
        if raw.is_empty() || raw.starts_with("//") {
            file.comments.push(lines[index].to_string());
            index += 1;
            continue;
        }
        if raw.starts_with("include") {
            file.issues.push(FbneoCheatIssue::IncludeUnsupported {
                line: index + 1,
                raw: raw.to_string(),
            });
            file.trailing_raw_lines.push(lines[index].to_string());
            index += 1;
            continue;
        }
        let Some(name) = quoted_after(raw, "cheat") else {
            file.issues.push(FbneoCheatIssue::UnknownLine {
                line: index + 1,
                raw: raw.to_string(),
            });
            file.trailing_raw_lines.push(lines[index].to_string());
            index += 1;
            continue;
        };
        if file.entries.len() >= FBNEO_MAX_CHEATS {
            file.issues.push(FbneoCheatIssue::TooManyCheats);
            break;
        }
        let mut entry = FbneoCheatEntry {
            name,
            cheat_type: 0,
            default_option: 0,
            state: FbneoCheatState::Disabled,
            options: Vec::new(),
        };
        let opened = raw.ends_with('{');
        index += 1;
        if !opened && index >= lines.len() {
            file.issues.push(FbneoCheatIssue::Malformed {
                line: index,
                detail: "cheat block has no body".into(),
                raw: raw.to_string(),
            });
            break;
        }
        let mut closed = false;
        while index < lines.len() {
            let body_raw = lines[index].trim();
            if body_raw == "}" {
                closed = true;
                index += 1;
                break;
            }
            if body_raw.is_empty() || body_raw.starts_with("//") {
                index += 1;
                continue;
            }
            if body_raw.starts_with("type ") {
                match parse_num(body_raw.trim_start_matches("type ").trim()) {
                    Some(value @ 0..=2) => entry.cheat_type = value as u8,
                    _ => file
                        .issues
                        .push(malformed(index, "invalid cheat type", body_raw)),
                }
            } else if body_raw.starts_with("default ") {
                match parse_num(body_raw.trim_start_matches("default ").trim()) {
                    Some(value @ 0..=95) => {
                        entry.default_option = value as u8;
                        entry.state = if value == 0 {
                            FbneoCheatState::Disabled
                        } else {
                            FbneoCheatState::Enabled
                        };
                    }
                    _ => file
                        .issues
                        .push(malformed(index, "invalid default option", body_raw)),
                }
            } else if let Some(option) = parse_option(body_raw, index, &mut file.issues) {
                if entry.options.len() >= FBNEO_MAX_OPTIONS_PER_CHEAT {
                    file.issues
                        .push(malformed(index, "too many options", body_raw));
                } else {
                    entry.options.push(option);
                }
            } else {
                entry.options.push(FbneoCheatOption {
                    number: 0,
                    label: String::new(),
                    operations: vec![FbneoCheatOperation::Opaque {
                        raw: body_raw.to_string(),
                        reason: "native line is not a documented FBNeo option".into(),
                    }],
                    raw_lines: vec![body_raw.to_string()],
                });
                file.issues.push(FbneoCheatIssue::UnknownLine {
                    line: index + 1,
                    raw: body_raw.to_string(),
                });
            }
            index += 1;
        }
        if !closed {
            file.issues
                .push(malformed(index, "unterminated cheat block", &entry.name));
        }
        file.entries.push(entry);
    }
    detect_duplicate_names(&mut file);
    file
}

pub fn render_fbneo_cheat_file(file: &FbneoCheatFile) -> String {
    let mut output = String::new();
    for comment in &file.comments {
        let _ = writeln!(output, "{comment}");
    }
    for entry in &file.entries {
        let _ = writeln!(output, "cheat \"{}\" {{", escape(&entry.name));
        let _ = writeln!(output, " type {}", entry.cheat_type);
        let _ = writeln!(output, " default {}", entry.default_option);
        for option in &entry.options {
            let _ = write!(output, " {} \"{}\"", option.number, escape(&option.label));
            for operation in &option.operations {
                match operation {
                    FbneoCheatOperation::Write8 {
                        cpu,
                        address,
                        value,
                    } => {
                        let _ = write!(output, ", {cpu}, 0x{address:X}, 0x{value:02X}");
                    }
                    FbneoCheatOperation::Opaque { raw, .. } => {
                        let _ = write!(output, " // opaque: {raw}");
                    }
                }
            }
            output.push('\n');
            for raw in &option.raw_lines {
                if !raw.is_empty() && !option.operations.iter().any(|op| matches!(op, FbneoCheatOperation::Opaque { raw: value, .. } if value == raw)) {
                    let _ = writeln!(output, " // {raw}");
                }
            }
        }
        output.push_str("}\n\n");
    }
    for line in &file.trailing_raw_lines {
        let _ = writeln!(output, "{line}");
    }
    output
}

pub fn merge_fbneo_cheat_file(
    existing: &FbneoCheatFile,
    additions: impl IntoIterator<Item = FbneoCheatEntry>,
) -> (FbneoCheatFile, Vec<FbneoCheatIssue>) {
    let mut merged = existing.clone();
    let mut issues = Vec::new();
    for addition in additions {
        if merged
            .entries
            .iter()
            .any(|entry| entry.name == addition.name)
        {
            issues.push(FbneoCheatIssue::DuplicateCheatName {
                name: addition.name,
            });
        } else {
            merged.entries.push(addition);
        }
    }
    issues
        .iter()
        .cloned()
        .for_each(|issue| merged.issues.push(issue));
    (merged, issues)
}

pub fn fbneo_readiness(file: &FbneoCheatFile) -> FbneoCheatReadiness {
    if !file.target.identity_verified {
        return FbneoCheatReadiness::NotReady;
    }
    if file.issues.iter().any(|issue| {
        matches!(
            issue,
            FbneoCheatIssue::IncludeUnsupported { .. }
                | FbneoCheatIssue::Malformed { .. }
                | FbneoCheatIssue::UnknownLine { .. }
                | FbneoCheatIssue::UnsupportedOperation { .. }
        )
    }) {
        return FbneoCheatReadiness::Unsupported;
    }
    if file.entries.iter().any(|entry| {
        entry.options.iter().any(|option| {
            option
                .operations
                .iter()
                .any(|op| matches!(op, FbneoCheatOperation::Opaque { .. }))
        })
    }) {
        // The native line is retained for review, but cannot be safely
        // rendered by the bounded writer without changing its semantics.
        return FbneoCheatReadiness::Unsupported;
    }
    if file.entries.iter().any(|entry| entry.cheat_type > 0) {
        FbneoCheatReadiness::RuntimeEnableRequired
    } else {
        FbneoCheatReadiness::Ready
    }
}

fn parse_option(
    line: &str,
    line_index: usize,
    issues: &mut Vec<FbneoCheatIssue>,
) -> Option<FbneoCheatOption> {
    let first_space = line.find(char::is_whitespace)?;
    let number = parse_num(line[..first_space].trim())?;
    if !(0..=95).contains(&number) {
        issues.push(malformed(line_index, "option number outside 0..95", line));
        return None;
    }
    let remainder = line[first_space..].trim_start();
    let (label, tail) = quoted_prefix(remainder)?;
    let operation_tail = tail.trim().trim_start_matches(',').trim();
    let mut operations = Vec::new();
    let values: Vec<&str> = if operation_tail.is_empty() {
        Vec::new()
    } else {
        operation_tail.split(',').map(str::trim).collect()
    };
    if values.len() % 3 != 0 || values.len() / 3 > FBNEO_MAX_OPERATIONS_PER_OPTION {
        issues.push(malformed(
            line_index,
            "option must contain up to 32 CPU/address/value triples",
            line,
        ));
        return Some(FbneoCheatOption {
            number: number as u8,
            label,
            operations: vec![FbneoCheatOperation::Opaque {
                raw: line.to_string(),
                reason: "invalid operation triple count".into(),
            }],
            raw_lines: vec![line.to_string()],
        });
    }
    for group in values.chunks(3) {
        let Some(cpu) = parse_num(group[0]).and_then(|v| u8::try_from(v).ok()) else {
            issues.push(FbneoCheatIssue::UnsupportedOperation {
                line: line_index + 1,
                raw: line.to_string(),
            });
            operations.push(FbneoCheatOperation::Opaque {
                raw: line.to_string(),
                reason: "CPU number is not bounded".into(),
            });
            continue;
        };
        let (Some(address), Some(value)) = (
            parse_num(group[1]),
            parse_num(group[2]).and_then(|v| u8::try_from(v).ok()),
        ) else {
            issues.push(FbneoCheatIssue::UnsupportedOperation {
                line: line_index + 1,
                raw: line.to_string(),
            });
            operations.push(FbneoCheatOperation::Opaque {
                raw: line.to_string(),
                reason: "address/value is not a bounded number".into(),
            });
            continue;
        };
        operations.push(FbneoCheatOperation::Write8 {
            cpu,
            address,
            value,
        });
    }
    Some(FbneoCheatOption {
        number: number as u8,
        label,
        operations,
        raw_lines: Vec::new(),
    })
}

fn quoted_after(line: &str, prefix: &str) -> Option<String> {
    let rest = line.strip_prefix(prefix)?.trim_start();
    quoted_prefix(rest).map(|(value, _)| value)
}

fn quoted_prefix(value: &str) -> Option<(String, &str)> {
    let value = value.strip_prefix('"')?;
    let end = value.find('"')?;
    Some((value[..end].to_string(), &value[end + 1..]))
}

fn parse_num(value: &str) -> Option<u64> {
    let value = value.trim();
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16).ok()
    } else {
        value.parse().ok()
    }
}

fn malformed(index: usize, detail: &str, raw: &str) -> FbneoCheatIssue {
    FbneoCheatIssue::Malformed {
        line: index + 1,
        detail: detail.into(),
        raw: raw.into(),
    }
}

fn detect_duplicate_names(file: &mut FbneoCheatFile) {
    for index in 0..file.entries.len() {
        if file.entries[..index]
            .iter()
            .any(|entry| entry.name == file.entries[index].name)
        {
            file.issues.push(FbneoCheatIssue::DuplicateCheatName {
                name: file.entries[index].name.clone(),
            });
        }
    }
}

fn escape(value: &str) -> String {
    value.replace('"', "\\\"")
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(verified: bool) -> FbneoCheatTarget {
        FbneoCheatTarget {
            shortname: "mslug".into(),
            system: Some("Neo Geo".into()),
            identity_verified: verified,
        }
    }

    const FILE: &str = r#"// local fixture
cheat "Infinite Lives" {
 type 0
 default 1
 0 "Disabled"
 1 "Lives", 0, 0x1234, 0x09, 1, 0x5678, 0x09
}

cheat "Opaque" {
 default 0
 0 "Disabled"
 1 "Native", foo
}
"#;

    #[test]
    fn parses_set_specific_native_byte_writes_and_state() {
        let file = parse_fbneo_cheat_file(FILE.as_bytes(), target(true));
        assert_eq!(file.entries.len(), 2);
        assert_eq!(file.entries[0].state, FbneoCheatState::Enabled);
        assert_eq!(file.entries[0].options[1].operations.len(), 2);
        assert_eq!(fbneo_readiness(&file), FbneoCheatReadiness::Unsupported);
        assert_eq!(
            file.entries[0].options[1].operations[0].as_neutral(),
            Some(CheatOperation::Write8 {
                address: 0x1234,
                value: 9
            })
        );
    }

    #[test]
    fn identity_is_required_and_title_does_not_authorize_apply() {
        let file = parse_fbneo_cheat_file(FILE.as_bytes(), target(false));
        assert_eq!(fbneo_readiness(&file), FbneoCheatReadiness::NotReady);
    }

    #[test]
    fn writer_is_deterministic_and_merge_preserves_existing_entries() {
        let existing = parse_fbneo_cheat_file(FILE.as_bytes(), target(true));
        let addition = existing.entries[0].clone();
        let (merged, issues) = merge_fbneo_cheat_file(&existing, [addition]);
        assert!(matches!(
            issues[0],
            FbneoCheatIssue::DuplicateCheatName { .. }
        ));
        assert_eq!(
            render_fbneo_cheat_file(&merged),
            render_fbneo_cheat_file(&merged)
        );
        assert_eq!(merged.entries.len(), existing.entries.len());
    }

    #[test]
    fn malformed_and_include_records_fail_closed_but_remain_visible() {
        let file = parse_fbneo_cheat_file(
            b"include \"other.ini\"\ncheat \"bad\" {\n 1 \"x\", 1, nope\n",
            target(true),
        );
        assert!(
            file.issues
                .iter()
                .any(|issue| matches!(issue, FbneoCheatIssue::IncludeUnsupported { .. }))
        );
        assert_eq!(fbneo_readiness(&file), FbneoCheatReadiness::Unsupported);
    }

    #[test]
    fn exact_shortname_plan_and_shared_apply_never_touch_rom_content() {
        let root = tempfile::tempdir().unwrap();
        let system = root.path().join("system");
        let cheat_root = system.join("fbneo").join("cheats");
        std::fs::create_dir_all(&cheat_root).unwrap();
        let rom = root.path().join("mslug.zip");
        std::fs::write(&rom, b"ROM BYTES").unwrap();
        let file = parse_fbneo_cheat_file(
            b"cheat \"Lives\" {\n default 1\n 0 \"Off\"\n 1 \"On\", 0, 0x1234, 0x09\n}\n",
            target(true),
        );
        let bytes = render_fbneo_cheat_file(&file).into_bytes();
        let plan =
            build_fbneo_cheat_apply_plan(&cheat_root, "libretro-fbneo", &file, bytes.clone())
                .unwrap();
        assert_eq!(plan.destination.path, cheat_root.join("mslug.ini"));
        let result = apply_fbneo_cheat_plan(
            &plan,
            &FbneoCheatApplyOptions {
                general_approved: true,
                replacement_approved: false,
                operation_id: "fbneo-test".into(),
                timestamp_unix_seconds: 1,
                history_root: root.path().join("history"),
                backup_root: root.path().join("backup"),
            },
        )
        .unwrap();
        assert!(matches!(
            result.journal.status,
            super::super::shared_transaction::SharedApplyStatus::Success
        ));
        assert_eq!(std::fs::read(&rom).unwrap(), b"ROM BYTES");
        assert_eq!(std::fs::read(plan.destination.path).unwrap(), bytes);
    }
}
