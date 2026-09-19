//! Read-only inspection and shared-transaction planning for Cemu graphic packs.
//!
//! A real graphic pack is a directory containing a `rules.txt` definition and
//! its nested assets. `rules.txt` is the only accepted target evidence: names
//! and archive paths never identify a game. The package is copied as-is below
//! Cemu's `graphicPacks/<pack-name>` root; no files are flattened or rewritten.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;

use super::cemu_local::{
    CemuInstallationType, CemuProfile, CemuTitleKind, classify_title_kind,
    extract_title_identity, inspect_extracted_layout,
};
use super::shared_preview::{
    PreviewAdapter, PreviewIdentity, PreviewIdentityKind, PreviewIdentityState,
    PreviewMatchStrength, PreviewSourceItem, SharedPreviewReport,
    SharedPreviewRequest, build_shared_preview,
};
use super::shared_transaction::{
    SharedApplyOptions, SharedApplyResult, SharedRollbackOptions, SharedRollbackResult,
    SharedTransactionPlan, build_shared_transaction_plan, execute_shared_apply,
    execute_shared_rollback, preview_shared_rollback,
};
use crate::game_identity::{GameIdentityReport, IdentityKind, IdentityPlatform, IdentityStatus};

pub const CEMU_GRAPHIC_PACK_SOURCE_MODE: &str = "cemu_graphic_pack";
pub const CEMU_GRAPHIC_PACK_MAX_FILES: usize = 128;
pub const CEMU_GRAPHIC_PACK_MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
pub const CEMU_GRAPHIC_PACK_MAX_RULES_BYTES: u64 = 1024 * 1024;
const ARCHIVE_MAX_BYTES: u64 = 512 * 1024 * 1024;
const ARCHIVE_MAX_MEMBERS: usize = 10_000;
const ARCHIVE_MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CemuGraphicPackErrorKind {
    SourceNotFound,
    SourceNotRegular,
    SourceSymlink,
    ArchiveRejected,
    MissingRules,
    MalformedRules,
    AmbiguousPackRoot,
    UnsafePackName,
    UnsafeMember,
    ResourceLimit,
    TargetIdentityMissing,
    TargetIdentityConflict,
    TargetNotDeclared,
    ProfileUnavailable,
    ProfileUnsafe,
    PreviewFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemuGraphicPackError {
    pub kind: CemuGraphicPackErrorKind,
    pub detail: String,
}

impl std::fmt::Display for CemuGraphicPackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}
impl std::error::Error for CemuGraphicPackError {}

