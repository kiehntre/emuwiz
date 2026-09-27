//! Native RPCS3 `patch.yml` support.
//!
//! This adapter deliberately implements a bounded, conservative subset of
//! RPCS3's YAML rather than embedding a second YAML engine.  It understands
//! the documented identity/version/game and direct patch fields, rejects
//! aliases and unsupported structures, and preserves opaque lines in the
//! parsed entry for display.  Applying always writes the user-owned
//! `patches/imported_patch.yml` and `patch_config.yml`; shipped patch data and
//! game files are never modified.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
#[cfg(test)]
use sha2::{Digest, Sha256};

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

pub const RPCS3_PATCH_SOURCE_MODE: &str = "rpcs3_native_patch";
pub const RPCS3_PATCH_ENGINE_VERSION: &str = "1.2";
pub const RPCS3_PATCH_MAX_BYTES: usize = 512 * 1024;
pub const RPCS3_PATCH_MAX_LINES: usize = 8_192;
pub const RPCS3_PATCH_MAX_ENTRIES: usize = 512;
pub const RPCS3_PATCH_MAX_OPS: usize = 4_096;
const MAX_LINE: usize = 4 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rpcs3PatchFile {
    pub version: String,
    pub groups: Vec<Rpcs3PatchGroup>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rpcs3PatchGroup {
    pub hash: String,
    pub entries: Vec<Rpcs3PatchEntryDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rpcs3PatchEntryDefinition {
    pub description: String,
    pub title: Option<String>,
    pub title_id: Option<String>,
    pub versions: Vec<String>,
    pub patch_version: Option<String>,
    pub group: Option<String>,
    pub operations: Vec<Rpcs3PatchOperation>,
    pub opaque: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rpcs3PatchOperation {
    pub kind: String,
    pub offset: String,
    pub value: String,
    pub understood: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rpcs3PatchState {
    Enabled,
    Disabled,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rpcs3PatchIssue {
    pub line: Option<usize>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rpcs3PatchReadiness {
    ExactCompatible,
    AllVersionsCompatible,
    UnknownVersion,
    IncompatibleVersion,
    WrongTitleId,
    OpaqueOperation,
    Malformed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3PatchSelection {
    pub hash: String,
    pub entry: Rpcs3PatchEntryDefinition,
    pub state: Rpcs3PatchState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3PatchPlan {
    pub report: SharedPreviewReport,
    pub transaction: SharedTransactionPlan,
    pub patch_bytes: Vec<u8>,
    pub config_bytes: Vec<u8>,
    pub readiness: Rpcs3PatchReadiness,
    pub imported_patch_path: PathBuf,
    pub config_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rpcs3PatchError {
    MissingIdentity,
    IdentityMismatch,
    UnsupportedVersion(String),
    Malformed(String),
    ResourceLimit(String),
    UnsafePath(String),
    Preview(String),
}

impl std::fmt::Display for Rpcs3PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Rpcs3PatchError {}

fn scalar(value: &str) -> String {
    let value = value.trim().trim_end_matches(':').trim();
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(value)
        .replace("\\\"", "\"")
}

fn mapping(line: &str) -> Option<(usize, String, String)> {
    let indent = line.bytes().take_while(|byte| *byte == b' ').count();
    let trimmed = line[indent..].trim_end();
    let colon = trimmed.find(':')?;
    let key = scalar(&trimmed[..colon]);
    let value = scalar(&trimmed[colon + 1..]);
    (!key.is_empty()).then_some((indent, key, value))
}

fn parse_operation(line: &str) -> Option<Rpcs3PatchOperation> {
    let start = line.find('[')?;
    let end = line.rfind(']')?;
    let parts = line[start + 1..end]
        .split(',')
        .map(scalar)
        .collect::<Vec<_>>();
    if parts.len() < 3 || parts.iter().any(String::is_empty) {
        return None;
    }
    let kind = parts[0].clone();
    let understood = matches!(
        kind.as_str(),
        "byte" | "le16" | "be16" | "le32" | "be32" | "bd32"
    );
    Some(Rpcs3PatchOperation {
        kind,
        offset: parts[1].clone(),
        value: parts[2].clone(),
        understood,
    })
}

/// Parse the documented RPCS3 patch structure with strict resource limits.
pub fn parse_rpcs3_patch_yaml(bytes: &[u8]) -> Result<Rpcs3PatchFile, Rpcs3PatchIssue> {
    if bytes.len() > RPCS3_PATCH_MAX_BYTES {
        return Err(Rpcs3PatchIssue {
            line: None,
            message: "patch file exceeds the bounded size".into(),
        });
    }
    let text = std::str::from_utf8(bytes).map_err(|_| Rpcs3PatchIssue {
        line: None,
        message: "patch file is not UTF-8".into(),
    })?;
    let lines = text.lines().collect::<Vec<_>>();
    if lines.len() > RPCS3_PATCH_MAX_LINES {
        return Err(Rpcs3PatchIssue {
            line: None,
            message: "patch file exceeds the bounded line count".into(),
        });
    }
    let mut version = None;
    let mut groups = Vec::new();
    let mut group: Option<Rpcs3PatchGroup> = None;
    let mut entry: Option<Rpcs3PatchEntryDefinition> = None;
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut in_patch = false;
    let mut op_count = 0;

    let flush_entry = |group: &mut Option<Rpcs3PatchGroup>,
                       entry: &mut Option<Rpcs3PatchEntryDefinition>| {
        if let Some(value) = entry.take() {
            if let Some(group) = group {
                group.entries.push(value);
            }
        }
    };
    let flush_group = |groups: &mut Vec<Rpcs3PatchGroup>, group: &mut Option<Rpcs3PatchGroup>| {
        if let Some(value) = group.take() {
            groups.push(value);
        }
    };

    for (line_number, raw) in lines.iter().enumerate() {
        if raw.len() > MAX_LINE || raw.contains('\t') {
            return Err(Rpcs3PatchIssue {
                line: Some(line_number + 1),
                message: "tabs or an oversized line are not supported".into(),
            });
        }
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed == "---" {
            continue;
        }
        if trimmed.contains("&") || trimmed.contains("*") {
            return Err(Rpcs3PatchIssue {
                line: Some(line_number + 1),
                message: "YAML anchors and aliases are refused".into(),
            });
        }
        if trimmed.starts_with('-') {
            if in_patch {
                let op = parse_operation(trimmed).ok_or_else(|| Rpcs3PatchIssue {
                    line: Some(line_number + 1),
                    message: "malformed patch operation".into(),
                })?;
                op_count += 1;
                if op_count > RPCS3_PATCH_MAX_OPS {
                    return Err(Rpcs3PatchIssue {
                        line: Some(line_number + 1),
                        message: "patch operation limit reached".into(),
                    });
                }
                if let Some(entry) = entry.as_mut() {
                    entry.operations.push(op);
                }
            } else if let Some(entry) = entry.as_mut() {
                let version = scalar(trimmed.trim_start_matches('-'));
                if !version.is_empty() && stack.iter().any(|(_, name)| name == "Games") {
                    let game_keys = stack
                        .iter()
                        .filter(|(_, name)| name != "Games")
                        .map(|(_, name)| name.clone())
                        .collect::<Vec<_>>();
                    if entry.title.is_none() {
                        entry.title = game_keys.first().cloned();
                    }
                    if entry.title_id.is_none() {
                        entry.title_id = game_keys.get(1).cloned();
                    }
                    entry.versions.push(version);
                } else {
                    entry.opaque = true;
                }
            }
            continue;
        }
        let Some((indent, key, value)) = mapping(raw) else {
            return Err(Rpcs3PatchIssue {
                line: Some(line_number + 1),
                message: "unsupported YAML mapping".into(),
            });
        };
        if indent > 64 {
            return Err(Rpcs3PatchIssue {
                line: Some(line_number + 1),
                message: "YAML nesting is too deep".into(),
            });
        }
        while stack.last().is_some_and(|(level, _)| *level >= indent) {
            stack.pop();
        }
        if indent == 0 && key == "Version" {
            version = Some(value);
            continue;
        }
        if indent == 0 {
            flush_entry(&mut group, &mut entry);
            flush_group(&mut groups, &mut group);
            group = Some(Rpcs3PatchGroup {
                hash: key,
                entries: Vec::new(),
            });
            stack.clear();
            continue;
        }
        if indent == 2 {
            flush_entry(&mut group, &mut entry);
            entry = Some(Rpcs3PatchEntryDefinition {
                description: key.clone(),
                title: None,
                title_id: None,
                versions: Vec::new(),
                patch_version: None,
                group: None,
                operations: Vec::new(),
                opaque: false,
            });
            in_patch = false;
            stack.push((indent, key));
            continue;
        }
        if let Some(current) = entry.as_mut() {
            match key.as_str() {
                "Group" => current.group = (!value.is_empty()).then_some(value),
                "Patch Version" => current.patch_version = (!value.is_empty()).then_some(value),
                "Patch" => {
                    in_patch = true;
                }
                "Games" | "Author" | "Notes" | "Config" => {}
                _ if stack.iter().any(|(_, name)| name == "Games") && value.is_empty() => {
                    let game_keys = stack
                        .iter()
                        .filter(|(_, name)| name != "Games")
                        .map(|(_, name)| name.clone())
                        .collect::<Vec<_>>();
                    if current.title.is_none() {
                        current.title = Some(key.clone());
                    } else if current.title_id.is_none() {
                        current.title_id = Some(key.clone());
                    } else if key == "all" || key.chars().any(|c| c.is_ascii_digit()) {
                        current.versions.push(key.clone());
                    }
                    if current.title.is_none() {
                        current.title = game_keys.first().cloned();
                    }
                    if current.title_id.is_none() {
                        current.title_id = game_keys.get(1).cloned();
                    }
                }
                _ => current.opaque = true,
            }
        }
        stack.push((indent, key));
    }
    flush_entry(&mut group, &mut entry);
    flush_group(&mut groups, &mut group);
    let version = version.ok_or_else(|| Rpcs3PatchIssue {
        line: None,
        message: "RPCS3 patch Version is required".into(),
    })?;
    if groups
        .iter()
        .map(|group| group.entries.len())
        .sum::<usize>()
        > RPCS3_PATCH_MAX_ENTRIES
    {
        return Err(Rpcs3PatchIssue {
            line: None,
            message: "patch entry limit reached".into(),
        });
    }
    if groups.is_empty() {
        return Err(Rpcs3PatchIssue {
            line: None,
            message: "patch file contains no patch groups".into(),
        });
    }
    Ok(Rpcs3PatchFile { version, groups })
}

fn valid_title_id(value: &str) -> bool {
    value.len() == 9 && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

pub fn readiness(
    entry: &Rpcs3PatchEntryDefinition,
    title_id: &str,
    app_version: &str,
) -> Rpcs3PatchReadiness {
    if !valid_title_id(title_id)
        || entry
            .title_id
            .as_deref()
            .is_some_and(|value| value != title_id)
    {
        return Rpcs3PatchReadiness::WrongTitleId;
    }
    if entry
        .operations
        .iter()
        .any(|operation| !operation.understood)
    {
        return Rpcs3PatchReadiness::OpaqueOperation;
    }
    if entry.versions.iter().any(|version| version == app_version) {
        Rpcs3PatchReadiness::ExactCompatible
    } else if entry.versions.iter().any(|version| version == "all") {
        Rpcs3PatchReadiness::AllVersionsCompatible
    } else if entry.versions.is_empty() {
        Rpcs3PatchReadiness::UnknownVersion
    } else {
        Rpcs3PatchReadiness::IncompatibleVersion
    }
}

fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\\\""))
}

pub fn render_rpcs3_patch_yaml(file: &Rpcs3PatchFile) -> Vec<u8> {
    let mut out = format!("Version: {}\n", file.version);
    for group in &file.groups {
        out.push_str(&format!("\n{}:\n", quote(&group.hash)));
        for entry in &group.entries {
            out.push_str(&format!("  {}:\n", quote(&entry.description)));
            if let Some(title_id) = &entry.title_id {
                out.push_str("    Games:\n");
                out.push_str(&format!(
                    "      {}:\n        {}:\n",
                    quote(entry.title.as_deref().unwrap_or(title_id)),
                    quote(title_id)
                ));
                for version in &entry.versions {
                    out.push_str(&format!("          - {}\n", quote(version)));
                }
            }
            if let Some(value) = &entry.patch_version {
                out.push_str(&format!("    Patch Version: {}\n", quote(value)));
            }
            if let Some(value) = &entry.group {
                out.push_str(&format!("    Group: {}\n", quote(value)));
            }
            out.push_str("    Patch:\n");
            for operation in &entry.operations {
                out.push_str(&format!(
                    "      - [{}, {}, {}]\n",
                    operation.kind, operation.offset, operation.value
                ));
            }
        }
    }
    out.into_bytes()
}

/// Merge local patch definitions without replacing unrelated entries.
/// Identical definitions are deduplicated; conflicting definitions are
/// refused so the user can review them explicitly.
pub fn merge_rpcs3_patch_files(
    existing: &Rpcs3PatchFile,
    incoming: &Rpcs3PatchFile,
) -> Result<Rpcs3PatchFile, Rpcs3PatchError> {
    if existing.version != incoming.version {
        return Err(Rpcs3PatchError::UnsupportedVersion(format!(
            "cannot merge patch versions {} and {}",
            existing.version, incoming.version
        )));
    }
    let mut merged = existing.clone();
    for incoming_group in &incoming.groups {
        let group = if let Some(group) = merged
            .groups
            .iter_mut()
            .find(|group| group.hash == incoming_group.hash)
        {
            group
        } else {
            merged.groups.push(Rpcs3PatchGroup {
                hash: incoming_group.hash.clone(),
                entries: Vec::new(),
            });
            merged.groups.last_mut().expect("just pushed")
        };
        for incoming_entry in &incoming_group.entries {
            if let Some(existing_entry) = group
                .entries
                .iter()
                .find(|entry| entry.description == incoming_entry.description)
            {
                if existing_entry != incoming_entry {
                    return Err(Rpcs3PatchError::Malformed(format!(
                        "conflicting RPCS3 patch definition: {}",
                        incoming_entry.description
                    )));
                }
            } else {
                group.entries.push(incoming_entry.clone());
            }
        }
    }
    Ok(merged)
}

#[cfg(test)]
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn safe_root(root: &Path) -> Result<(), Rpcs3PatchError> {
    if !root.is_absolute()
        || root
            .components()
            .any(|component| component == std::path::Component::ParentDir)
    {
        return Err(Rpcs3PatchError::UnsafePath(root.display().to_string()));
    }
    Ok(())
}

/// Build a transaction from a generated imported patch and patch-config file.
/// The caller supplies an EmuWiz-managed staging directory; the destination is
/// the RPCS3 profile config root. Existing patch DB files are never selected.
pub fn build_rpcs3_patch_plan(
    staging_root: &Path,
    configuration_root: &Path,
    selected_archive: &Path,
    profile_id: &str,
    title_id: &str,
    app_version: &str,
    file: &Rpcs3PatchFile,
    state: Rpcs3PatchState,
) -> Result<Rpcs3PatchPlan, Rpcs3PatchError> {
    safe_root(staging_root)?;
    safe_root(configuration_root)?;
    if !valid_title_id(title_id) {
        return Err(Rpcs3PatchError::MissingIdentity);
    }
    if file.version != RPCS3_PATCH_ENGINE_VERSION {
        return Err(Rpcs3PatchError::UnsupportedVersion(file.version.clone()));
    }
    if file
        .groups
        .iter()
        .flat_map(|group| group.entries.iter())
        .any(|entry| readiness(entry, title_id, app_version) == Rpcs3PatchReadiness::WrongTitleId)
    {
        return Err(Rpcs3PatchError::IdentityMismatch);
    }
    let patch_bytes = render_rpcs3_patch_yaml(file);
    let config_bytes = format!(
        "{}:\n  {}:\n    {}:\n      {}:\n        {}:\n          Enabled: {}\n",
        file.groups[0].hash,
        file.groups[0].entries[0].description,
        file.groups[0].entries[0]
            .title
            .as_deref()
            .unwrap_or(title_id),
        title_id,
        app_version,
        matches!(state, Rpcs3PatchState::Enabled)
    )
    .into_bytes();
    fs::create_dir_all(staging_root)
        .map_err(|error| Rpcs3PatchError::Preview(error.to_string()))?;
    let patch_path = staging_root.join("patches").join("imported_patch.yml");
    let config_path = staging_root.join("patch_config.yml");
    fs::create_dir_all(patch_path.parent().unwrap())
        .map_err(|error| Rpcs3PatchError::Preview(error.to_string()))?;
    fs::write(&patch_path, &patch_bytes)
        .map_err(|error| Rpcs3PatchError::Preview(error.to_string()))?;
    fs::write(&config_path, &config_bytes)
        .map_err(|error| Rpcs3PatchError::Preview(error.to_string()))?;
    parse_rpcs3_patch_yaml(&patch_bytes)
        .map_err(|issue| Rpcs3PatchError::Malformed(issue.message))?;
    let source_items = [
        (
            patch_path.clone(),
            PathBuf::from("patches/imported_patch.yml"),
        ),
        (config_path.clone(), PathBuf::from("patch_config.yml")),
    ]
    .into_iter()
    .map(|(source_path, relative)| PreviewSourceItem {
        adapter: PreviewAdapter::Rpcs3Patch,
        source_path,
        expected_source_digest: None,
        destination_relative_paths: vec![relative],
        match_strength: PreviewMatchStrength::VerifiedExact,
    })
    .collect();
    let report = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::Rpcs3Patch,
        selected_archive: selected_archive.to_path_buf(),
        platform: Some("PS3".into()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::Rpcs3TitleId,
            state: PreviewIdentityState::Verified,
            value: Some(title_id.into()),
            archive_path: selected_archive.to_path_buf(),
            revision: None,
        },
        destination_root: configuration_root.to_path_buf(),
        source_items,
    })
    .map_err(|error| Rpcs3PatchError::Preview(error.to_string()))?;
    let transaction =
        build_shared_transaction_plan(&report, profile_id, RPCS3_PATCH_SOURCE_MODE, staging_root)
            .map_err(|error| Rpcs3PatchError::Preview(error.detail))?;
    let readiness = file
        .groups
        .iter()
        .flat_map(|group| group.entries.iter())
        .map(|entry| readiness(entry, title_id, app_version))
        .next()
        .unwrap_or(Rpcs3PatchReadiness::Malformed);
    Ok(Rpcs3PatchPlan {
        report,
        transaction,
        patch_bytes,
        config_bytes,
        readiness,
        imported_patch_path: configuration_root.join("patches/imported_patch.yml"),
        config_path: configuration_root.join("patch_config.yml"),
    })
}

pub fn apply_rpcs3_patch(plan: &Rpcs3PatchPlan, options: &SharedApplyOptions) -> SharedApplyResult {
    execute_shared_apply(&plan.transaction, options)
}

pub fn preview_rpcs3_patch_rollback(
    journal: &Path,
    root: &Path,
    backup: &Path,
) -> super::shared_transaction::SharedRollbackPreview {
    preview_shared_rollback(journal, root, backup)
}

pub fn rollback_rpcs3_patch(
    preview: &super::shared_transaction::SharedRollbackPreview,
    options: &SharedRollbackOptions,
) -> SharedRollbackResult {
    execute_shared_rollback(preview, options)
}

pub fn rpcs3_patch_loadability_facts(
    configuration_root: &Path,
    title_id: &str,
    app_version: Option<&str>,
) -> Option<Rpcs3PatchLoadabilityFacts> {
    valid_title_id(title_id).then(|| Rpcs3PatchLoadabilityFacts {
        patch_path: configuration_root.join("patches/imported_patch.yml"),
        config_path: configuration_root.join("patch_config.yml"),
        title_id: title_id.into(),
        app_version: app_version.map(str::to_owned),
        restart_required: true,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rpcs3PatchLoadabilityFacts {
    pub patch_path: PathBuf,
    pub config_path: PathBuf,
    pub title_id: String,
    pub app_version: Option<String>,
    pub restart_required: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML: &str = "Version: 1.2\n\nPPU-123:\n  \"60 FPS\":\n    Games:\n      \"Example\":\n        \"BLUS12345\":\n          \"01.03\":\n    Group: \"Performance\"\n    Patch:\n      - [ be32, 0x00100000, 0x3f800000 ]\n";

    #[test]
    fn parses_identity_version_and_direct_write() {
        let file = parse_rpcs3_patch_yaml(YAML.as_bytes()).unwrap();
        let entry = &file.groups[0].entries[0];
        assert_eq!(entry.title_id.as_deref(), Some("BLUS12345"));
        assert_eq!(
            readiness(entry, "BLUS12345", "01.03"),
            Rpcs3PatchReadiness::ExactCompatible
        );
        assert!(entry.operations[0].understood);
    }

    #[test]
    fn wrong_title_and_version_are_blocked() {
        let entry = &parse_rpcs3_patch_yaml(YAML.as_bytes()).unwrap().groups[0].entries[0];
        assert_eq!(
            readiness(entry, "BLES54321", "01.03"),
            Rpcs3PatchReadiness::WrongTitleId
        );
        assert_eq!(
            readiness(entry, "BLUS12345", "01.04"),
            Rpcs3PatchReadiness::IncompatibleVersion
        );
    }

    #[test]
    fn opaque_operation_is_retained_and_aliases_refused() {
        let opaque = YAML.replace("be32", "jump");
        let entry = &parse_rpcs3_patch_yaml(opaque.as_bytes()).unwrap().groups[0].entries[0];
        assert_eq!(
            readiness(entry, "BLUS12345", "01.03"),
            Rpcs3PatchReadiness::OpaqueOperation
        );
        assert!(parse_rpcs3_patch_yaml(b"Version: 1.2\nA: &anchor\n").is_err());
    }

    #[test]
    fn rendering_is_deterministic_and_bounded() {
        let file = parse_rpcs3_patch_yaml(YAML.as_bytes()).unwrap();
        assert_eq!(
            render_rpcs3_patch_yaml(&file),
            render_rpcs3_patch_yaml(&file)
        );
        assert!(!digest(&render_rpcs3_patch_yaml(&file)).is_empty());
    }

    #[test]
    fn plan_targets_only_user_patch_files() {
        let file = parse_rpcs3_patch_yaml(YAML.as_bytes()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let staging = temp.path().join("staging");
        let config = temp.path().join("rpcs3");
        fs::create_dir_all(&config).unwrap();
        let plan = build_rpcs3_patch_plan(
            &staging,
            &config,
            &temp.path().join("game.iso"),
            "profile",
            "BLUS12345",
            "01.03",
            &file,
            Rpcs3PatchState::Enabled,
        )
        .unwrap();
        assert_eq!(plan.readiness, Rpcs3PatchReadiness::ExactCompatible);
        assert_eq!(plan.transaction.entries.len(), 2);
        assert!(plan.transaction.entries.iter().all(|entry| {
            let path = entry.destination_relative_path.to_path_buf().unwrap();
            path == PathBuf::from("patch_config.yml")
                || path == PathBuf::from("patches/imported_patch.yml")
        }));
        assert!(plan.patch_bytes.starts_with(b"Version: 1.2"));
        assert!(
            plan.config_bytes
                .windows(b"Enabled: true".len())
                .any(|window| window == b"Enabled: true")
        );
    }
}
