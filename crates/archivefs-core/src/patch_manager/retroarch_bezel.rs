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
    BezelPlanError, BezelPlanRefusal, BezelPlannedFile, RetroArchContentIdentity,
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
    let Some(launch) = request.retroarch_launch.as_ref() else {
        return Ok(refuse(
            plan,
            BezelPlanRefusal::RetroArchLaunchIdentityUnknown,
            "the exact content file and RetroArch core this game launches with are not confirmed, so no per-game override is written (RetroArch never loads one named after a display title)",
        ));
    };
    if core != launch.core_library_name {
        return Ok(refuse(
            plan,
            BezelPlanRefusal::RetroArchLaunchIdentityUnknown,
            "the selected core is not the core this game launches with",
        ));
    }
    let override_path = match retroarch_game_override_path(&config_path, &config_root, launch) {
        Ok(path) => path,
        Err((refusal, detail)) => return Ok(refuse(plan, refusal, &detail)),
    };
    let descriptor_name = format!("emuwiz-{}.cfg", stem(&image_name));
    let descriptor = overlay_root.join(&descriptor_name);
    let overlay_ref = descriptor.display().to_string();
    let existing_override = read_optional(&override_path)?;
    let merged_override = merge_override(existing_override.as_deref(), &overlay_ref);
    let descriptor_bytes = overlay_descriptor(&image_name);
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

/// What an Apply finally left behind, after its automatic rollback.
#[derive(Debug)]
pub struct RetroArchBezelApplyReport {
    pub result: SharedApplyResult,
    pub automatic_rollback: BezelAutomaticRollback,
}

/// The automatic rollback a partly failed Apply runs, judged by its actual
/// result rather than by what the Apply attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BezelAutomaticRollback {
    /// The Apply did not partly fail, so nothing was rolled back.
    NotNeeded,
    /// Every change the Apply made was undone: no changes remain.
    Completed,
    /// Some changes remain. `undo_available` is whether a fresh Undo preview
    /// of the Apply is actually available.
    Incomplete { undo_available: bool },
}

/// Apply a sealed plan through the shared transaction/history executor.
pub fn apply_retroarch_bezel_plan(
    plan: &BezelApplyPlan,
    options: &RetroArchBezelApplyOptions,
) -> Result<SharedApplyResult, RetroArchBezelApplyError> {
    apply_retroarch_bezel_plan_reported(plan, options).map(|report| report.result)
}

