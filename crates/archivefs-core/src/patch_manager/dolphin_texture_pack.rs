//! Explicit, expanded multi-file Dolphin texture-pack manifests.
//!
//! This is deliberately a manifest/plan seam, not an archive importer. A
//! caller must provide every source path, destination filename, size, and
//! SHA-256 digest, together with the verified Dolphin GameID it intends to
//! target. Compatibility therefore comes from verified disc identity plus an
//! explicit manifest target, never from a pack filename or directory name.
//!
//! Destination paths are the conservative first slice: each manifest file is
//! one safe filename beneath `Load/Textures/<verified GameID>`. This matches
//! the existing shared preview contract without widening that generic module
//! or inventing nested-pack semantics. Archive ingestion and arbitrary nested
//! texture layouts remain deferred.
//!
//! The disk contract is versioned JSON (`emuwiz.dolphin_texture_pack.v1`),
//! with every source path, destination filename, size, and SHA-256 digest
//! explicit in the manifest. It is intentionally not inferred from a folder
//! or archive name.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::dolphin_texture_mod::{
    DolphinTextureModError, DolphinTextureModErrorKind, DolphinTextureModIdentity,
    validate_dolphin_texture_source,
};
use super::shared_preview::{
    PreviewAdapter, PreviewIdentity, PreviewIdentityKind, PreviewIdentityState,
    PreviewMatchStrength, PreviewProposedAction, PreviewSourceItem, PreviewState,
    SharedPreviewReport, SharedPreviewRequest, build_shared_preview,
};
use super::shared_transaction::{
    SharedApplyOptions, SharedApplyResult, SharedRollbackOptions, SharedRollbackResult,
    SharedTransactionPlan, build_shared_transaction_plan, execute_shared_apply,
    execute_shared_rollback, generate_shared_operation_id, preview_shared_rollback,
};
use crate::game_identity::IdentityPlatform;

#[cfg(test)]
mod tests;

pub const DOLPHIN_TEXTURE_PACK_SOURCE_MODE: &str = "dolphin_texture_pack";
pub const DOLPHIN_TEXTURE_PACK_MANIFEST_FORMAT: &str = "emuwiz.dolphin_texture_pack.v1";
pub const DOLPHIN_TEXTURE_PACK_MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
pub const DOLPHIN_TEXTURE_PACK_MAX_TOTAL_SOURCE_BYTES: u64 =
    super::shared_transaction::SHARED_MAX_TOTAL_WRITTEN_BYTES;
