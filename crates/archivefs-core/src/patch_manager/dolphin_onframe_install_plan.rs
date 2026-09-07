//! Safe planning and staging for native Dolphin `[OnFrame]` patches.
//!
//! This module deliberately does not interpret or rewrite Action Replay or
//! Gecko sections.  It plans one complete, timing-explicit OnFrame document
//! and hands the resulting staged INI to the shared Dolphin transaction
//! machinery for preview, atomic apply, journaling, backup, and rollback.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::cheat_ir::{CheatDocument, CheatPlatform, CheatSourceFormat, encode_operation};
use super::gecko_document::{parse_dolphin_ini, replace_named_section};
use super::shared_preview::{
    PreviewAdapter, PreviewIdentity, PreviewIdentityKind, PreviewIdentityState,
    PreviewMatchStrength, PreviewSourceItem, SharedPreviewReport, SharedPreviewRequest,
    build_shared_preview,
};

pub const MAX_GENERATED_ONFRAME_INI_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DolphinOnFrameInstallStatus {
    Ready,
    AlreadyInstalled,
    EnabledExisting,
    Conflict,
    Refused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DolphinOnFrameInstallPlan {
    pub game_id: String,
    pub revision: Option<u16>,
    pub destination_file_name: String,
    pub patch_name: String,
    pub lines: Vec<String>,
    pub new_contents: String,
    pub status: DolphinOnFrameInstallStatus,
    pub can_apply: bool,
    pub conflicts: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DolphinOnFrameInstallRequest<'a> {
    pub game_id: &'a str,
    pub revision: Option<u16>,
    pub document: &'a CheatDocument,
    pub existing_contents: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct StagedDolphinOnFrameIni {
    pub staging_root: PathBuf,
    pub path: PathBuf,
    pub digest: String,
    pub contents: String,
    pub destination_existed: bool,
    pub destination_file_name: String,
}

#[derive(Debug, Clone)]
pub struct DolphinOnFrameInstallPreviewRequest {
    pub selected_archive: PathBuf,
    pub configuration_path: PathBuf,
    pub game_id: String,
    pub revision: Option<u16>,
    pub staged: StagedDolphinOnFrameIni,
}

#[derive(Debug, Clone)]
pub struct DolphinOnFrameInstallPreview {
    pub report: SharedPreviewReport,
    pub staged: StagedDolphinOnFrameIni,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DolphinOnFrameInstallPlanError {
    InvalidIdentity,
    InvalidTitle,
    UnsupportedDocument(String),
    ExistingDocumentMalformed,
    GeneratedFileTooLarge,
    StagingUnavailable(String),
    PreviewFailed(String),
}

impl std::fmt::Display for DolphinOnFrameInstallPlanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdentity => formatter.write_str("Dolphin game identity is invalid"),
            Self::InvalidTitle => formatter.write_str("OnFrame patch title is invalid"),
            Self::UnsupportedDocument(detail)
            | Self::StagingUnavailable(detail)
            | Self::PreviewFailed(detail) => formatter.write_str(detail),
            Self::ExistingDocumentMalformed => {
                formatter.write_str("existing Dolphin INI could not be safely parsed")
            }
            Self::GeneratedFileTooLarge => {
                formatter.write_str("generated Dolphin INI is too large")
            }
        }
    }
}

impl std::error::Error for DolphinOnFrameInstallPlanError {}

fn identity_file_name(game_id: &str, revision: Option<u16>) -> Option<String> {
    let game_id = game_id.trim();
    if !(3..=6).contains(&game_id.len())
        || !game_id.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return None;
    }
    let game_id = game_id.to_ascii_uppercase();
    Some(match revision {
        Some(revision) => format!("{game_id}r{revision}.ini"),
        None => format!("{game_id}.ini"),
    })
}

fn valid_patch_title(title: &str) -> bool {
    let title = title.trim();
    !title.is_empty() && !title.contains(['\n', '\r', '$', '='])
}