/// [`apply_retroarch_bezel_plan`] together with what its automatic rollback
/// actually achieved, so callers report the final state truthfully.
pub fn apply_retroarch_bezel_plan_reported(
    plan: &BezelApplyPlan,
    options: &RetroArchBezelApplyOptions,
) -> Result<RetroArchBezelApplyReport, RetroArchBezelApplyError> {
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
    // The sealed plan recorded where RetroArch looks for this game's override.
    let override_path = plan
        .config_entries
        .first()
        .map(|entry| entry.path.clone())
        .ok_or_else(|| apply_error("RetroArch plan has no per-game override entry"))?;
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
    let descriptor_bytes = overlay_descriptor(&image_name);
    // Changing any existing file is a replacement. Refuse before staging so
    // an unapproved run never half-applies and then rolls itself back.
    if !options.replacement_approved {
        let new_digest = |path: &Path| {
            if path == override_path {
                Some(crate::bezel_apply::digest_bytes(&merged.bytes))
            } else if path == descriptor {
                Some(crate::bezel_apply::digest_bytes(&descriptor_bytes))
            } else {
                Some(plan.source.sha256.clone())
            }
        };
        if plan
            .files
            .iter()
            .any(|file| file.exists && file.existing_sha256 != new_digest(&file.path))
        {
            return Err(apply_error(
                "changing existing RetroArch files requires explicit replacement approval",
            ));
        }
    }
    let stage = tempfile::tempdir().map_err(|e| apply_error(&e.to_string()))?;
    let staged_image = stage.path().join("source.bin");
    let staged_descriptor = stage.path().join("overlay.cfg");
    let staged_override = stage.path().join("override.cfg");
    fs::copy(&plan.source.path, &staged_image).map_err(|e| apply_error(&e.to_string()))?;
    fs::write(&staged_descriptor, &descriptor_bytes).map_err(|e| apply_error(&e.to_string()))?;
    fs::write(&staged_override, &merged.bytes).map_err(|e| apply_error(&e.to_string()))?;
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
    let mut automatic_rollback = BezelAutomaticRollback::NotNeeded;
    if result.journal.status == crate::patch_manager::SharedApplyStatus::PartialFailure {
        automatic_rollback = BezelAutomaticRollback::Incomplete {
            undo_available: false,
        };
        if let Some(journal_path) = result.journal_path.as_ref() {
            let rollback = preview_shared_rollback(journal_path, config_root, &options.backup_root);
            if rollback.available {
                let rolled_back = execute_shared_rollback(
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
                if rolled_back.status == crate::patch_manager::SharedApplyStatus::Success {
                    automatic_rollback = BezelAutomaticRollback::Completed;
                }
            }
            if automatic_rollback != BezelAutomaticRollback::Completed {
                // Offer Undo only if a fresh preview says it can really run.
                automatic_rollback = BezelAutomaticRollback::Incomplete {
                    undo_available: preview_shared_rollback(
                        journal_path,
                        config_root,
                        &options.backup_root,
                    )
                    .available,
                };
            }
        }
    }
    Ok(RetroArchBezelApplyReport {
        result,
        automatic_rollback,
    })
}

/// RetroArch's per-game override for `launch`:
/// `<config directory>/<core library_name>/<content file stem>.cfg`, where
/// the config directory is `rgui_config_directory` or, when unset or
/// `default`, `config` beside retroarch.cfg (the same rules as
/// `launch::retroarch_overrides`). Anything that cannot be resolved exactly
/// inside the selected RetroArch folder is refused, never guessed.
fn retroarch_game_override_path(
    config_path: &Path,
    config_root: &Path,
    launch: &RetroArchContentIdentity,
) -> Result<PathBuf, (BezelPlanRefusal, String)> {
    if !launch.content_path.is_absolute() {
        return Err((
            BezelPlanRefusal::RetroArchLaunchIdentityUnknown,
            "the launch content path is not absolute".into(),
        ));
    }
    let content_stem = launch
        .content_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(stem)
        .filter(|value| safe_component(value))
        .ok_or_else(|| {
            (
                BezelPlanRefusal::RetroArchUnsafeName,
                "the launch content file name cannot name a RetroArch override".to_string(),
            )
        })?;
    if !safe_component(&launch.core_library_name) {
        return Err((
            BezelPlanRefusal::RetroArchUnsafeName,
            "the core library name cannot name a RetroArch override folder".into(),
        ));
    }
    let text = read_optional(config_path)
        .map_err(|error| (error.refusal, error.detail))?
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    // The last assignment wins, as in RetroArch.
    let configured = text
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "rgui_config_directory")
                .then(|| value.trim().trim_matches('"').to_string())
        })
        .next_back();
    let directory = match configured.as_deref() {
        None | Some("") | Some("default") => config_root.join("config"),
        Some(value) => {
            let expanded = crate::launch::retroarch_overrides::expand_directory(
                value,
                launch.home.as_deref(),
            )
            .map_err(|reason| {
                (
                    BezelPlanRefusal::RetroArchConfigScopeInvalid,
                    format!(
                        "RetroArch's rgui_config_directory {value:?} cannot be used ({reason})"
                    ),
                )
            })?;
            if !is_safe_relative(config_root, &expanded) {
                return Err((
                    BezelPlanRefusal::RetroArchConfigScopeInvalid,
                    format!(
                        "RetroArch's rgui_config_directory {value:?} is outside the selected RetroArch folder, so EmuWiz cannot place the override safely"
                    ),
                ));
            }
            expanded
        }
    };
    Ok(directory
        .join(&launch.core_library_name)
        .join(format!("{content_stem}.cfg")))
}