const MAX_PACK_FILES: usize = 256;
const MAX_PACK_NAME_BYTES: usize = 256;
const MAX_PACK_VERSION_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DolphinTexturePackFile {
    pub source_path: PathBuf,
    /// The source path relative to the explicitly selected pack root.
    #[serde(default)]
    pub source_relative_path: PathBuf,
    /// A single filename beneath `<verified GameID>`, not an arbitrary path.
    pub destination_filename: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DolphinTexturePackManifest {
    pub format: String,
    pub name: String,
    pub version: Option<String>,
    pub target_game_id: String,
    /// Absolute root explicitly approved for all manifest source files.
    pub source_root: PathBuf,
    pub files: Vec<DolphinTexturePackFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinTexturePackPreviewRequest {
    pub selected_archive: PathBuf,
    pub identity: DolphinTextureModIdentity,
    pub destination_root: PathBuf,
    pub source_root: PathBuf,
    pub manifest: DolphinTexturePackManifest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinTexturePackBuildRequest {
    pub source_root: PathBuf,
    pub identity: DolphinTextureModIdentity,
    pub name: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinTexturePackRejectedFile {
    pub relative_path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinTexturePackBuildPreview {
    pub manifest: DolphinTexturePackManifest,
    pub rejected: Vec<DolphinTexturePackRejectedFile>,
    pub total_bytes: u64,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinTexturePackPlan {
    pub manifest: DolphinTexturePackManifest,
    pub report: SharedPreviewReport,
}

/// Result of applying one complete texture-pack plan. A partial shared apply
/// is immediately handed to the existing rollback machinery so callers do
/// not leave a pack half-installed.
#[derive(Debug)]
pub struct DolphinTexturePackApplyResult {
    pub apply: SharedApplyResult,
    pub rollback: Option<SharedRollbackResult>,
}

impl DolphinTexturePackPlan {
    pub fn install_count(&self) -> usize {
        self.report
            .entries
            .iter()
            .filter(|entry| entry.proposed_action == PreviewProposedAction::Install)
            .count()
    }

    pub fn replacement_count(&self) -> usize {
        self.report
            .entries
            .iter()
            .filter(|entry| entry.proposed_action == PreviewProposedAction::Replace)
            .count()
    }

    pub fn already_installed_count(&self) -> usize {
        self.report
            .entries
            .iter()
            .filter(|entry| entry.proposed_action == PreviewProposedAction::Skip)
            .count()
    }

    pub fn is_applyable(&self) -> bool {
        self.report.complete
            && !self.report.entries.is_empty()
            && self.report.entries.iter().all(|entry| {
                entry.eligibility == super::shared_preview::PreviewEligibility::Eligible
                    && matches!(
                        entry.state,
                        PreviewState::InstallNew
                            | PreviewState::AlreadyInstalled
                            | PreviewState::ReplaceDifferent
                    )
            })
    }
}

fn pack_error(
    kind: DolphinTextureModErrorKind,
    detail: impl Into<String>,
) -> DolphinTextureModError {
    DolphinTextureModError {
        kind,
        detail: detail.into(),
    }
}

fn safe_manifest_filename(value: &str) -> bool {
    let mut components = Path::new(value).components();
    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(component)), None)
            if !component.is_empty() && component == std::ffi::OsStr::new(value)
    )
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sha256_file(path: &Path) -> Result<(u64, String), DolphinTextureModError> {
    let bytes = fs::read(path).map_err(|error| {
        pack_error(
            DolphinTextureModErrorKind::SourceNotFound,
            format!("{}: {error}", path.display()),
        )
    })?;
    if bytes.len() as u64 > super::shared_transaction::SHARED_MAX_SOURCE_BYTES {
        return Err(pack_error(
            DolphinTextureModErrorKind::SourceTooLarge,
            "texture pack source exceeds the shared transaction size limit",
        ));
    }
    let mut digest = Sha256::new();
    digest.update(&bytes);
    let digest = digest.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    Ok((bytes.len() as u64, hex))
}

/// Validates the explicit manifest and all source files without mutating
/// either source or destination. The source root is also checked after
/// canonicalization so a symlinked parent cannot escape the approved pack
/// directory.
pub fn validate_dolphin_texture_pack_manifest(
    manifest: &DolphinTexturePackManifest,
    identity: &DolphinTextureModIdentity,
    source_root: &Path,
) -> Result<(), DolphinTextureModError> {
    if manifest.format != DOLPHIN_TEXTURE_PACK_MANIFEST_FORMAT {
        return Err(pack_error(
            DolphinTextureModErrorKind::PreviewFailed,
            "unsupported Dolphin texture-pack manifest format",
        ));
    }
    if !matches!(
        identity.platform,
        IdentityPlatform::GameCube | IdentityPlatform::Wii
    ) {
        return Err(pack_error(
            DolphinTextureModErrorKind::WrongPlatform,
            "Dolphin texture packs require a verified GameCube or Wii identity",
        ));
    }
    if manifest.target_game_id != identity.game_id {
        return Err(pack_error(
            DolphinTextureModErrorKind::IdentityUnverified,
            "texture pack target GameID does not match the verified selected game",
        ));
    }
    if manifest.name.is_empty() || manifest.name.len() > MAX_PACK_NAME_BYTES {
        return Err(pack_error(
            DolphinTextureModErrorKind::SourceUnsafeFilename,
            "texture pack name is empty or exceeds the metadata bound",
        ));
    }
    if manifest
        .version
        .as_ref()
        .is_some_and(|version| version.is_empty() || version.len() > MAX_PACK_VERSION_BYTES)
    {
        return Err(pack_error(
            DolphinTextureModErrorKind::SourceUnsafeFilename,
            "texture pack version exceeds the metadata bound",
        ));
    }
    if manifest.files.is_empty() || manifest.files.len() > MAX_PACK_FILES {
        return Err(pack_error(
            DolphinTextureModErrorKind::SourceTooLarge,
            "texture pack file count is empty or exceeds the safe bound",
        ));
    }
    if !source_root.is_absolute() {
        return Err(pack_error(
            DolphinTextureModErrorKind::ProfileRootUnsafe,
            "texture pack source root must be absolute",
        ));
    }
    if manifest.source_root != source_root {
        return Err(pack_error(
            DolphinTextureModErrorKind::SourceOutsideApprovedScope,
            "manifest source root does not match the approved source root",
        ));
    }
    let canonical_root = fs::canonicalize(source_root).map_err(|error| {
        pack_error(
            DolphinTextureModErrorKind::SourceNotFound,
            format!("{}: {error}", source_root.display()),
        )
    })?;
    let mut destinations = BTreeSet::new();
    for file in &manifest.files {
        if !file.source_relative_path.as_os_str().is_empty()
            && (file.source_relative_path.is_absolute()
                || file
                    .source_relative_path
                    .components()
                    .any(|component| matches!(component, Component::ParentDir | Component::CurDir)))
        {
            return Err(pack_error(
                DolphinTextureModErrorKind::SourceOutsideApprovedScope,
                "texture pack source-relative path is unsafe",
            ));
        }
        if !valid_sha256(&file.sha256) {
            return Err(pack_error(
                DolphinTextureModErrorKind::SourceChanged,
                "texture pack manifest contains an invalid SHA-256 digest",
            ));
        }
        if !safe_manifest_filename(&file.destination_filename) {
            return Err(pack_error(
                DolphinTextureModErrorKind::SourceUnsafeFilename,
                "texture pack destination must be one safe filename",
            ));
        }
        let folded = file.destination_filename.to_lowercase();
        if !destinations.insert(folded) {
            return Err(pack_error(
                DolphinTextureModErrorKind::PreviewFailed,
                "texture pack contains duplicate or case-colliding destinations",
            ));
        }
        if !file.source_path.is_absolute() || file.source_path.strip_prefix(source_root).is_err() {
            return Err(pack_error(
                DolphinTextureModErrorKind::SourceOutsideApprovedScope,
                "texture pack source is outside the approved source root",
            ));
        }
        let source = validate_dolphin_texture_source(&file.source_path)?;
        let canonical_source = fs::canonicalize(&source.path).map_err(|error| {
            pack_error(
                DolphinTextureModErrorKind::SourceNotFound,
                format!("{}: {error}", source.path.display()),
            )
        })?;
        if canonical_source.strip_prefix(&canonical_root).is_err() {
            return Err(pack_error(
                DolphinTextureModErrorKind::SourceOutsideApprovedScope,
                "texture pack source resolves outside the approved source root",
            ));
        }
        let (size, digest) = sha256_file(&file.source_path)?;
        if size != file.size_bytes || !digest.eq_ignore_ascii_case(&file.sha256) {
            return Err(pack_error(
                DolphinTextureModErrorKind::SourceChanged,
                format!(
                    "source {} does not match its manifest size/digest",
                    file.source_path.display()
                ),
            ));
        }
    }
    Ok(())
}

fn collect_pack_files(
    root: &Path,
    current: &Path,
    files: &mut Vec<(PathBuf, PathBuf)>,
    rejected: &mut Vec<DolphinTexturePackRejectedFile>,
) -> Result<(), DolphinTextureModError> {
    let mut children = fs::read_dir(current)
        .map_err(|error| {
            pack_error(
                DolphinTextureModErrorKind::SourceNotFound,
                error.to_string(),
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            pack_error(
                DolphinTextureModErrorKind::SourceNotFound,
                error.to_string(),
            )
        })?;
    children.sort_by_key(|entry| entry.file_name());
    for entry in children {
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map(Path::to_path_buf)
            .map_err(|_| {
                pack_error(
                    DolphinTextureModErrorKind::SourceOutsideApprovedScope,
                    "source escaped selected root",
                )
            })?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            pack_error(
                DolphinTextureModErrorKind::SourceNotFound,
                error.to_string(),
            )
        })?;
        if metadata.file_type().is_symlink() {
            rejected.push(DolphinTexturePackRejectedFile {
                relative_path: relative,
                reason: "symlink source is not accepted".to_string(),
            });
        } else if metadata.is_dir() {
            collect_pack_files(root, &path, files, rejected)?;
        } else if metadata.is_file() {
            files.push((path, relative));
        } else {
            rejected.push(DolphinTexturePackRejectedFile {
                relative_path: relative,
                reason: "special files are not accepted".to_string(),
            });
        }
    }
    Ok(())
}

/// Scans one explicitly selected, already-expanded texture-pack directory.
/// Only root-level PNG files are accepted by the current flat destination
/// contract; nested files are reported as deferred rather than guessed into
/// Dolphin's texture namespace. This function never writes to the source.
pub fn build_dolphin_texture_pack_manifest(
    request: &DolphinTexturePackBuildRequest,
) -> Result<DolphinTexturePackBuildPreview, DolphinTextureModError> {
    if !matches!(
        request.identity.platform,
        IdentityPlatform::GameCube | IdentityPlatform::Wii
    ) || request.identity.game_id.is_empty()
    {
        return Err(pack_error(
            DolphinTextureModErrorKind::IdentityUnverified,
            "a verified GameCube/Wii GameID is required to build a texture-pack manifest",
        ));
    }
    if !request.source_root.is_absolute() {
        return Err(pack_error(
            DolphinTextureModErrorKind::ProfileRootUnsafe,
            "texture-pack source root must be absolute",
        ));
    }
    let canonical_root = fs::canonicalize(&request.source_root).map_err(|error| {
        pack_error(
            DolphinTextureModErrorKind::SourceNotFound,
            error.to_string(),
        )
    })?;
    let mut candidates = Vec::new();
    let mut rejected = Vec::new();
    collect_pack_files(
        &request.source_root,
        &request.source_root,
        &mut candidates,
        &mut rejected,
    )?;
    candidates.sort_by(|left, right| left.1.cmp(&right.1));
    let mut accepted = Vec::new();
    let mut total_bytes = 0_u64;
    for (path, relative) in candidates {
        if relative
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty())
        {
            rejected.push(DolphinTexturePackRejectedFile {
                relative_path: relative,
                reason: "nested texture paths are not supported by the current manifest contract"
                    .to_string(),
            });
            continue;
        }
        let source = match validate_dolphin_texture_source(&path) {
            Ok(source) => source,
            Err(error) => {
                rejected.push(DolphinTexturePackRejectedFile {
                    relative_path: relative,
                    reason: error.detail,
                });
                continue;
            }
        };
        let canonical_source = fs::canonicalize(&path).map_err(|error| {
            pack_error(
                DolphinTextureModErrorKind::SourceNotFound,
                error.to_string(),
            )
        })?;
        if canonical_source.strip_prefix(&canonical_root).is_err() {
            rejected.push(DolphinTexturePackRejectedFile {
                relative_path: relative,
                reason: "source resolves outside the selected root".to_string(),
            });
            continue;
        }
        let (size, digest) = sha256_file(&path)?;
        if accepted.len() >= MAX_PACK_FILES {
            rejected.push(DolphinTexturePackRejectedFile {
                relative_path: relative,
                reason: "texture-pack file-count safety limit reached".to_string(),
            });
            continue;
        }
        if total_bytes.saturating_add(size) > DOLPHIN_TEXTURE_PACK_MAX_TOTAL_SOURCE_BYTES {
            rejected.push(DolphinTexturePackRejectedFile {
                relative_path: relative,
                reason: "texture-pack total source-size safety limit reached".to_string(),
            });
            continue;
        }
        total_bytes += size;
        accepted.push(DolphinTexturePackFile {
            source_path: source.path,
            source_relative_path: relative,
            destination_filename: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string(),
            size_bytes: size,
            sha256: digest,
        });
    }
    accepted.sort_by(|left, right| left.source_relative_path.cmp(&right.source_relative_path));
    rejected.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let complete = rejected.is_empty() && !accepted.is_empty();
    Ok(DolphinTexturePackBuildPreview {
        manifest: DolphinTexturePackManifest {
            format: DOLPHIN_TEXTURE_PACK_MANIFEST_FORMAT.to_string(),
            name: request.name.clone(),
            version: request.version.clone(),
            target_game_id: request.identity.game_id.clone(),
            source_root: request.source_root.clone(),
            files: accepted,
        },
        rejected,
        total_bytes,
        complete,
    })
}

/// Builds a read-only multi-file preview through the existing shared preview
/// engine. No destination is written here.
pub fn build_dolphin_texture_pack_preview(
    request: &DolphinTexturePackPreviewRequest,
) -> Result<DolphinTexturePackPlan, DolphinTextureModError> {
    validate_dolphin_texture_pack_manifest(
        &request.manifest,
        &request.identity,
        &request.source_root,
    )?;
    let source_items = request
        .manifest
        .files
        .iter()
        .map(|file| PreviewSourceItem {
            adapter: PreviewAdapter::Dolphin,
            source_path: file.source_path.clone(),
            expected_source_digest: Some(file.sha256.to_ascii_lowercase()),
            destination_relative_paths: vec![
                PathBuf::from(&request.identity.game_id).join(&file.destination_filename),
            ],
            match_strength: PreviewMatchStrength::VerifiedExact,
        })
        .collect();
    let report = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::Dolphin,
        selected_archive: request.selected_archive.clone(),
        platform: Some(
            match request.identity.platform {
                IdentityPlatform::GameCube => "GameCube",
                IdentityPlatform::Wii => "Wii",
                _ => unreachable!(),
            }
            .to_string(),
        ),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::DolphinTexturePack,
            state: PreviewIdentityState::Verified,
            value: Some(request.identity.game_id.clone()),
            archive_path: request.selected_archive.clone(),
            revision: None,
        },
        destination_root: request.destination_root.clone(),
        source_items,
    })
    .map_err(|error| pack_error(DolphinTextureModErrorKind::PreviewFailed, error.to_string()))?;
    Ok(DolphinTexturePackPlan {
        manifest: request.manifest.clone(),
        report,
    })
}

