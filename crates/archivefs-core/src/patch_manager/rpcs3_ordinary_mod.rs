//! Safe inspection and shared-transaction planning for ordinary RPCS3 mods.
//!
//! The package must carry structural PS3 identity in a bounded PARAM.SFO.
//! Names and archive paths are display metadata only. A `PS3_GAME` directory
//! is treated as the harmless disc-layout wrapper it is: its contents map to
//! the selected RPCS3 `dev_hdd0/game/<TITLE_ID>` root without flattening any
//! descendants.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use super::rpcs3_local::Rpcs3Profile;
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
use crate::game_identity::{GameIdentityReport, IdentityPlatform};
use crate::param_sfo::{SfoValue, parse_param_sfo};

pub const RPCS3_ORDINARY_MOD_SOURCE_MODE: &str = "rpcs3_ordinary_mod";
pub const RPCS3_ORDINARY_MOD_MAX_FILES: usize = 256;
pub const RPCS3_ORDINARY_MOD_MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
pub const RPCS3_ORDINARY_MOD_MAX_SFO_BYTES: u64 = 1024 * 1024;
const ARCHIVE_MAX_BYTES: u64 = 512 * 1024 * 1024;
const ARCHIVE_MAX_MEMBERS: usize = 10_000;
const ARCHIVE_MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rpcs3OrdinaryModErrorKind {
    SourceNotFound,
    SourceNotRegular,
    SourceSymlink,
    ArchiveRejected,
    MissingIdentity,
    ConflictingIdentity,
    MalformedIdentity,
    EmptyPackage,
    AmbiguousWrapper,
    UnsafeMember,
    ResourceLimit,
    ProfileUnavailable,
    ProfileUnsafe,
    IdentityUnavailable,
    IdentityConflict,
    PreviewFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3OrdinaryModError {
    pub kind: Rpcs3OrdinaryModErrorKind,
    pub detail: String,
}

impl std::fmt::Display for Rpcs3OrdinaryModError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}
impl std::error::Error for Rpcs3OrdinaryModError {}

fn error(kind: Rpcs3OrdinaryModErrorKind, detail: impl Into<String>) -> Rpcs3OrdinaryModError {
    Rpcs3OrdinaryModError { kind, detail: detail.into() }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3OrdinaryModInspection {
    pub source_path: PathBuf,
    pub source_root: PathBuf,
    pub mod_name: String,
    pub declared_title_id: String,
    pub identity_relative: PathBuf,
    pub files: Vec<PathBuf>,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3OrdinaryModArchiveInspection {
    pub archive_path: PathBuf,
    pub staging_root: PathBuf,
    pub inspection: Rpcs3OrdinaryModInspection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3OrdinaryModPlan {
    pub inspection: Rpcs3OrdinaryModInspection,
    pub target_title_id: String,
    pub destination_root: PathBuf,
    pub report: SharedPreviewReport,
}

#[derive(Debug)]
pub struct Rpcs3OrdinaryModApplyResult {
    pub apply: SharedApplyResult,
    pub rollback: Option<SharedRollbackResult>,
}

fn safe_component(value: &str) -> bool {
    let mut components = Path::new(value).components();
    matches!((components.next(), components.next()), (Some(Component::Normal(_)), None))
        && !value.is_empty()
}

fn is_safe_regular_file(metadata: &fs::Metadata) -> bool {
    metadata.is_file()
        && {
            #[cfg(unix)]
            {
                metadata.nlink() == 1
            }
            #[cfg(not(unix))]
            {
                true
            }
        }
}

fn read_title_id(path: &Path) -> Result<String, Rpcs3OrdinaryModError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| {
        error(Rpcs3OrdinaryModErrorKind::MissingIdentity, format!("PARAM.SFO cannot be read: {e}"))
    })?;
    if metadata.file_type().is_symlink() || !is_safe_regular_file(&metadata) {
        return Err(error(Rpcs3OrdinaryModErrorKind::UnsafeMember, "PARAM.SFO is not a regular file"));
    }
    if metadata.len() > RPCS3_ORDINARY_MOD_MAX_SFO_BYTES {
        return Err(error(Rpcs3OrdinaryModErrorKind::ResourceLimit, "PARAM.SFO exceeds the inspection limit"));
    }
    let bytes = fs::read(path).map_err(|e| error(Rpcs3OrdinaryModErrorKind::MalformedIdentity, e.to_string()))?;
    let sfo = parse_param_sfo(&bytes).ok_or_else(|| error(
        Rpcs3OrdinaryModErrorKind::MalformedIdentity,
        "PARAM.SFO is malformed or unsupported",
    ))?;
    let value = match sfo.get("TITLE_ID") {
        Some(SfoValue::Text(value)) => value.trim().to_ascii_uppercase(),
        _ => return Err(error(Rpcs3OrdinaryModErrorKind::MissingIdentity, "PARAM.SFO has no textual TITLE_ID")),
    };
    if value.len() != 9 || !value.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(error(Rpcs3OrdinaryModErrorKind::MalformedIdentity, "PARAM.SFO TITLE_ID is not a valid PS3 title ID"));
    }
    Ok(value)
}

