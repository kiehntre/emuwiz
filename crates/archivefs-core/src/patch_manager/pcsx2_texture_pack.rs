//! Safe PCSX2 texture replacement packs.
//!
//! PCSX2's currently proven replacement layout is
//! `<profile>/textures/<verified PS2 serial>/<texture filename>`.  This
//! adapter intentionally supports only that flat, expanded-directory shape;
//! it never infers an identity from a pack name and never unpacks or executes
//! pack content.

use std::fs;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use super::pcsx2_local::Pcsx2Profile;
use super::shared_preview::{
    PreviewAdapter, PreviewIdentity, PreviewIdentityKind, PreviewIdentityState,
    PreviewMatchStrength, PreviewSourceItem, PreviewState, SharedPreviewReport,
    SharedPreviewRequest, build_shared_preview,
};
use super::shared_transaction::{
    SharedApplyOptions, SharedApplyResult, SharedRollbackOptions, SharedRollbackResult,
    SharedTransactionPlan, build_shared_transaction_plan, execute_shared_apply,
    execute_shared_rollback, generate_shared_operation_id, preview_shared_rollback,
};
use crate::game_identity::{GameIdentityReport, IdentityPlatform};

pub const PCSX2_TEXTURE_PACK_SOURCE_MODE: &str = "pcsx2_texture_pack";
pub const PCSX2_TEXTURE_PACK_FORMAT: &str = "emuwiz.pcsx2_texture_pack.v1";
pub const PCSX2_TEXTURE_PACK_MAX_FILES: usize = super::pcsx2_local::PCSX2_MAX_TEXTURE_FILES;
pub const PCSX2_TEXTURE_PACK_MAX_TOTAL_BYTES: u64 =
    super::shared_transaction::SHARED_MAX_TOTAL_WRITTEN_BYTES;
