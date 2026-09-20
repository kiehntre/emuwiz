//! Safe PPSSPP texture replacement packs.
//!
//! PPSSPP stores replacement textures below `PSP/TEXTURES/<Disc ID>`.  This
//! adapter deliberately accepts an explicitly selected, expanded directory of
//! root-level PNG files.  Names and folders are presentation only; the target
//! is always the verified PSP Disc ID supplied by the identity layer.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::ppsspp_local::PpssppProfile;
use super::shared_preview::{
    build_shared_preview, PreviewAdapter, PreviewIdentity, PreviewIdentityKind,
    PreviewIdentityState, PreviewMatchStrength, PreviewSourceItem, SharedPreviewReport,
    SharedPreviewRequest,
};
use super::shared_transaction::{
    build_shared_transaction_plan, execute_shared_apply, execute_shared_rollback,
    generate_shared_operation_id, preview_shared_rollback, SharedApplyOptions, SharedApplyResult,
    SharedRollbackOptions, SharedRollbackResult, SharedTransactionPlan,
};
use crate::game_identity::{GameIdentityReport, IdentityPlatform};

pub const PPSSPP_TEXTURE_PACK_SOURCE_MODE: &str = "ppsspp_texture_pack";
pub const PPSSPP_TEXTURE_PACK_MANIFEST_FORMAT: &str = "emuwiz.ppsspp_texture_pack.v1";
pub const PPSSPP_TEXTURE_PACK_MAX_FILES: usize = 2_048;
pub const PPSSPP_TEXTURE_PACK_MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PpssppTexturePackErrorKind {
    WrongPlatform,
    IdentityArchiveMismatch,
    IdentityMissing,
    UnsafeIdentity,
    ProfileIneligible,
    DestinationUnsafe,
    SourceMissing,
    SourceSymlink,
    SourceSpecialFile,
    SourceOutsideApprovedScope,
    SourceChanged,
    SourceTooLarge,
    FileCountLimit,
    TotalSizeLimit,
    UnsafePath,
    MalformedPack,
    PreviewFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppTexturePackError {
    pub kind: PpssppTexturePackErrorKind,
    pub detail: String,
}
impl std::fmt::Display for PpssppTexturePackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}
impl std::error::Error for PpssppTexturePackError {}
fn error(kind: PpssppTexturePackErrorKind, detail: impl Into<String>) -> PpssppTexturePackError {
    PpssppTexturePackError {
        kind,
        detail: detail.into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppTextureIdentity {
    pub disc_id: String,
    pub platform: IdentityPlatform,
}

pub fn verified_ppsspp_texture_identity(
    report: &GameIdentityReport,
    archive: &Path,
) -> Result<PpssppTextureIdentity, PpssppTexturePackError> {
    if report.archive_path != archive {
        return Err(error(
            PpssppTexturePackErrorKind::IdentityArchiveMismatch,
            "identity report belongs to a different selected archive",
        ));
    }
    if report.platform != IdentityPlatform::PlayStation {
        return Err(error(
            PpssppTexturePackErrorKind::WrongPlatform,
            "PPSSPP texture replacement requires a PSP identity",
        ));
    }
    let Some(disc_id) = report.verified_psp_disc_id() else {
        return Err(error(
            PpssppTexturePackErrorKind::IdentityMissing,
            "no verified PSP Disc ID is available",
        ));
    };
    if !is_safe_component(disc_id) {
        return Err(error(
            PpssppTexturePackErrorKind::UnsafeIdentity,
            "verified PSP Disc ID is not a safe path component",
        ));
    }
    Ok(PpssppTextureIdentity {
        disc_id: disc_id.to_owned(),
        platform: report.platform,
    })
}

pub fn ppsspp_texture_destination_root(
    profile: &PpssppProfile,
) -> Result<PathBuf, PpssppTexturePackError> {
    if !profile.eligible {
        return Err(error(
            PpssppTexturePackErrorKind::ProfileIneligible,
            "selected PPSSPP profile is not eligible",
        ));
    }
    if !profile.textures_path.is_absolute() || profile.textures_path.parent().is_none() {
        return Err(error(
            PpssppTexturePackErrorKind::DestinationUnsafe,
            "PPSSPP texture root is not a safe absolute path",
        ));
    }
    Ok(profile.textures_path.clone())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PpssppTexturePackFile {
    pub source_path: PathBuf,
    pub source_relative_path: PathBuf,
    pub destination_filename: String,
    pub size_bytes: u64,
    pub sha256: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PpssppTexturePackManifest {
    pub format: String,
    pub name: String,
    pub target_disc_id: String,
    pub source_root: PathBuf,
    pub files: Vec<PpssppTexturePackFile>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppTexturePackBuildRequest {
    pub source_root: PathBuf,
    pub identity: PpssppTextureIdentity,
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppTexturePackRejectedFile {
    pub relative_path: PathBuf,
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppTexturePackBuildPreview {
    pub manifest: PpssppTexturePackManifest,
    pub rejected: Vec<PpssppTexturePackRejectedFile>,
    pub total_bytes: u64,
    pub complete: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppTexturePackPreviewRequest {
    pub selected_archive: PathBuf,
    pub identity: PpssppTextureIdentity,
    pub destination_root: PathBuf,
    pub source_root: PathBuf,
    pub manifest: PpssppTexturePackManifest,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppTexturePackPlan {
    pub manifest: PpssppTexturePackManifest,
    pub report: SharedPreviewReport,
}
#[derive(Debug)]
pub struct PpssppTexturePackApplyResult {
    pub apply: SharedApplyResult,
    pub rollback: Option<SharedRollbackResult>,
}

fn is_safe_component(value: &str) -> bool {
    let mut c = Path::new(value).components();
    matches!((c.next(), c.next()), (Some(Component::Normal(v)), None) if v == value)
}
fn digest_file(path: &Path) -> Result<(u64, String), PpssppTexturePackError> {
    let bytes = fs::read(path).map_err(|e| {
        error(
            PpssppTexturePackErrorKind::SourceMissing,
            format!("{}: {e}", path.display()),
        )
    })?;
    if bytes.len() as u64 > super::shared_transaction::SHARED_MAX_SOURCE_BYTES {
        return Err(error(
            PpssppTexturePackErrorKind::SourceTooLarge,
            "texture exceeds the shared source-size limit",
        ));
    }
    let mut h = Sha256::new();
    h.update(&bytes);
    let digest = h.finalize();
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok((bytes.len() as u64, hex))
}
fn validate_file(file: &PpssppTexturePackFile, root: &Path) -> Result<(), PpssppTexturePackError> {
    if file.source_path.is_absolute() && file.source_path.strip_prefix(root).is_err() {
        return Err(error(
            PpssppTexturePackErrorKind::SourceOutsideApprovedScope,
            "pack source escapes its selected root",
        ));
    }
    if file.source_path.is_relative()
        || file.source_relative_path.is_absolute()
        || file
            .source_relative_path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(error(
            PpssppTexturePackErrorKind::UnsafePath,
            "pack source path contains traversal or is not absolute",
        ));
    }
    if !is_safe_component(&file.destination_filename) {
        return Err(error(
            PpssppTexturePackErrorKind::UnsafePath,
            "texture destination filename is unsafe",
        ));
    }
    if file.destination_filename
        != file
            .source_relative_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
    {
        return Err(error(
            PpssppTexturePackErrorKind::MalformedPack,
            "manifest destination does not match its selected source",
        ));
    }
    let meta = fs::symlink_metadata(&file.source_path)
        .map_err(|e| error(PpssppTexturePackErrorKind::SourceMissing, e.to_string()))?;
    if meta.file_type().is_symlink() {
        return Err(error(
            PpssppTexturePackErrorKind::SourceSymlink,
            "symlink sources are refused",
        ));
    }
    if !meta.is_file() {
        return Err(error(
            PpssppTexturePackErrorKind::SourceSpecialFile,
            "only regular files are accepted",
        ));
    }
    let canonical_root = fs::canonicalize(root)
        .map_err(|e| error(PpssppTexturePackErrorKind::SourceMissing, e.to_string()))?;
    if fs::canonicalize(&file.source_path)
        .map_err(|e| error(PpssppTexturePackErrorKind::SourceMissing, e.to_string()))?
        .strip_prefix(canonical_root)
        .is_err()
    {
        return Err(error(
            PpssppTexturePackErrorKind::SourceOutsideApprovedScope,
            "source symlink escape refused",
        ));
    }
    let (size, digest) = digest_file(&file.source_path)?;
    if size != file.size_bytes || !digest.eq_ignore_ascii_case(&file.sha256) {
        return Err(error(
            PpssppTexturePackErrorKind::SourceChanged,
            "source changed since pack inspection",
        ));
    }
    Ok(())
}

pub fn build_ppsspp_texture_pack_manifest(
    request: &PpssppTexturePackBuildRequest,
) -> Result<PpssppTexturePackBuildPreview, PpssppTexturePackError> {
    if !request.source_root.is_absolute() {
        return Err(error(
            PpssppTexturePackErrorKind::SourceOutsideApprovedScope,
            "pack root must be absolute",
        ));
    }
    let mut entries = fs::read_dir(&request.source_root)
        .map_err(|e| error(PpssppTexturePackErrorKind::SourceMissing, e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| error(PpssppTexturePackErrorKind::SourceMissing, e.to_string()))?;
    entries.sort_by_key(|e| e.file_name());
    let mut files = Vec::new();
    let mut rejected = Vec::new();
    let mut total: u64 = 0;
    for entry in entries {
        let path = entry.path();
        let relative = PathBuf::from(entry.file_name());
        let meta = fs::symlink_metadata(&path)
            .map_err(|e| error(PpssppTexturePackErrorKind::SourceMissing, e.to_string()))?;
        if !meta.is_file() || meta.file_type().is_symlink() {
            rejected.push(PpssppTexturePackRejectedFile {
                relative_path: relative,
                reason: "symlink or non-regular file".into(),
            });
            continue;
        }
        if path
            .extension()
            .and_then(|e| e.to_str())
            .is_none_or(|e| !e.eq_ignore_ascii_case("png"))
        {
            rejected.push(PpssppTexturePackRejectedFile {
                relative_path: relative,
                reason: "only PNG textures are supported".into(),
            });
            continue;
        }
        if files.len() >= PPSSPP_TEXTURE_PACK_MAX_FILES {
            rejected.push(PpssppTexturePackRejectedFile {
                relative_path: relative,
                reason: "file-count limit reached".into(),
            });
            continue;
        }
        let (size, sha256) = digest_file(&path)?;
        if total.saturating_add(size) > PPSSPP_TEXTURE_PACK_MAX_TOTAL_BYTES {
            rejected.push(PpssppTexturePackRejectedFile {
                relative_path: relative,
                reason: "total expanded-size limit reached".into(),
            });
            continue;
        }
        total += size;
        files.push(PpssppTexturePackFile {
            source_path: path,
            source_relative_path: relative.clone(),
            destination_filename: relative.to_string_lossy().into_owned(),
            size_bytes: size,
            sha256,
        });
    }
    let complete = rejected.is_empty() && !files.is_empty();
    Ok(PpssppTexturePackBuildPreview {
        manifest: PpssppTexturePackManifest {
            format: PPSSPP_TEXTURE_PACK_MANIFEST_FORMAT.into(),
            name: request.name.clone(),
            target_disc_id: request.identity.disc_id.clone(),
            source_root: request.source_root.clone(),
            files,
        },
        rejected: rejected.clone(),
        total_bytes: total,
        complete,
    })
}

pub fn validate_ppsspp_texture_pack_manifest(
    manifest: &PpssppTexturePackManifest,
    identity: &PpssppTextureIdentity,
    root: &Path,
) -> Result<(), PpssppTexturePackError> {
    if manifest.format != PPSSPP_TEXTURE_PACK_MANIFEST_FORMAT || manifest.files.is_empty() {
        return Err(error(
            PpssppTexturePackErrorKind::MalformedPack,
            "unsupported or empty PPSSPP texture pack",
        ));
    }
    if manifest.target_disc_id != identity.disc_id {
        return Err(error(
            PpssppTexturePackErrorKind::IdentityMissing,
            "texture pack Disc ID does not match the verified selected game",
        ));
    }
    if manifest.source_root != root || !root.is_absolute() {
        return Err(error(
            PpssppTexturePackErrorKind::SourceOutsideApprovedScope,
            "pack root is not the approved absolute source root",
        ));
    }
    if manifest.files.len() > PPSSPP_TEXTURE_PACK_MAX_FILES {
        return Err(error(
            PpssppTexturePackErrorKind::FileCountLimit,
            "texture pack exceeds the file-count bound",
        ));
    }
    let mut names = BTreeSet::new();
    let mut total: u64 = 0;
    for file in &manifest.files {
        validate_file(file, root)?;
        if !names.insert(file.destination_filename.to_ascii_lowercase()) {
            return Err(error(
                PpssppTexturePackErrorKind::MalformedPack,
                "duplicate texture destination",
            ));
        }
        total = total.saturating_add(file.size_bytes);
    }
    if total > PPSSPP_TEXTURE_PACK_MAX_TOTAL_BYTES {
        return Err(error(
            PpssppTexturePackErrorKind::TotalSizeLimit,
            "texture pack exceeds the expanded-size bound",
        ));
    }
    Ok(())
}

pub fn build_ppsspp_texture_pack_preview(
    request: &PpssppTexturePackPreviewRequest,
) -> Result<PpssppTexturePackPlan, PpssppTexturePackError> {
    validate_ppsspp_texture_pack_manifest(
        &request.manifest,
        &request.identity,
        &request.source_root,
    )?;
    let source_items = request
        .manifest
        .files
        .iter()
        .map(|file| PreviewSourceItem {
            adapter: PreviewAdapter::Ppsspp,
            source_path: file.source_path.clone(),
            expected_source_digest: Some(file.sha256.to_ascii_lowercase()),
            destination_relative_paths: vec![
                PathBuf::from(&request.identity.disc_id).join(&file.destination_filename)
            ],
            match_strength: PreviewMatchStrength::VerifiedExact,
        })
        .collect();
    let report = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::Ppsspp,
        selected_archive: request.selected_archive.clone(),
        platform: Some("PSP".into()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::PpssppDiscId,
            state: PreviewIdentityState::Verified,
            value: Some(request.identity.disc_id.clone()),
            archive_path: request.selected_archive.clone(),
            revision: None,
        },
        destination_root: request.destination_root.clone(),
        source_items,
    })
    .map_err(|e| error(PpssppTexturePackErrorKind::PreviewFailed, e.to_string()))?;
    Ok(PpssppTexturePackPlan {
        manifest: request.manifest.clone(),
        report,
    })
}
impl PpssppTexturePackPlan {
    pub fn is_applyable(&self) -> bool {
        self.report.complete
            && !self.report.entries.is_empty()
            && self.report.entries.iter().all(|e| {
                matches!(
                    e.state,
                    super::shared_preview::PreviewState::InstallNew
                        | super::shared_preview::PreviewState::AlreadyInstalled
                        | super::shared_preview::PreviewState::ReplaceDifferent
                ) && e.eligibility == super::shared_preview::PreviewEligibility::Eligible
            })
    }
}

pub fn build_ppsspp_texture_pack_transaction_plan(
    pack: &PpssppTexturePackPlan,
    profile_id: &str,
    source_root: &Path,
) -> Result<SharedTransactionPlan, super::shared_transaction::SharedApplyFailure> {
    if !pack.is_applyable() {
        return Err(super::shared_transaction::SharedApplyFailure {
            kind: super::shared_transaction::SharedApplyFailureKind::InvalidPlan,
            path: None,
            detail: "PPSSPP preview contains blocked entries".into(),
        });
    }
    build_shared_transaction_plan(
        &pack.report,
        profile_id,
        PPSSPP_TEXTURE_PACK_SOURCE_MODE,
        source_root,
    )
}
pub fn execute_ppsspp_texture_pack_apply(
    plan: &SharedTransactionPlan,
    options: &SharedApplyOptions,
) -> PpssppTexturePackApplyResult {
    let apply = execute_shared_apply(plan, options);
    let rollback = if apply.journal.status
        == super::shared_transaction::SharedApplyStatus::PartialFailure
    {
        apply.journal_path.as_deref().and_then(|path| {
            plan.destination_root.to_path_buf().ok().and_then(|root| {
                let preview = preview_shared_rollback(path, &root, &options.backup_root);
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
        })
    } else {
        None
    };
    PpssppTexturePackApplyResult { apply, rollback }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dir(label: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("emuwiz-ppsspp-{label}-"))
            .tempdir()
            .unwrap()
    }
    fn identity() -> PpssppTextureIdentity {
        PpssppTextureIdentity {
            disc_id: "ULUS-12345".into(),
            platform: IdentityPlatform::PlayStation,
        }
    }
    fn digest(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }
    fn manifest(root: &Path, name: &str, bytes: &[u8]) -> PpssppTexturePackManifest {
        let path = root.join(name);
        fs::write(&path, bytes).unwrap();
        PpssppTexturePackManifest {
            format: PPSSPP_TEXTURE_PACK_MANIFEST_FORMAT.into(),
            name: "Pack".into(),
            target_disc_id: "ULUS-12345".into(),
            source_root: root.to_path_buf(),
            files: vec![PpssppTexturePackFile {
                source_path: path,
                source_relative_path: PathBuf::from(name),
                destination_filename: name.into(),
                size_bytes: bytes.len() as u64,
                sha256: digest(bytes),
            }],
        }
    }
    #[test]
    fn valid_pack_is_deterministic_and_binds_disc_id() {
        let root = dir("valid");
        fs::write(root.path().join("z.png"), b"z").unwrap();
        fs::write(root.path().join("a.png"), b"a").unwrap();
        let request = PpssppTexturePackBuildRequest {
            source_root: root.path().into(),
            identity: identity(),
            name: "Pack".into(),
        };
        let first = build_ppsspp_texture_pack_manifest(&request).unwrap();
        assert_eq!(first, build_ppsspp_texture_pack_manifest(&request).unwrap());
        assert!(first.complete);
        assert_eq!(first.manifest.files[0].destination_filename, "a.png");
    }
    #[test]
    fn mismatch_and_traversal_fail_closed() {
        let root = dir("identity");
        let mut pack = manifest(root.path(), "a.png", b"a");
        let mut other = identity();
        other.disc_id = "ULUS-99999".into();
        assert_eq!(
            validate_ppsspp_texture_pack_manifest(&pack, &other, root.path())
                .unwrap_err()
                .kind,
            PpssppTexturePackErrorKind::IdentityMissing
        );
        pack.files[0].destination_filename = "../escape.png".into();
        assert_eq!(
            validate_ppsspp_texture_pack_manifest(&pack, &identity(), root.path())
                .unwrap_err()
                .kind,
            PpssppTexturePackErrorKind::UnsafePath
        );
    }
    #[test]
    fn preview_distinguishes_create_already_installed_and_replace() {
        let root = dir("preview");
        let destination = root.path().join("PSP/TEXTURES");
        fs::create_dir_all(destination.join("ULUS-12345")).unwrap();
        let request = PpssppTexturePackPreviewRequest {
            selected_archive: root.path().join("game.iso"),
            identity: identity(),
            destination_root: destination.clone(),
            source_root: root.path().into(),
            manifest: manifest(root.path(), "new.png", b"new"),
        };
        let plan = build_ppsspp_texture_pack_preview(&request).unwrap();
        assert!(matches!(
            plan.report.entries[0].state,
            super::super::shared_preview::PreviewState::InstallNew
        ));
        fs::write(destination.join("ULUS-12345/new.png"), b"new").unwrap();
        let plan = build_ppsspp_texture_pack_preview(&request).unwrap();
        assert!(matches!(
            plan.report.entries[0].state,
            super::super::shared_preview::PreviewState::AlreadyInstalled
        ));
        fs::write(destination.join("ULUS-12345/new.png"), b"foreign").unwrap();
        let plan = build_ppsspp_texture_pack_preview(&request).unwrap();
        assert!(matches!(
            plan.report.entries[0].state,
            super::super::shared_preview::PreviewState::ReplaceDifferent
        ));
    }
    #[test]
    fn apply_and_rollback_are_transactional_and_source_stays_untouched() {
        let root = dir("apply");
        let destination = root.path().join("PSP/TEXTURES");
        fs::create_dir_all(&destination).unwrap();
        let request = PpssppTexturePackPreviewRequest {
            selected_archive: root.path().join("game.iso"),
            identity: identity(),
            destination_root: destination.clone(),
            source_root: root.path().into(),
            manifest: manifest(root.path(), "new.png", b"source"),
        };
        let plan = build_ppsspp_texture_pack_preview(&request).unwrap();
        let transaction =
            build_ppsspp_texture_pack_transaction_plan(&plan, "ppsspp", root.path()).unwrap();
        let storage = dir("apply-storage");
        let history = storage.path().join("history");
        let backups = storage.path().join("backups");
        let applied = execute_shared_apply(
            &transaction,
            &SharedApplyOptions {
                dry_run: false,
                confirmation: Some(super::super::shared_transaction::SharedApplyConfirmation {
                    plan_id: transaction.plan_id.clone(),
                    general_approved: true,
                    replacement_approved: true,
                }),
                operation_id: generate_shared_operation_id(),
                timestamp_unix_seconds: 1_700_000_000,
                current_context: transaction.context.clone(),
                history_root: history.clone(),
                backup_root: backups.clone(),
            },
        );
        assert_eq!(
            applied.journal.status,
            super::super::shared_transaction::SharedApplyStatus::Success,
            "{:#?}",
            applied.journal
        );
        assert_eq!(fs::read(root.path().join("new.png")).unwrap(), b"source");
        assert_eq!(
            fs::read(destination.join("ULUS-12345/new.png")).unwrap(),
            b"source"
        );
        let journal = applied.journal_path.unwrap();
        let rollback = preview_shared_rollback(&journal, &destination, &backups);
        let result = execute_shared_rollback(
            &rollback,
            &super::super::shared_transaction::SharedRollbackOptions {
                confirmation: super::super::shared_transaction::SharedRollbackConfirmation {
                    preview_id: rollback.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: generate_shared_operation_id(),
                timestamp_unix_seconds: 1_700_000_001,
                history_root: history,
                backup_root: backups,
            },
        );
        assert_eq!(
            result.status,
            super::super::shared_transaction::SharedApplyStatus::Success
        );
        assert!(!destination.join("ULUS-12345/new.png").exists());
        assert!(root.path().join("new.png").exists());
    }
}