fn error(kind: CemuGraphicPackErrorKind, detail: impl Into<String>) -> CemuGraphicPackError {
    CemuGraphicPackError { kind, detail: detail.into() }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemuGraphicPackInspection {
    pub source_path: PathBuf,
    pub source_root: PathBuf,
    pub pack_name: String,
    pub title_ids: Vec<String>,
    pub rules_path: PathBuf,
    pub files: Vec<PathBuf>,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemuGraphicPackArchiveInspection {
    pub archive_path: PathBuf,
    pub staging_root: PathBuf,
    pub inspection: CemuGraphicPackInspection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemuGraphicPackPlan {
    pub inspection: CemuGraphicPackInspection,
    pub target_title_id: String,
    pub destination_root: PathBuf,
    pub report: SharedPreviewReport,
}

#[derive(Debug)]
pub struct CemuGraphicPackApplyResult {
    pub apply: SharedApplyResult,
    pub rollback: Option<SharedRollbackResult>,
}

/// Returns the selected extracted Wii U title's own `meta.xml` ID only when
/// the already-loaded report independently verifies the Wii U platform and
/// belongs to the same selected path. A pack filename never participates in
/// this decision; disc/archive forms without this evidence remain blocked.
pub fn verified_cemu_title_id(
    report: &GameIdentityReport,
    selected_archive: &Path,
) -> Option<String> {
    if report.archive_path != selected_archive || report.platform != IdentityPlatform::WiiU {
        return None;
    }
    if !report
        .evidence
        .iter()
        .any(|fact| fact.kind == IdentityKind::Platform && fact.status == IdentityStatus::Verified)
    {
        return None;
    }
    let layout = inspect_extracted_layout(selected_archive).ok()?;
    let identity = extract_title_identity(layout.meta_xml_path.as_deref()?)?;
    let title_id = identity.title_id?.trim().to_ascii_uppercase();
    (classify_title_kind(&title_id) == CemuTitleKind::Base && valid_title_id(&title_id))
        .then_some(title_id)
}

fn valid_title_id(value: &str) -> bool {
    let value = value.trim();
    value.len() == 16
        && value.bytes().all(|b| b.is_ascii_hexdigit())
        && value[..8].eq_ignore_ascii_case("00050000")
}

fn safe_component(value: &str) -> bool {
    let mut components = Path::new(value).components();
    matches!((components.next(), components.next()), (Some(Component::Normal(_)), None))
        && !value.is_empty()
}

fn parse_rules(path: &Path) -> Result<(String, Vec<String>), CemuGraphicPackError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| error(
        CemuGraphicPackErrorKind::MissingRules,
        format!("rules.txt cannot be read: {e}"),
    ))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(error(CemuGraphicPackErrorKind::UnsafeMember, "rules.txt is not a regular file"));
    }
    if metadata.len() > CEMU_GRAPHIC_PACK_MAX_RULES_BYTES {
        return Err(error(CemuGraphicPackErrorKind::ResourceLimit, "rules.txt exceeds the inspection limit"));
    }
    let text = String::from_utf8(fs::read(path).map_err(|e| error(
        CemuGraphicPackErrorKind::MalformedRules,
        e.to_string(),
    ))?).map_err(|_| error(CemuGraphicPackErrorKind::MalformedRules, "rules.txt is not UTF-8"))?;
    let mut in_definition = false;
    let mut name = None;
    let mut ids = BTreeSet::new();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_definition = line[1..line.len() - 1].trim().eq_ignore_ascii_case("definition");
            continue;
        }
        if !in_definition || line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim();
        let value = value.trim().trim_matches('"');
        if key.eq_ignore_ascii_case("name") && !value.is_empty() {
            name = Some(value.to_string());
        } else if key.eq_ignore_ascii_case("titleIds") || key.eq_ignore_ascii_case("titleId") {
            for id in value.split([',', ' ', '\t']) {
                if !id.is_empty() {
                    if !valid_title_id(id) {
                        return Err(error(CemuGraphicPackErrorKind::MalformedRules, format!("invalid Wii U title ID in rules.txt: {id}")));
                    }
                    ids.insert(id.to_ascii_uppercase());
                }
            }
        }
    }
    let name = name.ok_or_else(|| error(CemuGraphicPackErrorKind::MalformedRules, "rules.txt has no [Definition] name"))?;
    if !safe_component(&name) {
        return Err(error(CemuGraphicPackErrorKind::UnsafePackName, "graphic-pack name is not one safe folder name"));
    }
    if ids.is_empty() {
        return Err(error(CemuGraphicPackErrorKind::TargetIdentityMissing, "rules.txt declares no base-game titleIds"));
    }
    Ok((name, ids.into_iter().collect()))
}

fn inspect_root(source_path: &Path, root: &Path) -> Result<CemuGraphicPackInspection, CemuGraphicPackError> {
    let rules = root.join("rules.txt");
    if !rules.exists() {
        return Err(error(CemuGraphicPackErrorKind::MissingRules, "graphic pack root must contain rules.txt"));
    }
    let (pack_name, title_ids) = parse_rules(&rules)?;
    let mut files = Vec::new();
    let mut total = 0_u64;
    fn walk(root: &Path, current: &Path, files: &mut Vec<PathBuf>, total: &mut u64) -> Result<(), CemuGraphicPackError> {
        let mut entries: Vec<_> = fs::read_dir(current).map_err(|e| error(CemuGraphicPackErrorKind::SourceNotFound, e.to_string()))?.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|e| error(CemuGraphicPackErrorKind::SourceNotFound, e.to_string()))?;
            if metadata.file_type().is_symlink() || (!metadata.is_file() && !metadata.is_dir()) {
                return Err(error(CemuGraphicPackErrorKind::UnsafeMember, format!("unsafe graphic-pack member: {}", path.display())));
            }
            if metadata.is_dir() {
                walk(root, &path, files, total)?;
            } else {
                let relative = path.strip_prefix(root).map_err(|_| error(CemuGraphicPackErrorKind::UnsafeMember, "member escaped graphic-pack root"))?.to_path_buf();
                if relative.components().any(|c| !matches!(c, Component::Normal(_))) {
                    return Err(error(CemuGraphicPackErrorKind::UnsafeMember, "graphic-pack member contains unsafe path components"));
                }
                if files.len() >= CEMU_GRAPHIC_PACK_MAX_FILES {
                    return Err(error(CemuGraphicPackErrorKind::ResourceLimit, "graphic-pack file-count limit reached"));
                }
                *total = total.saturating_add(metadata.len());
                if *total > CEMU_GRAPHIC_PACK_MAX_TOTAL_BYTES {
                    return Err(error(CemuGraphicPackErrorKind::ResourceLimit, "graphic-pack expanded-size limit reached"));
                }
                files.push(relative);
            }
        }
        Ok(())
    }
    walk(root, root, &mut files, &mut total)?;
    files.sort();
    Ok(CemuGraphicPackInspection { source_path: source_path.to_path_buf(), source_root: root.to_path_buf(), pack_name, title_ids, rules_path: rules, files, total_bytes: total })
}