fn onframe_lines(document: &CheatDocument) -> Result<Vec<String>, DolphinOnFrameInstallPlanError> {
    if !matches!(
        document.platform,
        CheatPlatform::GameCube | CheatPlatform::Wii
    ) || document.source_format != CheatSourceFormat::DolphinOnFrame
    {
        return Err(DolphinOnFrameInstallPlanError::UnsupportedDocument(
            "only a GameCube/Wii DolphinOnFrame document is installable".into(),
        ));
    }
    if !document.issues.is_empty() || document.operations.is_empty() {
        return Err(DolphinOnFrameInstallPlanError::UnsupportedDocument(
            "document contains warnings or no operations".into(),
        ));
    }
    document
        .operations
        .iter()
        .map(|operation| {
            encode_operation(
                operation,
                &super::cheat_ir::CheatTargetFormat::DolphinOnFrame,
            )
            .ok_or_else(|| {
                DolphinOnFrameInstallPlanError::UnsupportedDocument(
                    "every operation must be an exact OnFrame write".into(),
                )
            })
        })
        .collect()
}

#[derive(Debug, Clone)]
struct ExistingPatch {
    name: String,
    lines: Vec<String>,
}

fn parse_onframe_patches(lines: &[String]) -> Vec<ExistingPatch> {
    let mut patches = Vec::new();
    let mut current: Option<ExistingPatch> = None;
    for line in lines {
        if let Some(name) = line.trim().strip_prefix('$') {
            if let Some(previous) = current.take() {
                patches.push(previous);
            }
            current = Some(ExistingPatch {
                name: name.trim().to_string(),
                lines: Vec::new(),
            });
        } else if let Some(patch) = current.as_mut() {
            patch.lines.push(line.clone());
        }
    }
    if let Some(previous) = current {
        patches.push(previous);
    }
    patches
}