fn identity_candidates(root: &Path) -> Vec<PathBuf> {
    [root.join("PARAM.SFO"), root.join("PS3_GAME/PARAM.SFO")]
        .into_iter()
        .filter(|path| path.exists())
        .collect()
}

fn locate_root(source_path: &Path) -> Result<PathBuf, Rpcs3OrdinaryModError> {
    if !source_path.is_absolute() {
        return Err(error(Rpcs3OrdinaryModErrorKind::SourceNotFound, "RPCS3 mod source path must be absolute"));
    }
    let metadata = fs::symlink_metadata(source_path).map_err(|e| error(Rpcs3OrdinaryModErrorKind::SourceNotFound, e.to_string()))?;
    if metadata.file_type().is_symlink() {
        return Err(error(Rpcs3OrdinaryModErrorKind::SourceSymlink, "RPCS3 mod source is a symlink"));
    }
    if !metadata.is_dir() {
        return Err(error(Rpcs3OrdinaryModErrorKind::SourceNotRegular, "RPCS3 mod source must be a folder or ZIP"));
    }
    if !identity_candidates(source_path).is_empty() {
        return Ok(source_path.to_path_buf());
    }
    let mut roots = Vec::new();
    for entry in fs::read_dir(source_path).map_err(|e| error(Rpcs3OrdinaryModErrorKind::SourceNotFound, e.to_string()))? {
        let entry = entry.map_err(|e| error(Rpcs3OrdinaryModErrorKind::SourceNotFound, e.to_string()))?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|e| error(Rpcs3OrdinaryModErrorKind::SourceNotFound, e.to_string()))?;
        if metadata.file_type().is_symlink() {
            return Err(error(Rpcs3OrdinaryModErrorKind::UnsafeMember, "wrapper contains a symlink"));
        }
        if metadata.is_dir() && !identity_candidates(&entry.path()).is_empty() {
            roots.push(entry.path());
        }
    }
    match roots.len() {
        1 => Ok(roots.remove(0)),
        0 => Err(error(Rpcs3OrdinaryModErrorKind::MissingIdentity, "no unambiguous PARAM.SFO identity was found")),
        _ => Err(error(Rpcs3OrdinaryModErrorKind::AmbiguousWrapper, "package contains multiple possible RPCS3 mod roots")),
    }
}

