//! RetroArch's reviewed, per-game bezel writer.
//!
//! The writer deliberately owns only two things in the discovered RetroArch
//! configuration root: the copied overlay image/descriptor and the per-game
//! override.  It never edits `retroarch.cfg`.  All bytes are staged through
//! the shared preview/apply transaction, which supplies the precondition,
//! backup, atomic-write, journal, and exact rollback contract.

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::bezel_apply::{
    BezelApplyPlan, BezelApplyRequest, BezelApplyStatus, BezelConfigEntry, BezelFilePurpose,
    BezelPlanError, BezelPlanRefusal, BezelPlannedFile,
};
use crate::patch_manager::{
    PreviewAdapter, PreviewIdentity, PreviewIdentityKind, PreviewIdentityState,
    PreviewMatchStrength, PreviewSourceItem, SharedApplyConfirmation, SharedApplyOptions,
    SharedApplyResult, SharedPreviewRequest, build_shared_preview, build_shared_transaction_plan,
    execute_shared_apply, execute_shared_rollback, preview_shared_rollback,
    require_retroarch_bezel_verification,
};

const MAX_CONFIG_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct RetroArchBezelApplyOptions {
    pub general_approved: bool,
    pub replacement_approved: bool,
    pub operation_id: String,
    pub timestamp_unix_seconds: u64,
    pub history_root: PathBuf,
    pub backup_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetroArchBezelApplyError {
    pub detail: String,
}

impl std::fmt::Display for RetroArchBezelApplyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for RetroArchBezelApplyError {}

/// Convert the read-only planner result into an owned, apply-capable plan.
/// No mutation occurs here.
pub fn prepare_retroarch_bezel_plan(
    request: &BezelApplyRequest,
) -> Result<BezelApplyPlan, BezelPlanError> {
    let mut plan = crate::bezel_apply::build_bezel_apply_plan(request)?;
    let Some(core) = plan.target.core.as_deref() else {
        return Ok(refuse(
            plan,
            BezelPlanRefusal::RetroArchCoreRequired,
            "an explicitly selected RetroArch core is required for a per-game override",
        ));
    };
    let (Some(config_path), Some(overlay_root)) = (
        plan.target.config_path.clone(),
        plan.target.destination_root.clone(),
    ) else {
        return Ok(plan);
    };
    let Some(config_root) = config_path.parent().map(Path::to_path_buf) else {
        return Ok(refuse(
            plan,
            BezelPlanRefusal::RetroArchConfigScopeInvalid,
            "RetroArch config scope has no parent directory",
        ));
    };
    if !is_safe_relative(&config_root, &overlay_root) {
        return Ok(refuse(
            plan,
            BezelPlanRefusal::RetroArchOverlayOutsideConfigRoot,
            "RetroArch overlay directory is outside the selected config root",
        ));
    }
    let Some(image_name) = safe_name(plan.source.path.file_name()) else {
        return Ok(refuse(
            plan,
            BezelPlanRefusal::RetroArchUnsafeName,
            "source bezel filename is not a safe single path component",
        ));
    };
    if !safe_component(core) || !safe_component(&plan.target.resolved_identity) {
        return Ok(refuse(
            plan,
            BezelPlanRefusal::RetroArchUnsafeName,
            "RetroArch core and game identity must be safe path components",
        ));
    }
    let descriptor_name = format!("emuwiz-{}.cfg", stem(&image_name));
    let descriptor = overlay_root.join(&descriptor_name);
    let override_path = config_root
        .join("config")
        .join(core)
        .join(format!("{}.cfg", plan.target.resolved_identity));
    let overlay_ref = descriptor.display().to_string();
    let existing_override = read_optional(&override_path)?;
    let merged_override = merge_override(existing_override.as_deref(), &overlay_ref);
    let descriptor_bytes = format!(
        "overlays = 1\noverlay0_overlay = \"{}\"\noverlay0_full_screen = true\n",
        image_name.replace('"', "")
    )
    .into_bytes();
    let destinations = [
        (
            overlay_root.join(&image_name),
            BezelFilePurpose::OverlayAsset,
        ),
        (descriptor.clone(), BezelFilePurpose::EmulatorConfig),
        (override_path.clone(), BezelFilePurpose::EmulatorConfig),
    ];
    let mut conflicts = Vec::new();
    let mut unsafe_destination = false;
    for (path, _) in &destinations {
        if path_is_symlink_or_unsafe(path) {
            unsafe_destination = true;
            conflicts.push(format!(
                "unsafe existing RetroArch destination: {}",
                path.display()
            ));
        }
    }
    if unsafe_destination {
        return Ok(refuse(
            plan,
            BezelPlanRefusal::RetroArchDestinationUnsafe,
            "RetroArch bezel destination contains a symlink or non-directory parent",
        ));
    }
    if merged_override.conflict {
        conflicts.push(format!(
            "existing user setting conflicts at {}",
            override_path.display()
        ));
    }
    if let Some(existing) = read_optional(&descriptor)? {
        if existing != descriptor_bytes {
            conflicts.push(format!(
                "existing overlay descriptor differs at {}",
                descriptor.display()
            ));
        }
    }
    plan.files = destinations
        .iter()
        .map(|(path, purpose)| {
            let existing = read_optional(path).ok().flatten();
            BezelPlannedFile {
                path: path.clone(),
                purpose: *purpose,
                exists: existing.is_some(),
                existing_sha256: existing.as_deref().map(crate::bezel_apply::digest_bytes),
            }
        })
        .collect();
    plan.config_entries = vec![
        BezelConfigEntry {
            path: override_path.clone(),
            key: "input_overlay".into(),
            old_value: merged_override.old_overlay,
            new_value: overlay_ref,
        },
        BezelConfigEntry {
            path: override_path,
            key: "input_overlay_enable".into(),
            old_value: merged_override.old_enabled,
            new_value: "true".into(),
        },
    ];
    plan.conflicts = conflicts;
    plan.warnings.clear();
    plan.refusals.clear();
    plan.status = BezelApplyStatus::Ready;
    plan.rollback.supported = true;
    plan.rollback.exact_restore = true;
    plan.rollback.backup_paths = plan
        .files
        .iter()
        .filter(|file| file.exists)
        .map(|file| file.path.clone())
        .collect();
    plan.rollback.reason = "shared RetroArch transaction journal stores exact prior bytes".into();
    plan.warnings.push("Only EmuWiz-owned overlay files and this per-game override are managed; retroarch.cfg remains untouched.".into());
    crate::bezel_apply::reseal_bezel_apply_plan(&mut plan)?;
    Ok(plan)
}

/// Apply a sealed plan through the shared transaction/history executor.
pub fn apply_retroarch_bezel_plan(
    plan: &BezelApplyPlan,
    options: &RetroArchBezelApplyOptions,
) -> Result<SharedApplyResult, RetroArchBezelApplyError> {
    if plan.status != BezelApplyStatus::Ready {
        return Err(apply_error("RetroArch bezel plan is refused"));
    }
    if plan.source_is_stale() {
        return Err(apply_error("source bezel changed since preview"));
    }
    let (Some(config_path), Some(overlay_root)) = (
        plan.target.config_path.as_ref(),
        plan.target.destination_root.as_ref(),
    ) else {
        return Err(apply_error("RetroArch plan has no selected config scope"));
    };
    let config_root = config_path
        .parent()
        .ok_or_else(|| apply_error("invalid config scope"))?;
    for file in &plan.files {
        let current = read_optional(&file.path).map_err(|e| apply_error(&e.to_string()))?;
        let digest = current.as_deref().map(crate::bezel_apply::digest_bytes);
        if current.is_some() != file.exists || digest != file.existing_sha256 {
            return Err(apply_error("RetroArch destination changed since preview"));
        }
    }
    let image_name = safe_name(plan.source.path.file_name())
        .ok_or_else(|| apply_error("unsafe source filename"))?;
    let descriptor = overlay_root.join(format!("emuwiz-{}.cfg", stem(&image_name)));
    let override_path = config_root
        .join("config")
        .join(
            plan.target
                .core
                .as_deref()
                .ok_or_else(|| apply_error("RetroArch core is missing"))?,
        )
        .join(format!("{}.cfg", plan.target.resolved_identity));
    let existing_override =
        read_optional(&override_path).map_err(|e| apply_error(&e.to_string()))?;
    let merged = merge_override(
        existing_override.as_deref(),
        &descriptor.display().to_string(),
    );
    if merged.conflict && !options.replacement_approved {
        return Err(apply_error(
            "existing user overlay setting requires explicit replacement approval",
        ));
    }
    let descriptor_bytes = format!(
        "overlays = 1\noverlay0_overlay = \"{}\"\noverlay0_full_screen = true\n",
        image_name.replace('"', "")
    )
    .into_bytes();
    let stage = tempfile::tempdir().map_err(|e| apply_error(&e.to_string()))?;
    let staged_image = stage.path().join("source.bin");
    let staged_descriptor = stage.path().join("overlay.cfg");
    let staged_override = stage.path().join("override.cfg");
    fs::copy(&plan.source.path, &staged_image).map_err(|e| apply_error(&e.to_string()))?;
    fs::write(&staged_descriptor, &descriptor_bytes).map_err(|e| apply_error(&e.to_string()))?;
    fs::write(&staged_override, merged.bytes).map_err(|e| apply_error(&e.to_string()))?;
    let relative = |path: &Path| {
        path.strip_prefix(config_root)
            .map(Path::to_path_buf)
            .map_err(|_| apply_error("destination escaped selected config root"))
    };
    let items = vec![
        PreviewSourceItem {
            adapter: PreviewAdapter::RetroArch,
            source_path: staged_image,
            expected_source_digest: None,
            destination_relative_paths: vec![relative(&overlay_root.join(&image_name))?],
            match_strength: PreviewMatchStrength::VerifiedExact,
        },
        PreviewSourceItem {
            adapter: PreviewAdapter::RetroArch,
            source_path: staged_descriptor,
            expected_source_digest: None,
            destination_relative_paths: vec![relative(&descriptor)?],
            match_strength: PreviewMatchStrength::VerifiedExact,
        },
        PreviewSourceItem {
            adapter: PreviewAdapter::RetroArch,
            source_path: staged_override,
            expected_source_digest: None,
            destination_relative_paths: vec![relative(&override_path)?],
            match_strength: PreviewMatchStrength::VerifiedExact,
        },
    ];
    let preview = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::RetroArch,
        selected_archive: plan.source.path.clone(),
        platform: Some(plan.target.platform.clone()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::RetroArchCatalogueMatch,
            state: PreviewIdentityState::Verified,
            value: Some(plan.target.resolved_identity.clone()),
            archive_path: plan.source.path.clone(),
            revision: None,
        },
        destination_root: config_root.to_path_buf(),
        source_items: items,
    })
    .map_err(|e| apply_error(&e.to_string()))?;
    if preview
        .entries
        .iter()
        .any(|entry| !entry.blockers.is_empty())
    {
        return Err(apply_error(
            "RetroArch destination changed or failed safety inspection",
        ));
    }
    let mut transaction =
        build_shared_transaction_plan(&preview, "retroarch", "retroarch_bezel", stage.path())
            .map_err(|e| apply_error(&format!("{e:?}")))?;
    require_retroarch_bezel_verification(&mut transaction)
        .map_err(|e| apply_error(&format!("{e:?}")))?;
    let current_context = transaction.context.clone();
    let result = execute_shared_apply(
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
            current_context,
            history_root: options.history_root.clone(),
            backup_root: options.backup_root.clone(),
        },
    );
    if result.journal.status == crate::patch_manager::SharedApplyStatus::PartialFailure {
        if let Some(journal_path) = result.journal_path.as_ref() {
            let rollback = preview_shared_rollback(journal_path, config_root, &options.backup_root);
            if rollback.available {
                let _ = execute_shared_rollback(
                    &rollback,
                    &crate::patch_manager::SharedRollbackOptions {
                        confirmation: crate::patch_manager::SharedRollbackConfirmation {
                            preview_id: rollback.preview_id.clone(),
                            approved: true,
                        },
                        rollback_operation_id: format!("{}-rollback", options.operation_id),
                        timestamp_unix_seconds: options.timestamp_unix_seconds,
                        history_root: options.history_root.clone(),
                        backup_root: options.backup_root.clone(),
                    },
                );
            }
        }
    }
    Ok(result)
}