/// Converts an applyable pack preview into the existing shared transaction
/// plan. Replacement is represented by the shared plan and requires the
/// caller's existing explicit replacement approval at apply time.
pub fn build_dolphin_texture_pack_transaction_plan(
    pack: &DolphinTexturePackPlan,
    profile_id: &str,
    source_root: &Path,
) -> Result<SharedTransactionPlan, super::shared_transaction::SharedApplyFailure> {
    if !pack.is_applyable() {
        return Err(super::shared_transaction::SharedApplyFailure {
            kind: super::shared_transaction::SharedApplyFailureKind::InvalidPlan,
            path: None,
            detail: "texture pack preview contains blocked or incomplete entries".to_string(),
        });
    }
    build_shared_transaction_plan(
        &pack.report,
        profile_id,
        DOLPHIN_TEXTURE_PACK_SOURCE_MODE,
        source_root,
    )
}

/// Applies a pack through the shared journaled engine and automatically rolls
/// back a journaled partial apply. If the journal itself could not be written,
/// the shared result retains that failure for the caller's recovery handling.
pub fn execute_dolphin_texture_pack_apply(
    plan: &SharedTransactionPlan,
    options: &SharedApplyOptions,
) -> DolphinTexturePackApplyResult {
    let apply = execute_shared_apply(plan, options);
    let rollback = if apply.journal.status
        == super::shared_transaction::SharedApplyStatus::PartialFailure
    {
        apply
            .journal_path
            .as_deref()
            .zip(plan.destination_root.to_path_buf().ok().as_deref())
            .and_then(|(journal_path, root)| {
                let preview = preview_shared_rollback(journal_path, root, &options.backup_root);
                preview.available.then(|| {
                    execute_shared_rollback(
                        &preview,
                        &SharedRollbackOptions {
                            confirmation: super::shared_transaction::SharedRollbackConfirmation {
                                preview_id: preview.preview_id.clone(),
                                approved: true,
                            },
                            rollback_operation_id: generate_shared_operation_id(),
                            timestamp_unix_seconds: options
                                .timestamp_unix_seconds
                                .saturating_add(1),
                            history_root: options.history_root.clone(),
                            backup_root: options.backup_root.clone(),
                        },
                    )
                })
            })
    } else {
        None
    };
    DolphinTexturePackApplyResult { apply, rollback }
}