fn inspect_root(source_path: &Path, root: &Path) -> Result<Rpcs3OrdinaryModInspection, Rpcs3OrdinaryModError> {
    let candidates = identity_candidates(root);
    if candidates.is_empty() {
        return Err(error(Rpcs3OrdinaryModErrorKind::MissingIdentity, "RPCS3 mod must contain PARAM.SFO or PS3_GAME/PARAM.SFO"));
    }
    let mut ids = BTreeSet::new();
    for candidate in &candidates {
        ids.insert(read_title_id(candidate)?);
    }
    if ids.len() != 1 {
        return Err(error(Rpcs3OrdinaryModErrorKind::ConflictingIdentity, "package contains conflicting PS3 TITLE_ID evidence"));
    }
    let declared_title_id = ids.into_iter().next().expect("one identity checked");
    let identity_relative = candidates[0].strip_prefix(root).map_err(|_| error(Rpcs3OrdinaryModErrorKind::UnsafeMember, "identity escaped package root"))?.to_path_buf();
    let mut files = Vec::new();
    let mut total = 0_u64;
    fn walk(root: &Path, current: &Path, files: &mut Vec<PathBuf>, total: &mut u64) -> Result<(), Rpcs3OrdinaryModError> {
        let mut entries: Vec<_> = fs::read_dir(current).map_err(|e| error(Rpcs3OrdinaryModErrorKind::SourceNotFound, e.to_string()))?.collect::<Result<Vec<_>, _>>().map_err(|e| error(Rpcs3OrdinaryModErrorKind::SourceNotFound, e.to_string()))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|e| error(Rpcs3OrdinaryModErrorKind::SourceNotFound, e.to_string()))?;
            if metadata.file_type().is_symlink()
                || (!is_safe_regular_file(&metadata) && !metadata.is_dir())
            {
                return Err(error(Rpcs3OrdinaryModErrorKind::UnsafeMember, format!("unsafe RPCS3 mod member: {}", path.display())));
            }
            if metadata.is_dir() {
                walk(root, &path, files, total)?;
            } else {
                let relative = path.strip_prefix(root).map_err(|_| error(Rpcs3OrdinaryModErrorKind::UnsafeMember, "member escaped package root"))?.to_path_buf();
                if relative.components().any(|component| !matches!(component, Component::Normal(_))) {
                    return Err(error(Rpcs3OrdinaryModErrorKind::UnsafeMember, "package member contains unsafe path components"));
                }
                if files.len() >= RPCS3_ORDINARY_MOD_MAX_FILES {
                    return Err(error(Rpcs3OrdinaryModErrorKind::ResourceLimit, "RPCS3 mod file-count limit reached"));
                }
                *total = total.saturating_add(metadata.len());
                if *total > RPCS3_ORDINARY_MOD_MAX_TOTAL_BYTES {
                    return Err(error(Rpcs3OrdinaryModErrorKind::ResourceLimit, "RPCS3 mod expanded-size limit reached"));
                }
                files.push(relative);
            }
        }
        Ok(())
    }
    walk(root, root, &mut files, &mut total)?;
    files.retain(|path| path != &identity_relative && path != Path::new("PARAM.SFO") && path != Path::new("PS3_GAME/PARAM.SFO"));
    if files.is_empty() {
        return Err(error(Rpcs3OrdinaryModErrorKind::EmptyPackage, "RPCS3 mod contains no installable files"));
    }
    files.sort();
    let mod_name = root.file_name().and_then(|value| value.to_str()).filter(|value| safe_component(value)).unwrap_or("RPCS3 mod").to_string();
    Ok(Rpcs3OrdinaryModInspection { source_path: source_path.to_path_buf(), source_root: root.to_path_buf(), mod_name, declared_title_id, identity_relative, files, total_bytes: total })
}

pub fn inspect_rpcs3_ordinary_mod(source_path: &Path) -> Result<Rpcs3OrdinaryModInspection, Rpcs3OrdinaryModError> {
    let root = locate_root(source_path)?;
    inspect_root(source_path, &root)
}