fn refuse(mut plan: BezelApplyPlan, refusal: BezelPlanRefusal, detail: &str) -> BezelApplyPlan {
    plan.status = BezelApplyStatus::Refused;
    plan.refusals = vec![refusal];
    plan.warnings = vec![detail.into()];
    plan
}

fn apply_error(detail: &str) -> RetroArchBezelApplyError {
    RetroArchBezelApplyError {
        detail: detail.into(),
    }
}

fn safe_name(value: Option<&std::ffi::OsStr>) -> Option<String> {
    let value = value?.to_str()?.to_string();
    safe_component(&value).then_some(value)
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_control())
}

fn stem(value: &str) -> String {
    value
        .rsplit_once('.')
        .map_or(value, |(stem, _)| stem)
        .to_string()
}

fn is_safe_relative(root: &Path, path: &Path) -> bool {
    path.is_absolute()
        && root.is_absolute()
        && !path.components().any(|c| c == Component::ParentDir)
        && path.strip_prefix(root).is_ok()
}

fn path_is_symlink_or_unsafe(path: &Path) -> bool {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if let Ok(metadata) = fs::symlink_metadata(&current) {
            if metadata.file_type().is_symlink() || (!metadata.is_dir() && current != path) {
                return true;
            }
        }
    }
    false
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, BezelPlanError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(BezelPlanError {
            refusal: BezelPlanRefusal::RetroArchUnsafeName,
            path: Some(path.into()),
            detail: "RetroArch destination is a symlink".into(),
        }),
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_CONFIG_BYTES => {
            fs::read(path).map(Some).map_err(|e| BezelPlanError {
                refusal: BezelPlanRefusal::RetroArchConfigScopeInvalid,
                path: Some(path.into()),
                detail: e.to_string(),
            })
        }
        Ok(metadata) if metadata.is_file() => Err(BezelPlanError {
            refusal: BezelPlanRefusal::RetroArchConfigScopeInvalid,
            path: Some(path.into()),
            detail: "RetroArch config exceeds bounded size".into(),
        }),
        Ok(_) => Err(BezelPlanError {
            refusal: BezelPlanRefusal::RetroArchConfigScopeInvalid,
            path: Some(path.into()),
            detail: "RetroArch destination is not a regular file".into(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(BezelPlanError {
            refusal: BezelPlanRefusal::RetroArchConfigScopeInvalid,
            path: Some(path.into()),
            detail: error.to_string(),
        }),
    }
}

struct MergedOverride {
    bytes: Vec<u8>,
    conflict: bool,
    old_overlay: Option<String>,
    old_enabled: Option<String>,
}

fn merge_override(existing: Option<&[u8]>, overlay: &str) -> MergedOverride {
    let mut text = existing
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default();
    let mut old_overlay = None;
    let mut old_enabled = None;
    let mut conflict = false;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"').to_string();
        match key {
            "input_overlay" => {
                old_overlay = Some(value.clone());
                if value != overlay {
                    conflict = true;
                }
            }
            "input_overlay_enable" => {
                old_enabled = Some(value.clone());
                if value != "true" {
                    conflict = true;
                }
            }
            _ => {}
        }
    }
    if old_overlay.is_none() {
        append_line(&mut text, &format!("input_overlay = \"{}\"", overlay));
    }
    if old_enabled.is_none() {
        append_line(&mut text, "input_overlay_enable = \"true\"");
    }
    MergedOverride {
        bytes: text.into_bytes(),
        conflict,
        old_overlay,
        old_enabled,
    }
}