fn enabled_names(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .filter_map(|line| line.trim().strip_prefix('$'))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

fn with_enabled_name(lines: &[String], name: &str) -> Vec<String> {
    let mut result = lines.to_vec();
    if !enabled_names(lines).iter().any(|existing| existing == name) {
        result.push(format!("${name}"));
    }
    result
}

fn without_enabled_name(lines: &[String], name: &str) -> Vec<String> {
    lines
        .iter()
        .filter(|line| line.trim() != format!("${name}"))
        .cloned()
        .collect()
}

/// Plans a complete, exact OnFrame patch merge without reading or writing the
/// filesystem. Existing sections and unrelated lines are preserved by the
/// full-fidelity Dolphin INI editor.
pub fn plan_dolphin_onframe_install(
    request: &DolphinOnFrameInstallRequest<'_>,
) -> Result<DolphinOnFrameInstallPlan, DolphinOnFrameInstallPlanError> {
    let file_name = identity_file_name(request.game_id, request.revision)
        .ok_or(DolphinOnFrameInstallPlanError::InvalidIdentity)?;
    let title = request.document.title.trim();
    if !valid_patch_title(title) {
        return Err(DolphinOnFrameInstallPlanError::InvalidTitle);
    }
    let lines = onframe_lines(request.document)?;
    let document = request
        .existing_contents
        .map(parse_dolphin_ini)
        .unwrap_or_else(|| parse_dolphin_ini(""));
    if request.existing_contents.is_some() && !document.warnings.is_empty() {
        return Err(DolphinOnFrameInstallPlanError::ExistingDocumentMalformed);
    }

    let existing = parse_onframe_patches(&document.named_section_lines("OnFrame"));
    let same_name: Vec<_> = existing
        .iter()
        .filter(|patch| patch.name == title)
        .collect();
    let enabled = enabled_names(&document.named_section_lines("OnFrame_Enabled"));
    let disabled = enabled_names(&document.named_section_lines("OnFrame_Disabled"));

    let (status, can_apply, new_contents, conflicts, warnings) = match same_name.as_slice() {
        [patch] if patch.lines == lines && enabled.iter().any(|name| name == title) => (
            DolphinOnFrameInstallStatus::AlreadyInstalled,
            true,
            request.existing_contents.unwrap_or("").to_string(),
            Vec::new(),
            Vec::new(),
        ),
        [patch] if patch.lines == lines => {
            let contents = replace_named_section(
                &document,
                "OnFrame_Enabled",
                with_enabled_name(&document.named_section_lines("OnFrame_Enabled"), title),
            );
            let enabled_document = parse_dolphin_ini(&contents);
            let contents = replace_named_section(
                &enabled_document,
                "OnFrame_Disabled",
                without_enabled_name(
                    &enabled_document.named_section_lines("OnFrame_Disabled"),
                    title,
                ),
            );
            (
                DolphinOnFrameInstallStatus::EnabledExisting,
                true,
                contents,
                Vec::new(),
                vec!["existing OnFrame patch was enabled".into()],
            )
        }
        [] if disabled.iter().any(|name| name == title) => (
            DolphinOnFrameInstallStatus::Conflict,
            false,
            request.existing_contents.unwrap_or("").to_string(),
            vec![format!(
                "OnFrame patch title '{title}' is already referenced by the disabled-name section"
            )],
            Vec::new(),
        ),
        [] => {
            let mut onframe = document.named_section_lines("OnFrame");
            if !onframe.is_empty() && !onframe.last().is_some_and(String::is_empty) {
                onframe.push(String::new());
            }
            onframe.push(format!("${title}"));
            onframe.extend(lines.iter().cloned());
            let contents = replace_named_section(&document, "OnFrame", onframe);
            let next = parse_dolphin_ini(&contents);
            let contents = replace_named_section(
                &next,
                "OnFrame_Enabled",
                with_enabled_name(&next.named_section_lines("OnFrame_Enabled"), title),
            );
            (
                DolphinOnFrameInstallStatus::Ready,
                true,
                contents,
                Vec::new(),
                Vec::new(),
            )
        }
        _ => (
            DolphinOnFrameInstallStatus::Conflict,
            false,
            request.existing_contents.unwrap_or("").to_string(),
            vec![format!(
                "OnFrame patch title '{title}' already has different content"
            )],
            Vec::new(),
        ),
    };

    if new_contents.len() > MAX_GENERATED_ONFRAME_INI_BYTES {
        return Err(DolphinOnFrameInstallPlanError::GeneratedFileTooLarge);
    }
    Ok(DolphinOnFrameInstallPlan {
        game_id: request.game_id.trim().to_ascii_uppercase(),
        revision: request.revision,
        destination_file_name: file_name,
        patch_name: title.to_string(),
        lines,
        new_contents,
        status,
        can_apply,
        conflicts,
        warnings,
    })
}

fn digest(contents: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(contents.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn stage_dolphin_onframe_install(
    staging_root: &Path,
    plan: &DolphinOnFrameInstallPlan,
    destination_existed: bool,
) -> Result<StagedDolphinOnFrameIni, DolphinOnFrameInstallPlanError> {
    if !plan.can_apply {
        return Err(DolphinOnFrameInstallPlanError::UnsupportedDocument(
            "refusing to stage a blocked OnFrame plan".into(),
        ));
    }
    fs::create_dir_all(staging_root)
        .map_err(|error| DolphinOnFrameInstallPlanError::StagingUnavailable(error.to_string()))?;
    let path = staging_root.join(&plan.destination_file_name);
    let temporary = staging_root.join(format!(".{}.partial", plan.destination_file_name));
    fs::write(&temporary, plan.new_contents.as_bytes())
        .map_err(|error| DolphinOnFrameInstallPlanError::StagingUnavailable(error.to_string()))?;
    fs::rename(&temporary, &path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        DolphinOnFrameInstallPlanError::StagingUnavailable(error.to_string())
    })?;
    Ok(StagedDolphinOnFrameIni {
        staging_root: staging_root.to_path_buf(),
        path,
        digest: digest(&plan.new_contents),
        contents: plan.new_contents.clone(),
        destination_existed,
        destination_file_name: plan.destination_file_name.clone(),
    })
}

pub fn build_dolphin_onframe_install_preview(
    request: &DolphinOnFrameInstallPreviewRequest,
) -> Result<DolphinOnFrameInstallPreview, DolphinOnFrameInstallPlanError> {
    let relative = PathBuf::from("GameSettings").join(&request.staged.destination_file_name);
    let identity_value = match request.revision {
        Some(revision) => format!("{}:r{revision}", request.game_id),
        None => request.game_id.clone(),
    };
    let report = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::Dolphin,
        selected_archive: request.selected_archive.clone(),
        platform: Some("GameCube/Wii".into()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::DolphinGameId,
            state: PreviewIdentityState::Verified,
            value: Some(identity_value),
            archive_path: request.selected_archive.clone(),
            revision: request.revision,
        },
        destination_root: request.configuration_path.clone(),
        source_items: vec![PreviewSourceItem {
            adapter: PreviewAdapter::Dolphin,
            source_path: request.staged.path.clone(),
            expected_source_digest: Some(request.staged.digest.clone()),
            destination_relative_paths: vec![relative],
            match_strength: PreviewMatchStrength::VerifiedExact,
        }],
    })
    .map_err(|error| DolphinOnFrameInstallPlanError::PreviewFailed(error.to_string()))?;
    Ok(DolphinOnFrameInstallPreview {
        report,
        staged: request.staged.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::cheat_ir::CheatOperation;
    use super::*;

    fn document(title: &str) -> CheatDocument {
        CheatDocument {
            title: title.into(),
            platform: CheatPlatform::GameCube,
            source_format: CheatSourceFormat::DolphinOnFrame,
            operations: vec![CheatOperation::OnFrameWrite32 {
                address: 0x8000_1234,
                value: 0xDEAD_BEEF,
            }],
            issues: Vec::new(),
            provenance: vec!["test".into()],
        }
    }

    #[test]
    fn plans_all_supported_widths_without_changing_timing_policy() {
        let mut document = document("Widths");
        document.operations = vec![
            CheatOperation::OnFrameWrite8 {
                address: 0x8000_0001,
                value: 0x12,
            },
            CheatOperation::OnFrameWrite16 {
                address: 0x8000_0002,
                value: 0x1234,
            },
            CheatOperation::OnFrameWrite32 {
                address: 0x8000_0004,
                value: 0x1234_5678,
            },
        ];
        let plan = plan_dolphin_onframe_install(&DolphinOnFrameInstallRequest {
            game_id: "GALE01",
            revision: None,
            document: &document,
            existing_contents: None,
        })
        .unwrap();
        assert!(plan.can_apply);
        assert!(plan.new_contents.contains("0x80000001:byte:0x00000012"));
        assert!(plan.new_contents.contains("0x80000002:word:0x00001234"));
        assert!(plan.new_contents.contains("0x80000004:dword:0x12345678"));
    }

    #[test]
    fn plans_native_file_and_preserves_unrelated_sections() {
        let existing = "[Core]\nCPUThread = True\n[ActionReplay]\n$AR\n04000000 00000001\n[Gecko]\n$G\n04000000 00000002\n";
        let plan = plan_dolphin_onframe_install(&DolphinOnFrameInstallRequest {
            game_id: "GALE01",
            revision: Some(2),
            document: &document("Frame Fix"),
            existing_contents: Some(existing),
        })
        .unwrap();
        assert_eq!(plan.destination_file_name, "GALE01r2.ini");
        assert!(plan.new_contents.contains("[ActionReplay]\n$AR"));
        assert!(plan.new_contents.contains("[Gecko]\n$G"));
        assert!(plan.new_contents.contains("[OnFrame]\n$Frame Fix"));
        assert!(plan.new_contents.contains("[OnFrame_Enabled]\n$Frame Fix"));
    }

    #[test]
    fn identical_enabled_patch_is_noop() {
        let first = plan_dolphin_onframe_install(&DolphinOnFrameInstallRequest {
            game_id: "GALE01",
            revision: None,
            document: &document("Frame Fix"),
            existing_contents: Some("[OnFrame]\n$Frame Fix\n0x80001234:dword:0xDEADBEEF\n[OnFrame_Enabled]\n$Frame Fix\n"),
        })
        .unwrap();
        assert_eq!(first.status, DolphinOnFrameInstallStatus::AlreadyInstalled);
        assert!(!first.new_contents.is_empty());
    }

    #[test]
    fn different_same_name_is_conflict_and_mixed_document_is_refused() {
        let conflict = plan_dolphin_onframe_install(&DolphinOnFrameInstallRequest {
            game_id: "GALE01",
            revision: None,
            document: &document("Frame Fix"),
            existing_contents: Some("[OnFrame]\n$Frame Fix\n0x80001234:dword:0x11111111\n"),
        })
        .unwrap();
        assert!(!conflict.can_apply);
        assert_eq!(conflict.status, DolphinOnFrameInstallStatus::Conflict);

        let mut mixed = document("mixed");
        mixed.operations.push(CheatOperation::UnsupportedRaw {
            source_format: CheatSourceFormat::DolphinOnFrame,
            raw: "conditional".into(),
            reason: "unsupported".into(),
        });
        assert!(
            plan_dolphin_onframe_install(&DolphinOnFrameInstallRequest {
                game_id: "GALE01",
                revision: None,
                document: &mixed,
                existing_contents: None,
            })
            .is_err()
        );
    }
}