fn locate_root(source_path: &Path) -> Result<PathBuf, CemuGraphicPackError> {
    if !source_path.is_absolute() {
        return Err(error(CemuGraphicPackErrorKind::SourceNotFound, "graphic-pack source path must be absolute"));
    }
    let metadata = fs::symlink_metadata(source_path).map_err(|e| error(CemuGraphicPackErrorKind::SourceNotFound, e.to_string()))?;
    if metadata.file_type().is_symlink() {
        return Err(error(CemuGraphicPackErrorKind::SourceSymlink, "graphic-pack source is a symlink"));
    }
    if !metadata.is_dir() {
        return Err(error(CemuGraphicPackErrorKind::SourceNotRegular, "graphic-pack source must be a folder or ZIP"));
    }
    if source_path.join("rules.txt").is_file() {
        return Ok(source_path.to_path_buf());
    }
    let mut roots = Vec::new();
    for entry in fs::read_dir(source_path).map_err(|e| error(CemuGraphicPackErrorKind::SourceNotFound, e.to_string()))? {
        let entry = entry.map_err(|e| error(CemuGraphicPackErrorKind::SourceNotFound, e.to_string()))?;
        if entry.file_type().map_err(|e| error(CemuGraphicPackErrorKind::SourceNotFound, e.to_string()))?.is_dir() && entry.path().join("rules.txt").is_file() {
            roots.push(entry.path());
        }
    }
    if roots.len() == 1 { Ok(roots.remove(0)) } else if roots.is_empty() {
        Err(error(CemuGraphicPackErrorKind::MissingRules, "no unambiguous folder containing rules.txt was found"))
    } else {
        Err(error(CemuGraphicPackErrorKind::AmbiguousPackRoot, "package contains multiple graphic-pack roots"))
    }
}

pub fn inspect_cemu_graphic_pack(source_path: &Path) -> Result<CemuGraphicPackInspection, CemuGraphicPackError> {
    let root = locate_root(source_path)?;
    inspect_root(source_path, &root)
}

pub fn inspect_cemu_graphic_pack_zip(source_path: &Path) -> Result<CemuGraphicPackArchiveInspection, CemuGraphicPackError> {
    let metadata = fs::symlink_metadata(source_path).map_err(|e| error(CemuGraphicPackErrorKind::SourceNotFound, e.to_string()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(error(CemuGraphicPackErrorKind::SourceNotRegular, "graphic-pack archive must be a regular file"));
    }
    if metadata.len() > ARCHIVE_MAX_BYTES {
        return Err(error(CemuGraphicPackErrorKind::ResourceLimit, "graphic-pack archive exceeds the compressed-size limit"));
    }
    let staging_root = std::env::temp_dir().join(format!("archivefs-cemu-graphic-pack-{}-{}", std::process::id(), current_stage_id()));
    fs::create_dir(&staging_root).map_err(|e| error(CemuGraphicPackErrorKind::ArchiveRejected, e.to_string()))?;
    let trusted = crate::safe_read::TrustedRoots::from_paths(source_path.parent());
    let limits = crate::dat::archive::limits::ArchiveLimits {
        max_members: ARCHIVE_MAX_MEMBERS,
        max_member_logical_bytes: CEMU_GRAPHIC_PACK_MAX_TOTAL_BYTES,
        max_archive_logical_bytes: ARCHIVE_MAX_TOTAL_BYTES,
        max_solid_decode_bytes: crate::dat::archive::limits::MAX_SOLID_DECODE_BYTES,
        max_dictionary_bytes: crate::dat::archive::limits::MAX_7Z_DICTIONARY_BYTES,
        max_compression_ratio: 1_000,
        max_header_bytes: crate::dat::archive::limits::MAX_7Z_HEADER_BYTES,
        max_zip_central_directory_bytes: crate::dat::archive::limits::MAX_ZIP_CENTRAL_DIRECTORY_BYTES,
        max_aggregate_decoder_memory_bytes: crate::dat::archive::limits::MAX_7Z_AGGREGATE_DECODER_MEMORY_BYTES,
        max_coders_per_folder: crate::dat::archive::limits::MAX_7Z_CODERS_PER_FOLDER,
    };
    let members = crate::dat::archive::zip::extract_zip_members_to(source_path, &trusted, &limits, &AtomicBool::new(false), &staging_root)
        .map_err(|e| { let _ = fs::remove_dir_all(&staging_root); error(CemuGraphicPackErrorKind::ArchiveRejected, format!("ZIP refused: {e:?}")) })?;
    if members.is_empty() { let _ = fs::remove_dir_all(&staging_root); return Err(error(CemuGraphicPackErrorKind::MissingRules, "archive contains no files")); }
    let root = match locate_root(&staging_root) { Ok(root) => root, Err(e) => { let _ = fs::remove_dir_all(&staging_root); return Err(e); } };
    let inspection = match inspect_root(source_path, &root) { Ok(value) => value, Err(e) => { let _ = fs::remove_dir_all(&staging_root); return Err(e); } };
    Ok(CemuGraphicPackArchiveInspection { archive_path: source_path.to_path_buf(), staging_root, inspection })
}

pub fn cemu_graphic_packs_root(profile: &CemuProfile) -> Result<PathBuf, CemuGraphicPackError> {
    if !profile.eligible { return Err(error(CemuGraphicPackErrorKind::ProfileUnavailable, profile.blocker.clone().unwrap_or_else(|| "Cemu profile is not eligible".into()))); }
    let root = match profile.installation_type {
        CemuInstallationType::Native => std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| error(CemuGraphicPackErrorKind::ProfileUnsafe, "HOME is unavailable"))?.join(".local/share/Cemu/graphicPacks"),
        CemuInstallationType::FlatpakUser => std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| error(CemuGraphicPackErrorKind::ProfileUnsafe, "HOME is unavailable"))?.join(".var/app/info.cemu.Cemu/data/Cemu/graphicPacks"),
        CemuInstallationType::Portable | CemuInstallationType::Explicit => profile.configuration_path.join("graphicPacks"),
    };
    if !root.is_absolute() || root.parent().is_none() { return Err(error(CemuGraphicPackErrorKind::ProfileUnsafe, "derived Cemu graphicPacks path is unsafe")); }
    Ok(root)
}