fn append_line(text: &mut String, line: &str) {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bezel_decorations::{
        DecorationAsset, DecorationEvidence, DecorationProvenance, DecorationReadiness,
        DecorationScope, DecorationSource, LocalBezelImageInfo, ViewportMetadata,
    };
    use crate::patch_manager::{
        SharedRollbackConfirmation, SharedRollbackOptions, execute_shared_rollback,
        preview_shared_rollback,
    };

    fn request(root: &Path, source: &Path) -> BezelApplyRequest {
        BezelApplyRequest {
            source_asset: DecorationAsset {
                id: "test-bezel".into(),
                scope: DecorationScope::Game,
                source: DecorationSource::LocalPack {
                    path: source.display().to_string(),
                },
                evidence: DecorationEvidence::VerifiedIdentity {
                    identity: "game-id".into(),
                },
                targets: Vec::new(),
                readiness: DecorationReadiness::Ready,
                provenance: DecorationProvenance {
                    provider: "test".into(),
                    reference: "fixture".into(),
                    retrieved_at_unix_seconds: None,
                },
                viewport: Some(ViewportMetadata {
                    left: 0,
                    top: 0,
                    width: 100,
                    height: 80,
                }),
            },
            source_image: LocalBezelImageInfo {
                width: 100,
                height: 80,
                format: "png".into(),
            },
            resolved_identity: "game-id".into(),
            platform: "SNES".into(),
            emulator: "retroarch".into(),
            core: Some("snes9x".into()),
            config_path: Some(root.join("retroarch.cfg")),
            overlay_root: Some(root.join("overlays")),
            approved_roots: vec![root.to_path_buf()],
        }
    }

    fn options(root: &Path, id: &str) -> RetroArchBezelApplyOptions {
        RetroArchBezelApplyOptions {
            general_approved: true,
            replacement_approved: false,
            operation_id: id.into(),
            timestamp_unix_seconds: 1,
            history_root: root.with_extension("history"),
            backup_root: root.with_extension("backups"),
        }
    }

    #[test]
    fn new_apply_preserves_source_and_unrelated_config_and_undoes_exactly() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("bezel.png");
        fs::write(&source, b"source-bytes").unwrap();
        let unrelated = root.path().join("retroarch.cfg");
        fs::write(&unrelated, b"video_driver = \"gl\"\n").unwrap();
        let original_source = fs::read(&source).unwrap();
        let original_config = fs::read(&unrelated).unwrap();
        let plan = prepare_retroarch_bezel_plan(&request(root.path(), &source)).unwrap();
        assert_eq!(plan.status, BezelApplyStatus::Ready);
        assert!(plan.conflicts.is_empty());
        let result = apply_retroarch_bezel_plan(&plan, &options(root.path(), "bezel-new")).unwrap();
        assert_eq!(
            result.journal.status,
            crate::patch_manager::SharedApplyStatus::Success
        );
        assert_eq!(fs::read(&source).unwrap(), original_source);
        assert_eq!(fs::read(&unrelated).unwrap(), original_config);
        let journal = result.journal_path.unwrap();
        let rollback = preview_shared_rollback(
            &journal,
            root.path(),
            &root.path().with_extension("backups"),
        );
        assert!(rollback.available);
        let undone = execute_shared_rollback(
            &rollback,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: rollback.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "bezel-new-undo".into(),
                timestamp_unix_seconds: 2,
                history_root: root.path().with_extension("history"),
                backup_root: root.path().with_extension("backups"),
            },
        );
        assert_eq!(
            undone.status,
            crate::patch_manager::SharedApplyStatus::Success
        );
        assert!(!root.path().join("overlays/bezel.png").exists());
    }

    #[test]
    fn compatible_existing_config_is_idempotent_and_conflict_is_explicit() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("bezel.png");
        fs::write(&source, b"source-bytes").unwrap();
        fs::create_dir_all(root.path().join("config/snes9x")).unwrap();
        fs::create_dir_all(root.path().join("overlays")).unwrap();
        fs::write(root.path().join("overlays/bezel.png"), b"source-bytes").unwrap();
        fs::write(
            root.path().join("overlays/emuwiz-bezel.cfg"),
            b"overlays = 1\noverlay0_overlay = \"bezel.png\"\noverlay0_full_screen = true\n",
        )
        .unwrap();
        let compatible = String::from("# user comment\ninput_overlay = \"")
            + &root
                .path()
                .join("overlays/emuwiz-bezel.cfg")
                .display()
                .to_string()
            + "\"\ninput_overlay_enable = \"true\"\n";
        fs::write(root.path().join("config/snes9x/game-id.cfg"), compatible).unwrap();
        let plan = prepare_retroarch_bezel_plan(&request(root.path(), &source)).unwrap();
        assert_eq!(plan.status, BezelApplyStatus::Ready);
        assert!(plan.conflicts.is_empty());
        let mut conflict = request(root.path(), &source);
        fs::write(
            root.path().join("config/snes9x/game-id.cfg"),
            b"input_overlay = \"user.cfg\"\n",
        )
        .unwrap();
        conflict.core = Some("snes9x".into());
        let plan = prepare_retroarch_bezel_plan(&conflict).unwrap();
        assert!(
            plan.conflicts
                .iter()
                .any(|value| value.contains("conflicts"))
        );
        assert!(
            apply_retroarch_bezel_plan(&plan, &options(root.path(), "bezel-conflict")).is_err()
        );
    }

    #[test]
    fn stale_source_destination_and_symlink_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("bezel.png");
        fs::write(&source, b"source-bytes").unwrap();
        let plan = prepare_retroarch_bezel_plan(&request(root.path(), &source)).unwrap();
        fs::write(&source, b"changed").unwrap();
        assert!(apply_retroarch_bezel_plan(&plan, &options(root.path(), "bezel-stale")).is_err());

        fs::write(&source, b"source-bytes").unwrap();
        let plan = prepare_retroarch_bezel_plan(&request(root.path(), &source)).unwrap();
        fs::create_dir_all(root.path().join("overlays")).unwrap();
        fs::write(
            root.path().join("overlays/bezel.png"),
            b"destination-changed",
        )
        .unwrap();
        assert!(
            apply_retroarch_bezel_plan(&plan, &options(root.path(), "bezel-destination")).is_err()
        );

        let escaped = tempfile::tempdir().unwrap();
        let mut symlink_request = request(root.path(), &source);
        symlink_request.overlay_root = Some(root.path().join("link"));
        std::os::unix::fs::symlink(escaped.path(), root.path().join("link")).unwrap();
        let plan = prepare_retroarch_bezel_plan(&symlink_request).unwrap();
        assert_eq!(plan.status, BezelApplyStatus::Refused);
        assert_eq!(
            plan.refusals,
            vec![BezelPlanRefusal::RetroArchDestinationUnsafe]
        );
    }
}