pub fn inspect_rpcs3_ordinary_mod_zip(source_path: &Path) -> Result<Rpcs3OrdinaryModArchiveInspection, Rpcs3OrdinaryModError> {
    let metadata = fs::symlink_metadata(source_path).map_err(|e| error(Rpcs3OrdinaryModErrorKind::SourceNotFound, e.to_string()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(error(Rpcs3OrdinaryModErrorKind::SourceNotRegular, "RPCS3 mod archive must be a regular file"));
    }
    if metadata.len() > ARCHIVE_MAX_BYTES {
        return Err(error(Rpcs3OrdinaryModErrorKind::ResourceLimit, "RPCS3 mod archive exceeds the compressed-size limit"));
    }
    let staging_root = std::env::temp_dir().join(format!("archivefs-rpcs3-mod-{}-{}", std::process::id(), stage_id()));
    fs::create_dir(&staging_root).map_err(|e| error(Rpcs3OrdinaryModErrorKind::ArchiveRejected, e.to_string()))?;
    let trusted = crate::safe_read::TrustedRoots::from_paths(source_path.parent());
    let limits = crate::dat::archive::limits::ArchiveLimits {
        max_members: ARCHIVE_MAX_MEMBERS,
        max_member_logical_bytes: RPCS3_ORDINARY_MOD_MAX_TOTAL_BYTES,
        max_archive_logical_bytes: ARCHIVE_MAX_TOTAL_BYTES,
        max_solid_decode_bytes: crate::dat::archive::limits::MAX_SOLID_DECODE_BYTES,
        max_dictionary_bytes: crate::dat::archive::limits::MAX_7Z_DICTIONARY_BYTES,
        max_compression_ratio: 1_000,
        max_header_bytes: crate::dat::archive::limits::MAX_7Z_HEADER_BYTES,
        max_zip_central_directory_bytes: crate::dat::archive::limits::MAX_ZIP_CENTRAL_DIRECTORY_BYTES,
        max_aggregate_decoder_memory_bytes: crate::dat::archive::limits::MAX_7Z_AGGREGATE_DECODER_MEMORY_BYTES,
        max_coders_per_folder: crate::dat::archive::limits::MAX_7Z_CODERS_PER_FOLDER,
    };
    let extracted = crate::dat::archive::zip::extract_zip_members_to(source_path, &trusted, &limits, &AtomicBool::new(false), &staging_root).map_err(|e| {
        let _ = fs::remove_dir_all(&staging_root);
        error(Rpcs3OrdinaryModErrorKind::ArchiveRejected, format!("ZIP refused: {e:?}"))
    })?;
    if extracted.is_empty() {
        let _ = fs::remove_dir_all(&staging_root);
        return Err(error(Rpcs3OrdinaryModErrorKind::EmptyPackage, "RPCS3 mod archive contains no files"));
    }
    let root = match locate_root(&staging_root) {
        Ok(root) => root,
        Err(error_value) => { let _ = fs::remove_dir_all(&staging_root); return Err(error_value); }
    };
    let inspection = match inspect_root(source_path, &root) {
        Ok(value) => value,
        Err(error_value) => { let _ = fs::remove_dir_all(&staging_root); return Err(error_value); }
    };
    Ok(Rpcs3OrdinaryModArchiveInspection { archive_path: source_path.to_path_buf(), staging_root, inspection })
}

pub fn rpcs3_ordinary_mod_destination_root(profile: &Rpcs3Profile, title_id: &str) -> Result<PathBuf, Rpcs3OrdinaryModError> {
    if !profile.eligible {
        return Err(error(Rpcs3OrdinaryModErrorKind::ProfileUnavailable, "RPCS3 profile is not eligible"));
    }
    let title_id = title_id.trim().to_ascii_uppercase();
    if title_id.len() != 9 || !title_id.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(error(Rpcs3OrdinaryModErrorKind::IdentityUnavailable, "a verified PS3 title ID is required"));
    }
    let root = profile.games_path.join(title_id);
    if !root.is_absolute() || root.parent().is_none() {
        return Err(error(Rpcs3OrdinaryModErrorKind::ProfileUnsafe, "derived RPCS3 game destination is unsafe"));
    }
    Ok(root)
}