/// The overlay descriptor RetroArch's `input_overlay` points at. RetroArch
/// refuses an overlay without `overlayN_descs` ("Failed to read number of
/// descs"), so the bezel-only overlay declares zero input descriptors.
/// Checked against RetroArch 1.22.2 (Flatpak) with FCEUmm.
fn overlay_descriptor(image_name: &str) -> Vec<u8> {
    format!(
        "overlays = 1\noverlay0_overlay = \"{}\"\noverlay0_full_screen = true\noverlay0_descs = 0\n",
        image_name.replace('"', "")
    )
    .into_bytes()
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

/// Merge the two overlay keys into a per-game override. Conflicting values
/// are rewritten (the caller requires replacement approval for that); every
/// other line, including its line ending, is preserved byte for byte.
fn merge_override(existing: Option<&[u8]>, overlay: &str) -> MergedOverride {
    let original = existing
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default();
    let mut text = String::with_capacity(original.len());
    let mut old_overlay = None;
    let mut old_enabled = None;
    let mut conflict = false;
    for line in original.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let ending = &line[body.len()..];
        let Some((key, value)) = body.split_once('=') else {
            text.push_str(line);
            continue;
        };
        let value = value.trim().trim_matches('"').to_string();
        let replacement = match key.trim() {
            "input_overlay" => {
                old_overlay = Some(value.clone());
                (value != overlay).then(|| format!("input_overlay = \"{overlay}\""))
            }
            "input_overlay_enable" => {
                old_enabled = Some(value.clone());
                (value != "true").then(|| "input_overlay_enable = \"true\"".to_string())
            }
            _ => None,
        };
        match replacement {
            Some(replacement) => {
                conflict = true;
                text.push_str(&replacement);
                text.push_str(ending);
            }
            None => text.push_str(line),
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
            retroarch_launch: Some(RetroArchContentIdentity {
                content_path: root.join("roms/game-id.sfc"),
                core_library_name: "snes9x".into(),
                home: None,
            }),
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
    fn binary_png_bezel_installs_byte_for_byte_and_undoes() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("bezel.png");
        // A real PNG is not UTF-8; the post-write check must not reject it.
        let png = [
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0xFF, 0xFE, 0x00, 0xC3,
        ];
        fs::write(&source, png).unwrap();
        let plan = prepare_retroarch_bezel_plan(&request(root.path(), &source)).unwrap();
        assert_eq!(plan.status, BezelApplyStatus::Ready);
        let result = apply_retroarch_bezel_plan(&plan, &options(root.path(), "bezel-png")).unwrap();
        assert_eq!(
            result.journal.status,
            crate::patch_manager::SharedApplyStatus::Success
        );
        let installed = root.path().join("overlays/bezel.png");
        assert_eq!(fs::read(&installed).unwrap(), png);
        let rollback = preview_shared_rollback(
            &result.journal_path.unwrap(),
            root.path(),
            &root.path().with_extension("backups"),
        );
        let undone = execute_shared_rollback(
            &rollback,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: rollback.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "bezel-png-undo".into(),
                timestamp_unix_seconds: 2,
                history_root: root.path().with_extension("history"),
                backup_root: root.path().with_extension("backups"),
            },
        );
        assert_eq!(
            undone.status,
            crate::patch_manager::SharedApplyStatus::Success
        );
        assert!(!installed.exists());
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
            b"overlays = 1\noverlay0_overlay = \"bezel.png\"\noverlay0_full_screen = true\noverlay0_descs = 0\n",
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

    fn undo(root: &Path, journal: &Path, id: &str) -> crate::patch_manager::SharedApplyStatus {
        let rollback = preview_shared_rollback(journal, root, &root.with_extension("backups"));
        assert!(rollback.available, "{:?}", rollback);
        execute_shared_rollback(
            &rollback,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: rollback.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: id.into(),
                timestamp_unix_seconds: 2,
                history_root: root.with_extension("history"),
                backup_root: root.with_extension("backups"),
            },
        )
        .status
    }

    /// End-to-end against a disposable RetroArch root with a pre-existing
    /// user override: only the two overlay keys are added, the rest of the
    /// file and retroarch.cfg are byte-identical, and Undo restores the
    /// original override bytes exactly.
    #[test]
    fn existing_override_gets_only_overlay_keys_and_undo_restores_exact_bytes() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("bezel.png");
        fs::write(&source, b"source-bytes").unwrap();
        let main_cfg = root.path().join("retroarch.cfg");
        fs::write(
            &main_cfg,
            b"video_driver = \"gl\"\ninput_overlay = \"global.cfg\"\n",
        )
        .unwrap();
        let override_path = root.path().join("config/snes9x/game-id.cfg");
        fs::create_dir_all(override_path.parent().unwrap()).unwrap();
        let original = b"# mine\r\naspect_ratio_index = \"22\"\nvideo_smooth = \"false\"".to_vec();
        fs::write(&override_path, &original).unwrap();
        let main_before = fs::read(&main_cfg).unwrap();

        let plan = prepare_retroarch_bezel_plan(&request(root.path(), &source)).unwrap();
        assert_eq!(plan.status, BezelApplyStatus::Ready);
        assert!(plan.conflicts.is_empty());
        assert_eq!(plan.rollback.backup_paths, vec![override_path.clone()]);
        // Editing the user's existing override is a replacement: refused up
        // front, with nothing written, until it is explicitly approved.
        assert!(
            apply_retroarch_bezel_plan(&plan, &options(root.path(), "bezel-unapproved")).is_err()
        );
        assert_eq!(fs::read(&override_path).unwrap(), original);
        assert!(!root.path().join("overlays").exists());
        let mut approved = options(root.path(), "bezel-existing");
        approved.replacement_approved = true;
        let result = apply_retroarch_bezel_plan(&plan, &approved).unwrap();
        assert_eq!(
            result.journal.status,
            crate::patch_manager::SharedApplyStatus::Success
        );
        let written = fs::read_to_string(&override_path).unwrap();
        let descriptor = root.path().join("overlays/emuwiz-bezel.cfg");
        assert_eq!(
            written,
            format!(
                "# mine\r\naspect_ratio_index = \"22\"\nvideo_smooth = \"false\"\ninput_overlay = \"{}\"\ninput_overlay_enable = \"true\"\n",
                descriptor.display()
            )
        );
        assert_eq!(fs::read(&main_cfg).unwrap(), main_before);

        // A second Apply of the same (now stale) plan is refused, not repeated.
        assert!(apply_retroarch_bezel_plan(&plan, &approved).is_err());

        let journal = result.journal_path.unwrap();
        assert_eq!(
            undo(root.path(), &journal, "bezel-existing-undo"),
            crate::patch_manager::SharedApplyStatus::Success
        );
        assert_eq!(fs::read(&override_path).unwrap(), original);
        assert!(!descriptor.exists());
        assert!(!root.path().join("overlays/bezel.png").exists());
        assert_eq!(fs::read(&main_cfg).unwrap(), main_before);
    }

    /// Job #22: where the folder cannot replace files atomically, the
    /// override replacement is refused and the automatic rollback removes the
    /// overlay files it had created. The report says nothing remains (so no
    /// Undo is offered) while the backup and both journals are kept.
    #[test]
    fn refused_replacement_rolls_back_fully_and_reports_nothing_remaining() {
        use crate::patch_manager::shared_transaction::{FaultPoint, inject_fault};
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("bezel.png");
        fs::write(&source, b"source-bytes").unwrap();
        let override_path = root.path().join("config/snes9x/game-id.cfg");
        fs::create_dir_all(override_path.parent().unwrap()).unwrap();
        let original = b"video_smooth = \"false\"\n".to_vec();
        fs::write(&override_path, &original).unwrap();
        let plan = prepare_retroarch_bezel_plan(&request(root.path(), &source)).unwrap();
        let mut approved = options(root.path(), "bezel-no-exchange");
        approved.replacement_approved = true;
        inject_fault(Some(FaultPoint::ExchangeUnsupported));
        let report = apply_retroarch_bezel_plan_reported(&plan, &approved).unwrap();
        inject_fault(None);
        assert_eq!(
            report.result.journal.status,
            crate::patch_manager::SharedApplyStatus::PartialFailure
        );
        assert_eq!(report.automatic_rollback, BezelAutomaticRollback::Completed);
        // No changes remain.
        assert_eq!(fs::read(&override_path).unwrap(), original);
        assert!(!root.path().join("overlays/emuwiz-bezel.cfg").exists());
        assert!(!root.path().join("overlays/bezel.png").exists());
        // Recovery records are preserved.
        let replaced = report
            .result
            .journal
            .entries
            .iter()
            .find(|entry| entry.backup_path.is_some())
            .expect("the override was backed up before the refusal");
        assert!(replaced.failures.iter().any(|failure| failure.kind
            == crate::patch_manager::SharedApplyFailureKind::AtomicReplaceUnsupported));
        let backup = replaced
            .backup_path
            .as_ref()
            .unwrap()
            .to_path_buf()
            .unwrap();
        assert_eq!(fs::read(&backup).unwrap(), original);
        let journal = report.result.journal_path.clone().unwrap();
        assert!(journal.is_file());
        assert!(journal.with_extension("rollback.json").is_file());
        // Nothing is left to undo.
        assert!(
            !crate::patch_manager::preview_shared_rollback(
                &journal,
                root.path(),
                &root.path().with_extension("backups"),
            )
            .available
        );
    }

    /// An ordinary successful Apply needs no automatic rollback.
    #[test]
    fn successful_apply_reports_no_automatic_rollback() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("bezel.png");
        fs::write(&source, b"source-bytes").unwrap();
        let plan = prepare_retroarch_bezel_plan(&request(root.path(), &source)).unwrap();
        let report =
            apply_retroarch_bezel_plan_reported(&plan, &options(root.path(), "bezel-ok")).unwrap();
        assert_eq!(
            report.result.journal.status,
            crate::patch_manager::SharedApplyStatus::Success
        );
        assert_eq!(report.automatic_rollback, BezelAutomaticRollback::NotNeeded);
    }

    #[test]
    fn override_edited_after_preview_refuses_apply_and_writes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("bezel.png");
        fs::write(&source, b"source-bytes").unwrap();
        let override_path = root.path().join("config/snes9x/game-id.cfg");
        fs::create_dir_all(override_path.parent().unwrap()).unwrap();
        fs::write(&override_path, b"video_smooth = \"false\"\n").unwrap();
        let plan = prepare_retroarch_bezel_plan(&request(root.path(), &source)).unwrap();
        fs::write(&override_path, b"video_smooth = \"true\"\n").unwrap();
        assert!(apply_retroarch_bezel_plan(&plan, &options(root.path(), "bezel-edited")).is_err());
        assert_eq!(
            fs::read(&override_path).unwrap(),
            b"video_smooth = \"true\"\n"
        );
        assert!(!root.path().join("overlays").exists());
    }

    /// An approved replacement must really point RetroArch at the new
    /// overlay; reporting success while the user's old value stays in place
    /// would be a false result.
    #[test]
    fn approved_replacement_rewrites_conflicting_keys_and_undo_restores_them() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("bezel.png");
        fs::write(&source, b"source-bytes").unwrap();
        let override_path = root.path().join("config/snes9x/game-id.cfg");
        fs::create_dir_all(override_path.parent().unwrap()).unwrap();
        let original =
            b"video_smooth = \"false\"\ninput_overlay = \"user.cfg\"\ninput_overlay_enable = \"false\"\n".to_vec();
        fs::write(&override_path, &original).unwrap();
        let plan = prepare_retroarch_bezel_plan(&request(root.path(), &source)).unwrap();
        assert!(!plan.conflicts.is_empty());
        let mut approved = options(root.path(), "bezel-replace");
        approved.replacement_approved = true;
        let result = apply_retroarch_bezel_plan(&plan, &approved).unwrap();
        assert_eq!(
            result.journal.status,
            crate::patch_manager::SharedApplyStatus::Success
        );
        let descriptor = root.path().join("overlays/emuwiz-bezel.cfg");
        assert_eq!(
            fs::read_to_string(&override_path).unwrap(),
            format!(
                "video_smooth = \"false\"\ninput_overlay = \"{}\"\ninput_overlay_enable = \"true\"\n",
                descriptor.display()
            )
        );
        assert_eq!(
            undo(
                root.path(),
                &result.journal_path.unwrap(),
                "bezel-replace-undo"
            ),
            crate::patch_manager::SharedApplyStatus::Success
        );
        assert_eq!(fs::read(&override_path).unwrap(), original);
    }

    fn override_of(plan: &BezelApplyPlan) -> PathBuf {
        assert_eq!(plan.status, BezelApplyStatus::Ready, "{:?}", plan.warnings);
        plan.config_entries[0].path.clone()
    }

    fn launched(root: &Path, file: &str, core: &str) -> BezelApplyRequest {
        let source = root.join("bezel.png");
        fs::write(&source, b"source-bytes").unwrap();
        let mut request = request(root, &source);
        request.resolved_identity = "Super Mario World".into(); // display title
        request.core = Some(core.into());
        request.retroarch_launch = Some(RetroArchContentIdentity {
            content_path: root.join("roms/snes").join(file),
            core_library_name: core.into(),
            home: Some(root.to_path_buf()),
        });
        request
    }

    #[test]
    fn override_is_named_from_the_launched_file_not_the_display_title() {
        let root = tempfile::tempdir().unwrap();
        let plan = prepare_retroarch_bezel_plan(&launched(
            root.path(),
            "SMW renamed by user.sfc",
            "Snes9x",
        ))
        .unwrap();
        assert_eq!(
            override_of(&plan),
            root.path().join("config/Snes9x/SMW renamed by user.cfg")
        );
        assert!(plan.files.iter().all(|file| {
            !file
                .path
                .display()
                .to_string()
                .contains("Super Mario World")
        }));
    }

    #[test]
    fn region_suffixes_and_extra_dots_keep_retroarchs_stem() {
        let root = tempfile::tempdir().unwrap();
        for (file, expected) in [
            ("Super Mario World (USA).sfc", "Super Mario World (USA).cfg"),
            (
                "Game (Europe) (En,Fr,De) (Rev 1).v1.1.smc",
                "Game (Europe) (En,Fr,De) (Rev 1).v1.1.cfg",
            ),
        ] {
            let plan =
                prepare_retroarch_bezel_plan(&launched(root.path(), file, "Snes9x")).unwrap();
            assert_eq!(
                override_of(&plan),
                root.path().join("config/Snes9x").join(expected)
            );
        }
    }

    #[test]
    fn different_cores_get_their_own_library_name_folders() {
        let root = tempfile::tempdir().unwrap();
        let snes9x =
            prepare_retroarch_bezel_plan(&launched(root.path(), "Game (USA).sfc", "Snes9x"))
                .unwrap();
        let bsnes =
            prepare_retroarch_bezel_plan(&launched(root.path(), "Game (USA).sfc", "bsnes-hd beta"))
                .unwrap();
        assert_eq!(
            override_of(&snes9x),
            root.path().join("config/Snes9x/Game (USA).cfg")
        );
        assert_eq!(
            override_of(&bsnes),
            root.path().join("config/bsnes-hd beta/Game (USA).cfg")
        );
        // A core stem that is not the launch core's library name is refused.
        let mut stem = launched(root.path(), "Game (USA).sfc", "Snes9x");
        stem.core = Some("snes9x_libretro".into());
        let plan = prepare_retroarch_bezel_plan(&stem).unwrap();
        assert_eq!(
            plan.refusals,
            vec![BezelPlanRefusal::RetroArchLaunchIdentityUnknown]
        );
    }

    #[test]
    fn missing_or_relative_launch_identity_is_refused_without_writing() {
        let root = tempfile::tempdir().unwrap();
        let mut missing = launched(root.path(), "Game (USA).sfc", "Snes9x");
        missing.retroarch_launch = None;
        let plan = prepare_retroarch_bezel_plan(&missing).unwrap();
        assert_eq!(plan.status, BezelApplyStatus::Refused);
        assert_eq!(
            plan.refusals,
            vec![BezelPlanRefusal::RetroArchLaunchIdentityUnknown]
        );
        assert!(apply_retroarch_bezel_plan(&plan, &options(root.path(), "no-id")).is_err());
        let mut relative = launched(root.path(), "Game (USA).sfc", "Snes9x");
        relative.retroarch_launch.as_mut().unwrap().content_path = "Game (USA).sfc".into();
        let plan = prepare_retroarch_bezel_plan(&relative).unwrap();
        assert_eq!(plan.status, BezelApplyStatus::Refused);
        assert!(!root.path().join("config").exists());
        assert!(!root.path().join("overlays").exists());
    }

    #[test]
    fn configured_override_directory_is_used_only_inside_the_selected_root() {
        let root = tempfile::tempdir().unwrap();
        let inside = root.path().join("my-overrides");
        fs::write(
            root.path().join("retroarch.cfg"),
            format!("rgui_config_directory = \"{}\"\n", inside.display()),
        )
        .unwrap();
        let plan = prepare_retroarch_bezel_plan(&launched(root.path(), "Game (USA).sfc", "Snes9x"))
            .unwrap();
        assert_eq!(override_of(&plan), inside.join("Snes9x/Game (USA).cfg"));
        // RetroArch saves this setting as `~/...` with a trailing slash.
        fs::write(
            root.path().join("retroarch.cfg"),
            "rgui_config_directory = \"~/my-overrides/\"\n",
        )
        .unwrap();
        let plan = prepare_retroarch_bezel_plan(&launched(root.path(), "Game (USA).sfc", "Snes9x"))
            .unwrap();
        assert_eq!(override_of(&plan), inside.join("Snes9x/Game (USA).cfg"));
        for value in [
            "~other/config",
            ":/config",
            "relative/config",
            "/elsewhere/config",
        ] {
            fs::write(
                root.path().join("retroarch.cfg"),
                format!("rgui_config_directory = \"{value}\"\n"),
            )
            .unwrap();
            let plan =
                prepare_retroarch_bezel_plan(&launched(root.path(), "Game (USA).sfc", "Snes9x"))
                    .unwrap();
            assert_eq!(
                plan.refusals,
                vec![BezelPlanRefusal::RetroArchConfigScopeInvalid],
                "{value}"
            );
        }
    }

    /// RetroArch rejects an overlay config without `overlay0_descs`; the
    /// written descriptor must always declare it.
    #[test]
    fn descriptor_declares_descs_so_retroarch_accepts_it() {
        let text = String::from_utf8(overlay_descriptor("bezel.png")).unwrap();
        assert!(text.contains("overlay0_descs = 0\n"));
        assert!(text.contains("overlay0_overlay = \"bezel.png\"\n"));
    }
}