pub fn build_cemu_graphic_pack_plan(
    inspection: &CemuGraphicPackInspection,
    selected_title_id: &str,
    selected_archive: &Path,
    destination_root: &Path,
) -> Result<CemuGraphicPackPlan, CemuGraphicPackError> {
    let selected_title_id = selected_title_id.trim().to_ascii_uppercase();
    if !valid_title_id(&selected_title_id) { return Err(error(CemuGraphicPackErrorKind::TargetIdentityMissing, "a verified Cemu title ID is required")); }
    if !inspection.title_ids.contains(&selected_title_id) { return Err(error(CemuGraphicPackErrorKind::TargetNotDeclared, "the pack does not declare the selected game's title ID")); }
    let source_items = inspection.files.iter().map(|relative| PreviewSourceItem {
        adapter: PreviewAdapter::CemuGraphicPack,
        source_path: inspection.source_root.join(relative),
        expected_source_digest: None,
        destination_relative_paths: vec![PathBuf::from(&inspection.pack_name).join(relative)],
        match_strength: PreviewMatchStrength::VerifiedExact,
    }).collect();
    let report = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::CemuGraphicPack,
        selected_archive: selected_archive.to_path_buf(),
        platform: Some("WiiU".into()),
        identity: PreviewIdentity { kind: PreviewIdentityKind::CemuTitleId, state: PreviewIdentityState::Verified, value: Some(selected_title_id.clone()), archive_path: selected_archive.to_path_buf(), revision: None },
        destination_root: destination_root.to_path_buf(),
        source_items,
    }).map_err(|e| error(CemuGraphicPackErrorKind::PreviewFailed, e.to_string()))?;
    Ok(CemuGraphicPackPlan { inspection: inspection.clone(), target_title_id: selected_title_id, destination_root: destination_root.to_path_buf(), report })
}

pub fn build_cemu_graphic_pack_transaction_plan(plan: &CemuGraphicPackPlan, profile_id: &str) -> Result<SharedTransactionPlan, CemuGraphicPackError> {
    let mut transaction = build_shared_transaction_plan(&plan.report, profile_id, CEMU_GRAPHIC_PACK_SOURCE_MODE, &plan.inspection.source_root)
        .map_err(|e| error(CemuGraphicPackErrorKind::PreviewFailed, e.detail))?;
    super::shared_transaction::require_cemu_graphic_pack_verification(&mut transaction)
        .map_err(|e| error(CemuGraphicPackErrorKind::PreviewFailed, e.detail))?;
    Ok(transaction)
}

pub fn apply_cemu_graphic_pack(plan: &SharedTransactionPlan, options: &SharedApplyOptions) -> CemuGraphicPackApplyResult {
    let apply = execute_shared_apply(plan, options);
    CemuGraphicPackApplyResult { apply, rollback: None }
}

pub fn preview_cemu_graphic_pack_rollback(journal_path: &Path, destination_root: &Path, backup_root: &Path) -> super::shared_transaction::SharedRollbackPreview {
    preview_shared_rollback(journal_path, destination_root, backup_root)
}

pub fn rollback_cemu_graphic_pack(preview: &super::shared_transaction::SharedRollbackPreview, options: &SharedRollbackOptions) -> SharedRollbackResult {
    execute_shared_rollback(preview, options)
}

fn current_stage_id() -> u128 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0) }