fn destination_relative(inspection: &Rpcs3OrdinaryModInspection, relative: &Path) -> PathBuf {
    if inspection.identity_relative.starts_with("PS3_GAME") {
        relative.strip_prefix("PS3_GAME").unwrap_or(relative).to_path_buf()
    } else {
        relative.to_path_buf()
    }
}

pub fn build_rpcs3_ordinary_mod_plan(
    inspection: &Rpcs3OrdinaryModInspection,
    identity: &GameIdentityReport,
    destination_root: &Path,
) -> Result<Rpcs3OrdinaryModPlan, Rpcs3OrdinaryModError> {
    if identity.platform != IdentityPlatform::PlayStation3 {
        return Err(error(Rpcs3OrdinaryModErrorKind::IdentityUnavailable, "the selected game is not verified as PlayStation 3"));
    }
    let Some(title_id) = identity.verified_ps3_title_id().map(str::to_ascii_uppercase) else {
        return Err(error(Rpcs3OrdinaryModErrorKind::IdentityUnavailable, "a verified PS3 title ID is required; EmuWiz will not guess from names"));
    };
    if inspection.declared_title_id != title_id {
        return Err(error(Rpcs3OrdinaryModErrorKind::IdentityConflict, "mod PARAM.SFO TITLE_ID conflicts with the selected game's verified title ID"));
    }
    let source_items = inspection.files.iter().map(|relative| PreviewSourceItem {
        adapter: PreviewAdapter::Rpcs3OrdinaryMod,
        source_path: inspection.source_root.join(relative),
        expected_source_digest: None,
        destination_relative_paths: vec![destination_relative(inspection, relative)],
        match_strength: PreviewMatchStrength::VerifiedExact,
    }).collect();
    let report = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::Rpcs3OrdinaryMod,
        selected_archive: identity.archive_path.clone(),
        platform: Some("PS3".into()),
        identity: PreviewIdentity { kind: PreviewIdentityKind::Rpcs3TitleId, state: PreviewIdentityState::Verified, value: Some(title_id.clone()), archive_path: identity.archive_path.clone(), revision: None },
        destination_root: destination_root.to_path_buf(),
        source_items,
    }).map_err(|e| error(Rpcs3OrdinaryModErrorKind::PreviewFailed, e.to_string()))?;
    Ok(Rpcs3OrdinaryModPlan { inspection: inspection.clone(), target_title_id: title_id, destination_root: destination_root.to_path_buf(), report })
}

pub fn build_rpcs3_ordinary_mod_transaction_plan(plan: &Rpcs3OrdinaryModPlan, profile_id: &str) -> Result<SharedTransactionPlan, Rpcs3OrdinaryModError> {
    let mut transaction = build_shared_transaction_plan(&plan.report, profile_id, RPCS3_ORDINARY_MOD_SOURCE_MODE, &plan.inspection.source_root).map_err(|e| error(Rpcs3OrdinaryModErrorKind::PreviewFailed, e.detail))?;
    super::shared_transaction::require_rpcs3_ordinary_mod_verification(&mut transaction).map_err(|e| error(Rpcs3OrdinaryModErrorKind::PreviewFailed, e.detail))?;
    Ok(transaction)
}

pub fn apply_rpcs3_ordinary_mod(plan: &SharedTransactionPlan, options: &SharedApplyOptions) -> Rpcs3OrdinaryModApplyResult {
    Rpcs3OrdinaryModApplyResult { apply: execute_shared_apply(plan, options), rollback: None }
}

pub fn preview_rpcs3_ordinary_mod_rollback(journal_path: &Path, destination_root: &Path, backup_root: &Path) -> super::shared_transaction::SharedRollbackPreview {
    preview_shared_rollback(journal_path, destination_root, backup_root)
}

pub fn rollback_rpcs3_ordinary_mod(preview: &super::shared_transaction::SharedRollbackPreview, options: &SharedRollbackOptions) -> SharedRollbackResult {
    execute_shared_rollback(preview, options)
}

fn stage_id() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|duration| duration.as_nanos()).unwrap_or(0)
}
