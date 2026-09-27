//! Bounded native Flycast Dreamcast cheat files.
//!
//! Flycast stores cheats in a sectionless INI-like file.  The native file is
//! deliberately kept distinct from RetroArch `.cht`: upstream Flycast reads
//! `cheatN_*` fields such as `cheat_type`, `address`, `value`, and `enable`.
//! This adapter never guesses a CodeBreaker/GameShark decoder; imported
//! codes that Flycast has already converted are represented by their native
//! fields, while unknown fields and operation types remain opaque.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use super::shared_preview::{
    PreviewAdapter, PreviewIdentity, PreviewIdentityKind, PreviewIdentityState,
    PreviewMatchStrength, PreviewSourceItem, SharedPreviewReport, SharedPreviewRequest,
    build_shared_preview,
};
use super::shared_transaction::{
    SharedApplyConfirmation, SharedApplyOptions, SharedApplyResult, SharedTransactionPlan,
    build_shared_transaction_plan, execute_shared_apply,
};

pub const FLYCAST_CHEAT_MAX_BYTES: usize = 1024 * 1024;
pub const FLYCAST_CHEAT_MAX_LINES: usize = 8192;
pub const FLYCAST_CHEAT_MAX_LINE_BYTES: usize = 8192;
pub const FLYCAST_CHEAT_MAX_ENTRIES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlycastCheatState {
    Enabled,
    Disabled,
    RuntimeOnly,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlycastCheatType {
    Disabled,
    SetValue,
    Increase,
    Decrease,
    ConditionalEqual,
    ConditionalNotEqual,
    ConditionalGreater,
    ConditionalLess,
    Copy,
    Opaque(i32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DreamcastCheatCode {
    DirectWrite {
        address: u32,
        width: u32,
        value: u32,
    },
    Opaque {
        native_type: FlycastCheatType,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastCheatEntry {
    pub index: usize,
    pub description: String,
    pub enabled: bool,
    pub cheat_type: FlycastCheatType,
    pub code: DreamcastCheatCode,
    pub fields: BTreeMap<String, String>,
}

impl FlycastCheatEntry {
    pub fn direct_write(
        description: impl Into<String>,
        address: u32,
        width: u32,
        value: u32,
    ) -> Self {
        let mut fields = BTreeMap::new();
        fields.insert("desc".into(), description.into());
        fields.insert("address".into(), format!("{address}"));
        fields.insert("cheat_type".into(), "1".into());
        fields.insert(
            "memory_search_size".into(),
            width_to_shift(width).unwrap_or(0).to_string(),
        );
        fields.insert("value".into(), format!("{value}"));
        fields.insert("enable".into(), "true".into());
        Self {
            index: usize::MAX,
            description: fields["desc"].clone(),
            enabled: true,
            cheat_type: FlycastCheatType::SetValue,
            code: DreamcastCheatCode::DirectWrite {
                address,
                width,
                value,
            },
            fields,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastCheatFile {
    pub entries: Vec<FlycastCheatEntry>,
    pub preserved_lines: Vec<String>,
    pub declared_count: Option<usize>,
    pub warnings: Vec<FlycastCheatParseIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlycastCheatParseIssue {
    MalformedLine { line: usize },
    LineTooLong { line: usize },
    DuplicateField { entry: usize, field: String },
    InvalidValue { entry: usize, field: String },
    UnknownField { entry: usize, field: String },
    OpaqueOperation { entry: usize, native_type: i32 },
    EntryLimitReached,
    LineLimitReached,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DreamcastCheatIdentity {
    pub verified_product_code: String,
    pub disc_number: u8,
    pub source_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastCheatSelection {
    pub description: String,
    pub entry: FlycastCheatEntry,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlycastCheatReadiness {
    Ready,
    IdentityUnverified,
    WrongDisc,
    StaleDestination,
    Conflict,
    Malformed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlycastCheatConflictKind {
    IdentityUnverified,
    WrongDisc,
    DuplicateCheat,
    InvalidEntry,
    OpaqueOperation,
    MalformedFile,
    StaleDestination,
    UnsafePath,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastCheatConflict {
    pub kind: FlycastCheatConflictKind,
    pub detail: String,
}

#[derive(Debug)]
pub struct FlycastCheatPreview {
    pub file: FlycastCheatFile,
    pub conflicts: Vec<FlycastCheatConflict>,
    pub readiness: FlycastCheatReadiness,
    pub destination: PathBuf,
    pub report: SharedPreviewReport,
    pub transaction_plan: SharedTransactionPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastCheatRequest {
    pub profile_id: String,
    pub cheats_root: PathBuf,
    pub staging_root: PathBuf,
    pub identity: DreamcastCheatIdentity,
    pub destination: PathBuf,
    pub expected_destination_sha256: Option<String>,
    pub existing_or_imported: Vec<FlycastCheatEntry>,
    pub shared_across_discs: bool,
}

#[derive(Debug, Clone)]
pub struct FlycastCheatApplyOptions {
    pub general_approved: bool,
    pub replacement_approved: bool,
    pub operation_id: String,
    pub timestamp_unix_seconds: u64,
    pub history_root: PathBuf,
    pub backup_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlycastCheatError {
    Io(String),
    TooLarge,
    InvalidUtf8,
    UnsafePath,
    InvalidIdentity,
    WrongDisc,
    Conflict(Vec<FlycastCheatConflict>),
    StaleDestination {
        expected: String,
        actual: Option<String>,
    },
    Shared(String),
}

impl fmt::Display for FlycastCheatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for FlycastCheatError {}

pub fn parse_flycast_cheat_file(bytes: &[u8]) -> Result<FlycastCheatFile, FlycastCheatError> {
    if bytes.len() > FLYCAST_CHEAT_MAX_BYTES {
        return Err(FlycastCheatError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| FlycastCheatError::InvalidUtf8)?;
    let mut groups: BTreeMap<usize, BTreeMap<String, String>> = BTreeMap::new();
    let mut preserved_lines = Vec::new();
    let mut warnings = Vec::new();
    let mut declared_count = None;
    for (line_index, raw) in text.lines().enumerate() {
        let line = line_index + 1;
        if line > FLYCAST_CHEAT_MAX_LINES {
            warnings.push(FlycastCheatParseIssue::LineLimitReached);
            break;
        }
        if raw.len() > FLYCAST_CHEAT_MAX_LINE_BYTES {
            warnings.push(FlycastCheatParseIssue::LineTooLong { line });
            continue;
        }
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            preserved_lines.push(raw.to_string());
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            warnings.push(FlycastCheatParseIssue::MalformedLine { line });
            preserved_lines.push(raw.to_string());
            continue;
        };
        let key = key.trim().to_string();
        let value = value.trim().to_string();
        if key.eq_ignore_ascii_case("cheats") {
            declared_count = value.parse::<usize>().ok();
            continue;
        }
        if let Some((prefix, suffix)) = key.split_once('_')
            && let Some(index) = prefix
                .strip_prefix("cheat")
                .and_then(|v| v.parse::<usize>().ok())
            && !suffix.is_empty()
        {
            let group = groups.entry(index).or_default();
            if group.insert(suffix.to_string(), value).is_some() {
                warnings.push(FlycastCheatParseIssue::DuplicateField {
                    entry: index,
                    field: suffix.to_string(),
                });
            }
            continue;
        }
        preserved_lines.push(raw.to_string());
    }
    if groups.len() > FLYCAST_CHEAT_MAX_ENTRIES {
        return Err(FlycastCheatError::Conflict(vec![FlycastCheatConflict {
            kind: FlycastCheatConflictKind::MalformedFile,
            detail: "Flycast cheat entry limit reached".into(),
        }]));
    }
    let mut entries = Vec::new();
    for (index, fields) in groups {
        let (entry, mut issues) = entry_from_fields(index, fields);
        warnings.append(&mut issues);
        entries.push(entry);
    }
    Ok(FlycastCheatFile {
        entries,
        preserved_lines,
        declared_count,
        warnings,
    })
}

pub fn build_flycast_cheat_preview(
    request: &FlycastCheatRequest,
) -> Result<FlycastCheatPreview, FlycastCheatError> {
    validate_identity(&request.identity)?;
    ensure_safe_path(&request.cheats_root)?;
    ensure_safe_path(&request.destination)?;
    if request.identity.disc_number > 1 && !request.shared_across_discs {
        return Err(FlycastCheatError::WrongDisc);
    }
    if request.destination.parent() != Some(request.cheats_root.as_path())
        || request.destination.file_name().and_then(|v| v.to_str())
            != Some(&format!("{}.cht", request.identity.verified_product_code))
    {
        return Err(FlycastCheatError::UnsafePath);
    }
    let existing = read_bounded(&request.destination)?;
    if let Some(expected) = &request.expected_destination_sha256 {
        let actual = (!existing.is_empty()).then(|| sha256_hex(&existing));
        if actual.as_deref() != Some(expected) {
            return Err(FlycastCheatError::StaleDestination {
                expected: expected.clone(),
                actual,
            });
        }
    }
    let mut file = if existing.is_empty() {
        FlycastCheatFile {
            entries: Vec::new(),
            preserved_lines: Vec::new(),
            declared_count: None,
            warnings: Vec::new(),
        }
    } else {
        parse_flycast_cheat_file(&existing)?
    };
    let mut conflicts = file
        .warnings
        .iter()
        .filter_map(|issue| {
            matches!(
                issue,
                FlycastCheatParseIssue::OpaqueOperation { .. }
                    | FlycastCheatParseIssue::MalformedLine { .. }
            )
            .then_some(FlycastCheatConflict {
                kind: FlycastCheatConflictKind::MalformedFile,
                detail: format!("Flycast file contains {issue:?}"),
            })
        })
        .collect::<Vec<_>>();
    let mut fingerprints = BTreeSet::new();
    for entry in &file.entries {
        fingerprints.insert(entry_fingerprint(entry));
    }
    for imported in &request.existing_or_imported {
        let fingerprint = entry_fingerprint(imported);
        if !fingerprints.insert(fingerprint) {
            conflicts.push(FlycastCheatConflict {
                kind: FlycastCheatConflictKind::DuplicateCheat,
                detail: format!("duplicate Flycast cheat: {}", imported.description),
            });
            continue;
        }
        let mut entry = imported.clone();
        entry.index = file.entries.len();
        file.entries.push(entry);
    }
    if file
        .entries
        .iter()
        .any(|entry| matches!(entry.code, DreamcastCheatCode::Opaque { .. }))
    {
        conflicts.push(FlycastCheatConflict {
            kind: FlycastCheatConflictKind::OpaqueOperation,
            detail: "one or more Flycast operations are preserved but not interpreted".into(),
        });
    }
    if conflicts
        .iter()
        .any(|conflict| !matches!(conflict.kind, FlycastCheatConflictKind::OpaqueOperation))
    {
        return Err(FlycastCheatError::Conflict(conflicts));
    }
    let rendered = render_flycast_cheat_file(&file)?;
    fs::create_dir_all(&request.staging_root).map_err(io_error)?;
    let staged = request.staging_root.join("flycast-cheats.cht");
    fs::write(&staged, rendered).map_err(io_error)?;
    let report = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::Flycast,
        selected_archive: request.destination.clone(),
        platform: Some("Dreamcast".into()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::DreamcastProductCode,
            state: PreviewIdentityState::Verified,
            value: Some(request.identity.verified_product_code.clone()),
            archive_path: request.destination.clone(),
            revision: Some(request.identity.disc_number as u16),
        },
        destination_root: request.cheats_root.clone(),
        source_items: vec![PreviewSourceItem {
            adapter: PreviewAdapter::Flycast,
            source_path: staged,
            expected_source_digest: None,
            destination_relative_paths: vec![PathBuf::from(format!(
                "{}.cht",
                request.identity.verified_product_code
            ))],
            match_strength: PreviewMatchStrength::VerifiedExact,
        }],
    })
    .map_err(|error| FlycastCheatError::Shared(format!("{error:?}")))?;
    let transaction_plan = build_shared_transaction_plan(
        &report,
        &request.profile_id,
        "flycast-native-cheat",
        &request.staging_root,
    )
    .map_err(|error| FlycastCheatError::Shared(format!("{error:?}")))?;
    Ok(FlycastCheatPreview {
        file,
        conflicts,
        readiness: FlycastCheatReadiness::Ready,
        destination: request.destination.clone(),
        report,
        transaction_plan,
    })
}

pub fn apply_flycast_cheat_preview(
    preview: &FlycastCheatPreview,
    options: &FlycastCheatApplyOptions,
) -> Result<SharedApplyResult, FlycastCheatError> {
    if preview.readiness != FlycastCheatReadiness::Ready {
        return Err(FlycastCheatError::Conflict(vec![]));
    }
    let plan = &preview.transaction_plan;
    Ok(execute_shared_apply(
        plan,
        &SharedApplyOptions {
            dry_run: false,
            confirmation: Some(SharedApplyConfirmation {
                plan_id: plan.plan_id.clone(),
                general_approved: options.general_approved,
                replacement_approved: options.replacement_approved,
            }),
            operation_id: options.operation_id.clone(),
            timestamp_unix_seconds: options.timestamp_unix_seconds,
            current_context: plan.context.clone(),
            history_root: options.history_root.clone(),
            backup_root: options.backup_root.clone(),
        },
    ))
}

fn entry_from_fields(
    index: usize,
    fields: BTreeMap<String, String>,
) -> (FlycastCheatEntry, Vec<FlycastCheatParseIssue>) {
    let mut issues = Vec::new();
    let description = fields
        .get("desc")
        .cloned()
        .unwrap_or_else(|| format!("Cheat {}", index + 1));
    let enabled = fields
        .get("enable")
        .and_then(|v| parse_bool(v))
        .unwrap_or(false);
    let native = fields
        .get("cheat_type")
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(0);
    let cheat_type = match native {
        0 => FlycastCheatType::Disabled,
        1 => FlycastCheatType::SetValue,
        2 => FlycastCheatType::Increase,
        3 => FlycastCheatType::Decrease,
        4 => FlycastCheatType::ConditionalEqual,
        5 => FlycastCheatType::ConditionalNotEqual,
        6 => FlycastCheatType::ConditionalGreater,
        7 => FlycastCheatType::ConditionalLess,
        8 => FlycastCheatType::Copy,
        other => {
            issues.push(FlycastCheatParseIssue::OpaqueOperation {
                entry: index,
                native_type: other,
            });
            FlycastCheatType::Opaque(other)
        }
    };
    let code = if cheat_type == FlycastCheatType::SetValue {
        let address = fields.get("address").and_then(|v| parse_number(v));
        let width = fields
            .get("memory_search_size")
            .and_then(|v| v.parse::<u32>().ok())
            .and_then(|shift| 1u32.checked_shl(shift));
        let value = fields.get("value").and_then(|v| parse_number(v));
        match (address, width, value) {
            (Some(address), Some(width @ (1 | 2 | 4)), Some(value)) => {
                DreamcastCheatCode::DirectWrite {
                    address,
                    width,
                    value,
                }
            }
            _ => {
                issues.push(FlycastCheatParseIssue::InvalidValue {
                    entry: index,
                    field: "address/value/width".into(),
                });
                DreamcastCheatCode::Opaque {
                    native_type: cheat_type,
                }
            }
        }
    } else {
        DreamcastCheatCode::Opaque {
            native_type: cheat_type,
        }
    };
    (
        FlycastCheatEntry {
            index,
            description,
            enabled,
            cheat_type,
            code,
            fields,
        },
        issues,
    )
}

fn render_flycast_cheat_file(file: &FlycastCheatFile) -> Result<Vec<u8>, FlycastCheatError> {
    let mut lines = file.preserved_lines.clone();
    if !lines.is_empty() && !lines.last().is_some_and(String::is_empty) {
        lines.push(String::new());
    }
    lines.push(format!("cheats={}", file.entries.len()));
    for (index, entry) in file.entries.iter().enumerate() {
        for (key, value) in &entry.fields {
            let value = if key == "enable" {
                entry.enabled.to_string()
            } else {
                value.clone()
            };
            lines.push(format!("cheat{index}_{key}={value}"));
        }
    }
    Ok(lines.join("\n").into_bytes())
}

fn entry_fingerprint(entry: &FlycastCheatEntry) -> String {
    entry
        .fields
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

fn validate_identity(identity: &DreamcastCheatIdentity) -> Result<(), FlycastCheatError> {
    if identity.verified_product_code.is_empty()
        || identity.disc_number == 0
        || !identity
            .verified_product_code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ' '))
    {
        return Err(FlycastCheatError::InvalidIdentity);
    }
    Ok(())
}

fn ensure_safe_path(path: &Path) -> Result<(), FlycastCheatError> {
    if !path.is_absolute()
        || path.components().any(|c| c == Component::ParentDir)
        || path.file_name().is_none()
    {
        return Err(FlycastCheatError::UnsafePath);
    }
    Ok(())
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, FlycastCheatError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(FlycastCheatError::UnsafePath)
        }
        Ok(metadata) if metadata.len() as usize > FLYCAST_CHEAT_MAX_BYTES => {
            Err(FlycastCheatError::TooLarge)
        }
        Ok(_) => fs::read(path).map_err(io_error),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(io_error(error)),
    }
}

fn parse_number(value: &str) -> Option<u32> {
    let value = value.trim();
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .and_then(|v| u32::from_str_radix(v, 16).ok())
        .or_else(|| value.parse().ok())
}
fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" => Some(true),
        "false" | "no" | "0" => Some(false),
        _ => None,
    }
}
fn width_to_shift(width: u32) -> Option<u32> {
    match width {
        1 => Some(0),
        2 => Some(1),
        4 => Some(2),
        _ => None,
    }
}
fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}
fn io_error(error: std::io::Error) -> FlycastCheatError {
    FlycastCheatError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch_manager::shared_transaction::{
        SharedApplyStatus, SharedRollbackConfirmation, SharedRollbackOptions,
        execute_shared_rollback, preview_shared_rollback,
    };
    use tempfile::tempdir;

    fn request(
        root: &Path,
        destination: &Path,
        entries: Vec<FlycastCheatEntry>,
    ) -> FlycastCheatRequest {
        FlycastCheatRequest {
            profile_id: "flycast:test".into(),
            cheats_root: root.join("cheats"),
            staging_root: root.join("staging"),
            identity: DreamcastCheatIdentity {
                verified_product_code: "T-8109N".into(),
                disc_number: 1,
                source_digest: None,
            },
            destination: destination.to_path_buf(),
            expected_destination_sha256: None,
            existing_or_imported: entries,
            shared_across_discs: false,
        }
    }

    #[test]
    fn parses_native_fields_and_normalizes_direct_write() {
        let file = parse_flycast_cheat_file(b"cheats=1\ncheat0_desc=Infinite Health\ncheat0_cheat_type=1\ncheat0_address=0x1234\ncheat0_memory_search_size=2\ncheat0_value=99\ncheat0_enable=true\n").unwrap();
        assert_eq!(file.entries.len(), 1);
        assert_eq!(
            file.entries[0].code,
            DreamcastCheatCode::DirectWrite {
                address: 0x1234,
                width: 4,
                value: 99
            }
        );
        assert!(file.entries[0].enabled);
    }

    #[test]
    fn opaque_types_are_retained_but_not_silently_normalized() {
        let file = parse_flycast_cheat_file(
            b"cheat0_desc=Unknown\ncheat0_cheat_type=99\ncheat0_enable=true\n",
        )
        .unwrap();
        assert!(matches!(
            file.entries[0].code,
            DreamcastCheatCode::Opaque { .. }
        ));
        assert!(
            file.warnings
                .iter()
                .any(|issue| matches!(issue, FlycastCheatParseIssue::OpaqueOperation { .. }))
        );
    }

    #[test]
    fn wrong_disc_and_unverified_identity_refuse() {
        let root = tempdir().unwrap();
        let cheats = root.path().join("cheats");
        fs::create_dir(&cheats).unwrap();
        let dest = cheats.join("T-8109N.cht");
        let mut request = request(
            root.path(),
            &dest,
            vec![FlycastCheatEntry::direct_write("Health", 0x100, 4, 99)],
        );
        request.identity.disc_number = 2;
        assert!(matches!(
            build_flycast_cheat_preview(&request),
            Err(FlycastCheatError::WrongDisc)
        ));
        request.identity.verified_product_code.clear();
        request.identity.disc_number = 1;
        assert!(matches!(
            build_flycast_cheat_preview(&request),
            Err(FlycastCheatError::InvalidIdentity)
        ));
    }

    #[test]
    fn deterministic_apply_preserves_unrelated_lines_and_rolls_back() {
        let root = tempdir().unwrap();
        let cheats = root.path().join("cheats");
        fs::create_dir(&cheats).unwrap();
        let dest = cheats.join("T-8109N.cht");
        fs::write(&dest, b"; user note\nother_setting=keep\n").unwrap();
        let preview = build_flycast_cheat_preview(&request(
            root.path(),
            &dest,
            vec![FlycastCheatEntry::direct_write("Health", 0x100, 4, 99)],
        ))
        .unwrap();
        let first = fs::read(
            &preview
                .transaction_plan
                .approved_source_root
                .to_path_buf()
                .unwrap()
                .join("flycast-cheats.cht"),
        )
        .unwrap();
        let second = render_flycast_cheat_file(&preview.file).unwrap();
        assert_eq!(first, second);
        let history = tempdir().unwrap();
        let backup = tempdir().unwrap();
        let applied = apply_flycast_cheat_preview(
            &preview,
            &FlycastCheatApplyOptions {
                general_approved: true,
                replacement_approved: true,
                operation_id: "flycast".into(),
                timestamp_unix_seconds: 1,
                history_root: history.path().to_path_buf(),
                backup_root: backup.path().to_path_buf(),
            },
        )
        .unwrap();
        assert_eq!(
            applied.journal.status,
            SharedApplyStatus::Success,
            "journal: {:#?}; failure: {:#?}",
            applied.journal,
            applied.journal_failure
        );
        let journal = applied.journal_path.unwrap();
        let rollback = preview_shared_rollback(&journal, cheats.as_path(), backup.path());
        assert!(rollback.available);
        let result = execute_shared_rollback(
            &rollback,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: rollback.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "rollback".into(),
                timestamp_unix_seconds: 2,
                history_root: history.path().to_path_buf(),
                backup_root: backup.path().to_path_buf(),
            },
        );
        assert_eq!(result.status, SharedApplyStatus::Success);
        assert_eq!(
            fs::read(&dest).unwrap(),
            b"; user note\nother_setting=keep\n"
        );
    }
}