const MAX_PACK_ENTRIES: usize = super::pcsx2_local::PCSX2_MAX_ENTRIES_VISITED;
const MAX_PACK_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2TextureIdentity {
    pub archive_path: PathBuf,
    pub serial: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2TexturePackFile {
    pub source_path: PathBuf,
    pub source_relative_path: PathBuf,
    pub destination_filename: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2TexturePackInspection {
    pub source_root: PathBuf,
    pub files: Vec<Pcsx2TexturePackFile>,
    pub rejected: Vec<Pcsx2TexturePackRejectedFile>,
    pub total_bytes: u64,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2TexturePackRejectedFile {
    pub relative_path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2TexturePackPreviewRequest {
    pub selected_pack: PathBuf,
    pub source_root: PathBuf,
    pub identity: Pcsx2TextureIdentity,
    pub profile: Pcsx2Profile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2TexturePackPlan {
    pub inspection: Pcsx2TexturePackInspection,
    pub report: SharedPreviewReport,
}

#[derive(Debug)]
pub struct Pcsx2TexturePackApplyResult {
    pub apply: SharedApplyResult,
    pub rollback: Option<SharedRollbackResult>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pcsx2TexturePackErrorKind {
    WrongPlatform,
    IdentityUnverified,
    IdentityArchiveMismatch,
    UnsafeSerial,
    ProfileIneligible,
    ProfileRootUnavailable,
    ProfileRootUnsafe,
    SourceNotFound,
    SourceNotDirectory,
    SourceOutsideApprovedScope,
    SourceSymlink,
    SourceSpecialFile,
    UnsupportedContent,
    SourceTooLarge,
    SourceChanged,
    PreviewFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2TexturePackError {
    pub kind: Pcsx2TexturePackErrorKind,
    pub detail: String,
}

impl std::fmt::Display for Pcsx2TexturePackError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.detail)
    }
}
impl std::error::Error for Pcsx2TexturePackError {}

fn error(kind: Pcsx2TexturePackErrorKind, detail: impl Into<String>) -> Pcsx2TexturePackError {
    Pcsx2TexturePackError {
        kind,
        detail: detail.into(),
    }
}

pub fn verified_pcsx2_texture_identity(
    archive_path: &Path,
    report: &GameIdentityReport,
) -> Result<Pcsx2TextureIdentity, Pcsx2TexturePackError> {
    if report.archive_path != archive_path {
        return Err(error(
            Pcsx2TexturePackErrorKind::IdentityArchiveMismatch,
            "identity evidence belongs to a different selected game",
        ));
    }
    if report.platform != IdentityPlatform::PlayStation2 {
        return Err(error(
            Pcsx2TexturePackErrorKind::WrongPlatform,
            "PCSX2 texture replacements require a verified PlayStation 2 identity",
        ));
    }
    let Some(serial) = report.verified_ps2_serial() else {
        return Err(error(
            Pcsx2TexturePackErrorKind::IdentityUnverified,
            "a verified PS2 serial is required; EmuWiz will not guess from a filename or pack name",
        ));
    };
    validate_component(
        serial,
        Pcsx2TexturePackErrorKind::UnsafeSerial,
        "verified PS2 serial",
    )?;
    Ok(Pcsx2TextureIdentity {
        archive_path: archive_path.to_path_buf(),
        serial: serial.to_owned(),
    })
}

pub fn pcsx2_texture_destination_root(
    profile: &Pcsx2Profile,
    identity: &Pcsx2TextureIdentity,
) -> Result<PathBuf, Pcsx2TexturePackError> {
    if !profile.eligible {
        return Err(error(
            Pcsx2TexturePackErrorKind::ProfileIneligible,
            "selected PCSX2 profile is not eligible",
        ));
    }
    if !profile.configuration_path.is_absolute() {
        return Err(error(
            Pcsx2TexturePackErrorKind::ProfileRootUnsafe,
            "PCSX2 profile path is not absolute",
        ));
    }
    validate_component(
        &identity.serial,
        Pcsx2TexturePackErrorKind::UnsafeSerial,
        "verified PS2 serial",
    )?;
    Ok(profile.configuration_path.join("textures"))
}

fn validate_component(
    value: &str,
    kind: Pcsx2TexturePackErrorKind,
    label: &str,
) -> Result<(), Pcsx2TexturePackError> {
    let mut components = Path::new(value).components();
    if !matches!((components.next(), components.next()), (Some(Component::Normal(part)), None) if !part.is_empty() && part == std::ffi::OsStr::new(value))
    {
        return Err(error(
            kind,
            format!("{label} is not one safe path component"),
        ));
    }
    Ok(())
}

fn allowed_texture(name: &str) -> bool {
    matches!(
        name.rsplit('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "bmp"
    )
}

fn hash_file(path: &Path) -> Result<(u64, String), Pcsx2TexturePackError> {
    let bytes = fs::read(path).map_err(|e| {
        error(
            Pcsx2TexturePackErrorKind::SourceNotFound,
            format!("{}: {e}", path.display()),
        )
    })?;
    if bytes.len() as u64 > super::shared_transaction::SHARED_MAX_SOURCE_BYTES {
        return Err(error(
            Pcsx2TexturePackErrorKind::SourceTooLarge,
            "texture exceeds the shared per-file safety bound",
        ));
    }
    if bytes.starts_with(b"#!") || bytes.starts_with(b"\x7fELF") {
        return Err(error(
            Pcsx2TexturePackErrorKind::UnsupportedContent,
            "scripts and executable content are not texture files",
        ));
    }
    let digest = Sha256::digest(&bytes);
    Ok((
        bytes.len() as u64,
        digest.iter().map(|b| format!("{b:02x}")).collect(),
    ))
}

fn collect(
    root: &Path,
    current: &Path,
    depth: usize,
    visited: &mut usize,
    files: &mut Vec<(PathBuf, PathBuf)>,
    rejected: &mut Vec<Pcsx2TexturePackRejectedFile>,
) -> Result<(), Pcsx2TexturePackError> {
    if depth > MAX_PACK_DEPTH {
        return Err(error(
            Pcsx2TexturePackErrorKind::SourceTooLarge,
            "texture pack directory depth exceeds the safety bound",
        ));
    }
    let mut entries = fs::read_dir(current)
        .map_err(|e| error(Pcsx2TexturePackErrorKind::SourceNotFound, e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| error(Pcsx2TexturePackErrorKind::SourceNotFound, e.to_string()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        *visited += 1;
        if *visited > MAX_PACK_ENTRIES {
            return Err(error(
                Pcsx2TexturePackErrorKind::SourceTooLarge,
                "texture pack entry count exceeds the safety bound",
            ));
        }
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| {
                error(
                    Pcsx2TexturePackErrorKind::SourceOutsideApprovedScope,
                    "pack path escaped selected root",
                )
            })?
            .to_path_buf();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|e| error(Pcsx2TexturePackErrorKind::SourceNotFound, e.to_string()))?;
        if metadata.file_type().is_symlink() {
            rejected.push(Pcsx2TexturePackRejectedFile {
                relative_path: relative,
                reason: "symlink escape is rejected".into(),
            });
        } else if metadata.is_dir() {
            collect(root, &path, depth + 1, visited, files, rejected)?;
        } else if metadata.is_file() {
            files.push((path, relative));
        } else {
            rejected.push(Pcsx2TexturePackRejectedFile {
                relative_path: relative,
                reason: "special files are rejected".into(),
            });
        }
    }
    Ok(())
}

pub fn inspect_pcsx2_texture_pack(
    source_root: &Path,
) -> Result<Pcsx2TexturePackInspection, Pcsx2TexturePackError> {
    if !source_root.is_absolute() {
        return Err(error(
            Pcsx2TexturePackErrorKind::SourceOutsideApprovedScope,
            "selected pack root must be absolute",
        ));
    }
    let metadata = fs::symlink_metadata(source_root)
        .map_err(|e| error(Pcsx2TexturePackErrorKind::SourceNotFound, e.to_string()))?;
    if metadata.file_type().is_symlink() {
        return Err(error(
            Pcsx2TexturePackErrorKind::SourceSymlink,
            "selected pack root is a symlink",
        ));
    }
    if !metadata.is_dir() {
        return Err(error(
            Pcsx2TexturePackErrorKind::SourceNotDirectory,
            "selected pack must be an expanded directory",
        ));
    }
    let canonical_root = fs::canonicalize(source_root)
        .map_err(|e| error(Pcsx2TexturePackErrorKind::SourceNotFound, e.to_string()))?;
    let mut candidates = Vec::new();
    let mut rejected = Vec::new();
    let mut visited = 0;
    collect(
        source_root,
        source_root,
        0,
        &mut visited,
        &mut candidates,
        &mut rejected,
    )?;
    candidates.sort_by(|a, b| a.1.cmp(&b.1));
    let mut files = Vec::new();
    let mut total = 0u64;
    let mut complete = true;
    for (path, relative) in candidates {
        if relative.components().count() != 1
            || !allowed_texture(
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default(),
            )
        {
            rejected.push(Pcsx2TexturePackRejectedFile {
                relative_path: relative,
                reason: "only root-level supported image textures are accepted".into(),
            });
            continue;
        }
        let canonical = fs::canonicalize(&path)
            .map_err(|e| error(Pcsx2TexturePackErrorKind::SourceNotFound, e.to_string()))?;
        if canonical.strip_prefix(&canonical_root).is_err() {
            return Err(error(
                Pcsx2TexturePackErrorKind::SourceOutsideApprovedScope,
                "texture resolves outside selected pack",
            ));
        }
        let (size, sha256) = match hash_file(&path) {
            Ok(value) => value,
            Err(e) => {
                rejected.push(Pcsx2TexturePackRejectedFile {
                    relative_path: relative,
                    reason: e.detail,
                });
                continue;
            }
        };
        if files.len() >= PCSX2_TEXTURE_PACK_MAX_FILES
            || total.saturating_add(size) > PCSX2_TEXTURE_PACK_MAX_TOTAL_BYTES
        {
            complete = false;
            rejected.push(Pcsx2TexturePackRejectedFile {
                relative_path: relative,
                reason: "texture-pack file-count or total-size safety limit reached".into(),
            });
            continue;
        }
        total += size;
        files.push(Pcsx2TexturePackFile {
            source_path: path,
            destination_filename: relative.file_name().unwrap().to_string_lossy().into_owned(),
            source_relative_path: relative,
            size_bytes: size,
            sha256,
        });
    }
    Ok(Pcsx2TexturePackInspection {
        source_root: source_root.to_path_buf(),
        files,
        rejected,
        total_bytes: total,
        complete,
    })
}

fn validate_pack(
    request: &Pcsx2TexturePackPreviewRequest,
    inspection: &Pcsx2TexturePackInspection,
) -> Result<PathBuf, Pcsx2TexturePackError> {
    if request.identity.archive_path.as_os_str().is_empty() {
        return Err(error(
            Pcsx2TexturePackErrorKind::IdentityUnverified,
            "selected game identity is unavailable",
        ));
    }
    if request.source_root != inspection.source_root {
        return Err(error(
            Pcsx2TexturePackErrorKind::SourceOutsideApprovedScope,
            "pack inspection root changed",
        ));
    }
    let destination_root = pcsx2_texture_destination_root(&request.profile, &request.identity)?;
    if inspection.files.is_empty() || !inspection.complete || !inspection.rejected.is_empty() {
        return Err(error(
            Pcsx2TexturePackErrorKind::UnsupportedContent,
            "pack contains no complete, fully supported safe file set",
        ));
    }
    Ok(destination_root)
}

pub fn build_pcsx2_texture_pack_preview(
    request: &Pcsx2TexturePackPreviewRequest,
) -> Result<Pcsx2TexturePackPlan, Pcsx2TexturePackError> {
    let inspection = inspect_pcsx2_texture_pack(&request.source_root)?;
    let destination_root = validate_pack(request, &inspection)?;
    let source_items = inspection
        .files
        .iter()
        .map(|file| PreviewSourceItem {
            adapter: PreviewAdapter::Pcsx2,
            source_path: file.source_path.clone(),
            expected_source_digest: Some(file.sha256.clone()),
            destination_relative_paths: vec![
                PathBuf::from(&request.identity.serial).join(&file.destination_filename),
            ],
            match_strength: PreviewMatchStrength::VerifiedExact,
        })
        .collect();
    let report = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::Pcsx2,
        selected_archive: request.identity.archive_path.clone(),
        platform: Some("PS2".into()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::Pcsx2TexturePack,
            state: PreviewIdentityState::Verified,
            value: Some(request.identity.serial.clone()),
            archive_path: request.identity.archive_path.clone(),
            revision: None,
        },
        destination_root,
        source_items,
    })
    .map_err(|e| error(Pcsx2TexturePackErrorKind::PreviewFailed, e.to_string()))?;
    Ok(Pcsx2TexturePackPlan { inspection, report })
}

impl Pcsx2TexturePackPlan {
    pub fn is_applyable(&self) -> bool {
        self.report.complete
            && !self.report.entries.is_empty()
            && self.report.entries.iter().all(|e| {
                e.eligibility == super::shared_preview::PreviewEligibility::Eligible
                    && matches!(
                        e.state,
                        PreviewState::InstallNew
                            | PreviewState::AlreadyInstalled
                            | PreviewState::ReplaceDifferent
                    )
            })
    }
    pub fn create_count(&self) -> usize {
        self.report.summary.install_new
    }
    pub fn replace_count(&self) -> usize {
        self.report.summary.replace_different
    }
    pub fn already_installed_count(&self) -> usize {
        self.report.summary.already_installed
    }
    pub fn conflict_count(&self) -> usize {
        self.report.summary.conflicts + self.report.summary.blocked
    }
}

pub fn build_pcsx2_texture_pack_transaction_plan(
    plan: &Pcsx2TexturePackPlan,
    profile_id: &str,
    source_root: &Path,
) -> Result<SharedTransactionPlan, super::shared_transaction::SharedApplyFailure> {
    if !plan.is_applyable() {
        return Err(super::shared_transaction::SharedApplyFailure {
            kind: super::shared_transaction::SharedApplyFailureKind::InvalidPlan,
            path: None,
            detail: "PCSX2 texture preview is incomplete or blocked".into(),
        });
    }
    build_shared_transaction_plan(
        &plan.report,
        profile_id,
        PCSX2_TEXTURE_PACK_SOURCE_MODE,
        source_root,
    )
}

pub fn execute_pcsx2_texture_pack_apply(
    plan: &SharedTransactionPlan,
    options: &SharedApplyOptions,
) -> Pcsx2TexturePackApplyResult {
    let apply = execute_shared_apply(plan, options);
    let rollback = if apply.journal.status
        == super::shared_transaction::SharedApplyStatus::PartialFailure
    {
        apply.journal_path.as_deref().and_then(|journal| {
            let root = plan.destination_root.to_path_buf().ok()?;
            let preview = preview_shared_rollback(journal, &root, &options.backup_root);
            preview.available.then(|| {
                execute_shared_rollback(
                    &preview,
                    &SharedRollbackOptions {
                        confirmation: super::shared_transaction::SharedRollbackConfirmation {
                            preview_id: preview.preview_id.clone(),
                            approved: true,
                        },
                        rollback_operation_id: generate_shared_operation_id(),
                        timestamp_unix_seconds: options.timestamp_unix_seconds.saturating_add(1),
                        history_root: options.history_root.clone(),
                        backup_root: options.backup_root.clone(),
                    },
                )
            })
        })
    } else {
        None
    };
    Pcsx2TexturePackApplyResult { apply, rollback }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch_manager::pcsx2_local::{
        Pcsx2InstallationType, Pcsx2ProfileScope,
    };

    fn pack() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("pack")).unwrap();
        fs::write(root.path().join("pack/a.png"), b"texture-a").unwrap();
        fs::write(root.path().join("pack/b.webp"), b"texture-b").unwrap();
        root
    }

    fn profile(root: &Path) -> Pcsx2Profile {
        Pcsx2Profile {
            profile_id: "test-profile".into(),
            installation_type: Pcsx2InstallationType::Native,
            scope: Pcsx2ProfileScope::User,
            configuration_path: root.join("config"),
            provenance: "test profile",
            eligible: true,
            blockers: Vec::new(),
            patch_directories: Vec::new(),
            configuration_identity: None,
            executable_candidates: Vec::new(),
        }
    }

    fn plan_for(root: &Path) -> Pcsx2TexturePackPlan {
        let identity = Pcsx2TextureIdentity {
            archive_path: root.join("game.iso"),
            serial: "SLUS-12345".into(),
        };
        build_pcsx2_texture_pack_preview(&Pcsx2TexturePackPreviewRequest {
            selected_pack: root.join("pack"),
            source_root: root.join("pack"),
            identity,
            profile: profile(root),
        })
        .unwrap()
    }

    #[test]
    fn valid_pack_is_bounded_and_deterministic() {
        let root = pack();
        let first = inspect_pcsx2_texture_pack(&root.path().join("pack")).unwrap();
        let second = inspect_pcsx2_texture_pack(&root.path().join("pack")).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.files.len(), 2);
        assert_eq!(first.total_bytes, 18);
    }

    #[test]
    fn nested_and_script_content_are_rejected_without_mutation() {
        let root = pack();
        fs::create_dir(root.path().join("pack/nested")).unwrap();
        fs::write(root.path().join("pack/nested/c.png"), b"nested").unwrap();
        fs::write(root.path().join("pack/run.sh"), b"#!/bin/sh\necho nope").unwrap();
        let before = fs::read(root.path().join("pack/a.png")).unwrap();
        let result = inspect_pcsx2_texture_pack(&root.path().join("pack")).unwrap();
        assert_eq!(result.files.len(), 2);
        assert!(
            result
                .rejected
                .iter()
                .any(|item| item.relative_path == Path::new("nested/c.png"))
        );
        assert!(
            result
                .rejected
                .iter()
                .any(|item| item.relative_path == Path::new("run.sh"))
        );
        assert_eq!(fs::read(root.path().join("pack/a.png")).unwrap(), before);
    }

    #[test]
    fn malformed_pack_does_not_panic() {
        let root = tempfile::tempdir().unwrap();
        assert!(inspect_pcsx2_texture_pack(&root.path().join("missing")).is_err());
        let file = root.path().join("not-a-directory");
        fs::write(&file, b"x").unwrap();
        assert_eq!(
            inspect_pcsx2_texture_pack(&file).unwrap_err().kind,
            Pcsx2TexturePackErrorKind::SourceNotDirectory
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.path().join("escape.png")).unwrap();
        let result = inspect_pcsx2_texture_pack(root.path()).unwrap();
        assert!(
            result
                .rejected
                .iter()
                .any(|item| item.relative_path == Path::new("escape.png"))
        );
    }

    #[test]
    fn relative_pack_root_is_rejected() {
        assert!(inspect_pcsx2_texture_pack(Path::new("relative")).is_err());
    }

    #[test]
    fn destination_uses_proven_pcsx2_texture_layout() {
        let root = tempfile::tempdir().unwrap();
        let identity = Pcsx2TextureIdentity {
            archive_path: root.path().join("game.iso"),
            serial: "SLES-54321".into(),
        };
        assert_eq!(
            pcsx2_texture_destination_root(&profile(root.path()), &identity).unwrap(),
            root.path().join("config/textures")
        );
    }

    #[test]
    fn clean_create_is_previewed() {
        let root = pack();
        fs::rename(root.path().join("pack/a.png"), root.path().join("pack/pack-a.png")).unwrap();
        let plan = plan_for(root.path());
        assert_eq!(plan.create_count(), 2);
        assert_eq!(plan.replace_count(), 0);
        assert_eq!(plan.already_installed_count(), 0);
        assert!(plan.is_applyable());
    }

    #[test]
    fn already_installed_is_detected() {
        let root = pack();
        fs::create_dir_all(root.path().join("config/textures/SLUS-12345")).unwrap();
        fs::copy(root.path().join("pack/a.png"), root.path().join("config/textures/SLUS-12345/a.png")).unwrap();
        fs::copy(root.path().join("pack/b.webp"), root.path().join("config/textures/SLUS-12345/b.webp")).unwrap();
        let plan = plan_for(root.path());
        assert_eq!(plan.already_installed_count(), 2);
        assert_eq!(plan.create_count(), 0);
    }

    #[test]
    fn different_destination_requires_explicit_replacement() {
        let root = pack();
        fs::create_dir_all(root.path().join("config/textures/SLUS-12345")).unwrap();
        fs::write(root.path().join("config/textures/SLUS-12345/a.png"), b"foreign").unwrap();
        let plan = plan_for(root.path());
        assert_eq!(plan.replace_count(), 1);
        assert!(plan.report.entries.iter().any(|entry| entry.explicit_replacement_permission_required));
    }

    #[test]
    fn preview_order_is_deterministic() {
        let root = pack();
        let plan = plan_for(root.path());
        let names = plan.report.entries.iter().map(|entry| entry.destination_relative_path.clone()).collect::<Vec<_>>();
        assert_eq!(names, vec![Some(PathBuf::from("SLUS-12345/a.png")), Some(PathBuf::from("SLUS-12345/b.webp"))]);
    }

    #[test]
    fn oversized_file_count_is_reported() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..=PCSX2_TEXTURE_PACK_MAX_FILES {
            fs::write(root.path().join(format!("{index:04}.png")), b"x").unwrap();
        }
        let inspection = inspect_pcsx2_texture_pack(root.path()).unwrap();
        assert!(!inspection.complete);
        assert_eq!(inspection.files.len(), PCSX2_TEXTURE_PACK_MAX_FILES);
    }
}
