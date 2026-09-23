//! Safe application planning for ordinary archive/folder mod packages.
//!
//! This module is the archive/folder companion to the manifest-based
//! [`crate::mod_package`] foundation.  It accepts a caller-selected directory
//! or ZIP containing a real relative file tree, never executes package
//! content, and converts the inspected files into the existing local-mod
//! shared transaction adapter.  The selected game's already-verified identity
//! remains the only target authority; filenames and wrapper names are never
//! used as game identity.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use sha2::{Digest, Sha256};
use tempfile::{TempDir, tempdir};
use zip::ZipArchive;

use crate::game_identity::{GameIdentityReport, IdentityKind, IdentityStatus};
use crate::mod_package::{
    LocalModPackagePlan, ModCompatibilityResult, ModOperationKind, ModPlanBlockerKind,
    ModPlanConflict, ModPlanIssue, ProposedFileState, ProposedModFileOperation, SelectedGameForMod,
    SelectedGameForModSummary, build_local_mod_package_transaction_plan,
};
use crate::patch_manager::SharedTransactionPlan;

pub const MAX_ARCHIVE_MOD_PACKAGE_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_ARCHIVE_MOD_EXPANDED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const MAX_ARCHIVE_MOD_ENTRIES: usize = 4096;
pub const MAX_ARCHIVE_MOD_PATH_DEPTH: usize = 32;
pub const MAX_ARCHIVE_MOD_PATH_BYTES: usize = 4096;

const WRAPPER_DIRECTORIES: &[&str] = &[
    "mod", "package", "contents", "content", "files", "root", "payload", "data",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveModPackageFormat {
    Folder,
    Zip,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ArchiveModPackageFile {
    pub package_path: PathBuf,
    pub destination_relative_path: PathBuf,
    pub destination_path: PathBuf,
    pub size: u64,
    pub sha256: String,
    pub destination_state: ProposedFileState,
    pub action: ArchiveModFileAction,
    pub metadata: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveModFileAction {
    Create,
    Replace,
    AlreadyInstalled,
    Conflict,
    Blocked,
    IgnoredMetadata,
}

#[derive(Debug)]
pub struct ArchiveModPackagePlan {
    pub package_path: PathBuf,
    pub format: ArchiveModPackageFormat,
    pub package_sha256: String,
    pub normalized_root: PathBuf,
    pub wrapper_removed: Option<PathBuf>,
    pub files: Vec<ArchiveModPackageFile>,
    pub inspection: LocalModPackagePlan,
    pub source_fingerprint: String,
    staging: Option<Arc<TempDir>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveModPackageRequest {
    pub selected_game: SelectedGameForMod,
    pub package_path: PathBuf,
}

impl ArchiveModPackagePlan {
    pub fn eligible_for_apply(&self) -> bool {
        self.inspection.eligible_for_later_apply
            && self.files.iter().any(|file| {
                file.action == ArchiveModFileAction::Create
                    || file.action == ArchiveModFileAction::Replace
                    || file.action == ArchiveModFileAction::AlreadyInstalled
            })
    }
}

/// Inspects a folder or ZIP without changing either the package or the game.
/// ZIPs are staged in a private temporary directory so the shared transaction
/// can re-hash the exact payload files before applying them.
pub fn inspect_archive_mod_package(request: ArchiveModPackageRequest) -> ArchiveModPackagePlan {
    let format = if request.package_path.is_dir() {
        ArchiveModPackageFormat::Folder
    } else {
        ArchiveModPackageFormat::Zip
    };
    let package_sha256 = if request.package_path.is_file() {
        hash_file(&request.package_path).unwrap_or_default()
    } else {
        folder_fingerprint(&request.package_path).unwrap_or_default()
    };
    let mut blockers = Vec::new();
    let mut staging = None;
    let normalized_root = match format {
        ArchiveModPackageFormat::Folder => request.package_path.clone(),
        ArchiveModPackageFormat::Zip => match stage_zip(&request.package_path, &mut blockers) {
            Some((root, guard)) => {
                staging = Some(guard);
                root
            }
            None => request.package_path.clone(),
        },
    };

    let mut entries = Vec::new();
    if blockers.is_empty() {
        collect_tree(&normalized_root, &mut entries, &mut blockers);
    }
    let payload_entries: Vec<_> = entries
        .iter()
        .filter(|entry| !is_metadata_path(&entry.relative))
        .cloned()
        .collect();
    if payload_entries.is_empty() && blockers.is_empty() {
        blockers.push(issue(
            ModPlanBlockerKind::PackageLimitExceeded,
            "package contains no applicable mod files after metadata was ignored",
        ));
    }

    let (relative_paths, wrapper_removed) = choose_relative_paths(
        payload_entries
            .iter()
            .map(|entry| entry.relative.clone())
            .collect(),
    );
    let selected_summary = SelectedGameForModSummary {
        game_root: request.selected_game.game_root.clone(),
        archive_path: request.selected_game.identity.archive_path.clone(),
        platform: request.selected_game.identity.platform,
    };
    let mut inspection = LocalModPackagePlan {
        selected_game: selected_summary,
        package_root: normalized_root.clone(),
        manifest_path: normalized_root.join("<archive-package-no-manifest>"),
        package: None,
        compatibility: verified_game_compatibility(&request.selected_game.identity),
        operations: Vec::new(),
        conflicts: Vec::new(),
        warnings: Vec::new(),
        blockers,
        eligible_for_later_apply: false,
    };

    if request.selected_game.game_root.is_relative()
        || request.selected_game.identity.platform == crate::game_identity::IdentityPlatform::Other
    {
        inspection.blockers.push(issue(
            ModPlanBlockerKind::GameIdentityUnknown,
            "selected game must have an absolute root and supported verified platform",
        ));
    }
    if !has_one_verified_identity(&request.selected_game.identity) {
        inspection.blockers.push(issue(
            if has_conflicting_verified_identity(&request.selected_game.identity) {
                ModPlanBlockerKind::GameIdentityConflicting
            } else {
                ModPlanBlockerKind::GameIdentityUnknown
            },
            "package application requires exactly one unambiguous verified game identity",
        ));
    }

    let mut files = Vec::new();
    let mut seen = BTreeSet::new();
    for (entry, relative) in payload_entries.iter().zip(relative_paths) {
        let destination = request.selected_game.game_root.join(&relative);
        let key = relative.to_string_lossy().to_ascii_lowercase();
        if !seen.insert(key) {
            inspection.blockers.push(issue(
                ModPlanBlockerKind::DuplicateDestination,
                format!(
                    "multiple package files map to destination {}",
                    relative.display()
                ),
            ));
            files.push(ArchiveModPackageFile {
                package_path: entry.absolute.clone(),
                destination_relative_path: relative,
                destination_path: destination,
                size: entry.size,
                sha256: entry.sha256.clone(),
                destination_state: ProposedFileState::Unavailable,
                action: ArchiveModFileAction::Blocked,
                metadata: false,
            });
            continue;
        }
        let destination_state = inspect_destination(
            &destination,
            &request.selected_game.game_root,
            &mut inspection,
        );
        let action = match destination_state {
            ProposedFileState::Missing => ArchiveModFileAction::Create,
            ProposedFileState::ExistingRegularFile => match hash_file(&destination) {
                Some(hash) if hash == entry.sha256 => ArchiveModFileAction::AlreadyInstalled,
                Some(_) => ArchiveModFileAction::Replace,
                None => ArchiveModFileAction::Blocked,
            },
            ProposedFileState::ExistingDirectory | ProposedFileState::ExistingSpecialFile => {
                inspection.conflicts.push(ModPlanConflict {
                    kind: crate::mod_package::ModPlanConflictKind::DestinationIsNotRegularFile,
                    destination_path: destination.clone(),
                    detail: "package file collides with a non-regular destination".into(),
                });
                ArchiveModFileAction::Conflict
            }
            ProposedFileState::Unavailable => ArchiveModFileAction::Blocked,
        };
        if matches!(
            action,
            ArchiveModFileAction::Create
                | ArchiveModFileAction::Replace
                | ArchiveModFileAction::AlreadyInstalled
        ) {
            inspection.operations.push(ProposedModFileOperation {
                kind: if action == ArchiveModFileAction::Replace {
                    ModOperationKind::ReplaceFile
                } else {
                    ModOperationKind::CreateFile
                },
                payload_path: Some(entry.relative.clone()),
                source_path: Some(entry.absolute.clone()),
                destination_path: destination.clone(),
                destination_state,
                required_source_sha256: None,
                observed_source_sha256: match action {
                    ArchiveModFileAction::AlreadyInstalled => hash_file(&destination),
                    _ => None,
                },
                expected_result_sha256: Some(entry.sha256.clone()),
                observed_payload_sha256: Some(entry.sha256.clone()),
                patch_format: None,
            });
        }
        files.push(ArchiveModPackageFile {
            package_path: entry.absolute.clone(),
            destination_relative_path: relative,
            destination_path: destination,
            size: entry.size,
            sha256: entry.sha256.clone(),
            destination_state,
            action,
            metadata: false,
        });
    }
    inspection
        .operations
        .sort_by(|left, right| left.destination_path.cmp(&right.destination_path));
    inspection
        .conflicts
        .sort_by(|left, right| left.destination_path.cmp(&right.destination_path));
    inspection.blockers.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.detail.cmp(&right.detail))
    });
    if inspection.blockers.is_empty()
        && inspection.conflicts.is_empty()
        && !inspection.operations.is_empty()
    {
        inspection.compatibility.state = crate::mod_package::ModCompatibilityState::Compatible;
        inspection.eligible_for_later_apply = true;
    } else {
        inspection.compatibility.state = crate::mod_package::ModCompatibilityState::Incompatible;
    }

    ArchiveModPackagePlan {
        package_path: request.package_path,
        format,
        package_sha256: package_sha256.clone(),
        normalized_root,
        wrapper_removed,
        files,
        source_fingerprint: package_sha256,
        inspection,
        staging,
    }
}

/// Revalidates the selected package source and then builds the existing
/// shared transaction plan. No writes occur here.
pub fn build_archive_mod_package_transaction_plan(
    plan: &ArchiveModPackagePlan,
) -> Result<SharedTransactionPlan, crate::patch_manager::SharedApplyFailure> {
    // Keep the ZIP staging guard borrowed through plan construction; the
    // returned transaction intentionally relies on the caller retaining the
    // inspected plan until apply has revalidated and consumed its sources.
    let _staging_guard = &plan.staging;
    let current = if plan.package_path.is_file() {
        hash_file(&plan.package_path)
    } else {
        folder_fingerprint(&plan.package_path)
    };
    if current.as_deref() != Some(plan.source_fingerprint.as_str()) {
        return Err(crate::patch_manager::SharedApplyFailure {
            kind: crate::patch_manager::SharedApplyFailureKind::SourceChanged,
            path: Some(crate::patch_manager::SharedTransactionPath::from_path(
                &plan.package_path,
            )),
            detail: "the archive/folder package changed after inspection".into(),
        });
    }
    build_local_mod_package_transaction_plan(&plan.inspection)
}

#[derive(Clone, Debug)]
struct TreeEntry {
    relative: PathBuf,
    absolute: PathBuf,
    size: u64,
    sha256: String,
}

fn issue(kind: ModPlanBlockerKind, detail: impl Into<String>) -> ModPlanIssue {
    ModPlanIssue {
        kind,
        detail: detail.into(),
    }
}

fn stage_zip(path: &Path, blockers: &mut Vec<ModPlanIssue>) -> Option<(PathBuf, Arc<TempDir>)> {
    let size = match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => meta.len(),
        _ => {
            blockers.push(issue(
                ModPlanBlockerKind::PackageMissing,
                "archive is missing or not a regular file",
            ));
            return None;
        }
    };
    if size > MAX_ARCHIVE_MOD_PACKAGE_BYTES {
        blockers.push(issue(
            ModPlanBlockerKind::PackageLimitExceeded,
            "archive exceeds the compressed-size bound",
        ));
        return None;
    }
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            blockers.push(issue(
                ModPlanBlockerKind::PackagePathUnsafe,
                format!("cannot open archive: {error}"),
            ));
            return None;
        }
    };
    let mut archive = match ZipArchive::new(file) {
        Ok(value) => value,
        Err(error) => {
            blockers.push(issue(
                ModPlanBlockerKind::ManifestMalformed,
                format!("ZIP archive is malformed: {error}"),
            ));
            return None;
        }
    };
    if archive.len() > MAX_ARCHIVE_MOD_ENTRIES {
        blockers.push(issue(
            ModPlanBlockerKind::PackageLimitExceeded,
            "archive member count exceeds the bound",
        ));
        return None;
    }
    let guard = match tempdir() {
        Ok(dir) => Arc::new(dir),
        Err(error) => {
            blockers.push(issue(
                ModPlanBlockerKind::PackagePathUnsafe,
                format!("cannot create staging directory: {error}"),
            ));
            return None;
        }
    };
    let mut expanded = 0_u64;
    let mut seen_members = BTreeSet::new();
    for index in 0..archive.len() {
        let mut member = match archive.by_index(index) {
            Ok(member) => member,
            Err(error) => {
                blockers.push(issue(
                    ModPlanBlockerKind::ManifestMalformed,
                    format!("cannot inspect ZIP member: {error}"),
                ));
                return None;
            }
        };
        let Some(relative) = safe_member_path(member.name()) else {
            blockers.push(issue(
                ModPlanBlockerKind::PackagePathUnsafe,
                format!("unsafe archive member path: {}", member.name()),
            ));
            return None;
        };
        if relative.components().count() > MAX_ARCHIVE_MOD_PATH_DEPTH {
            blockers.push(issue(
                ModPlanBlockerKind::PackageLimitExceeded,
                "archive member path depth exceeds the bound",
            ));
            return None;
        }
        if !seen_members.insert(relative.to_string_lossy().to_ascii_lowercase()) {
            blockers.push(issue(
                ModPlanBlockerKind::DuplicateDestination,
                "archive contains duplicate destination members",
            ));
            return None;
        }
        if member.is_dir() {
            continue;
        }
        if let Some(mode) = member.unix_mode() {
            let kind = mode & 0o170000;
            if kind != 0 && kind != 0o100000 {
                blockers.push(issue(
                    ModPlanBlockerKind::UnsafePackageEntry,
                    "ZIP contains a symlink, hardlink, or special-file member",
                ));
                return None;
            }
            if mode & 0o111 != 0 {
                blockers.push(issue(
                    ModPlanBlockerKind::UnsafePackageEntry,
                    "ZIP contains an executable member",
                ));
                return None;
            }
        }
        expanded = expanded.saturating_add(member.size());
        if expanded > MAX_ARCHIVE_MOD_EXPANDED_BYTES {
            blockers.push(issue(
                ModPlanBlockerKind::PackageLimitExceeded,
                "expanded archive size exceeds the bound",
            ));
            return None;
        }
        let destination = guard.path().join(&relative);
        if let Some(parent) = destination.parent()
            && fs::create_dir_all(parent).is_err()
        {
            blockers.push(issue(
                ModPlanBlockerKind::PackagePathUnsafe,
                "cannot create safe archive staging path",
            ));
            return None;
        }
        let mut bytes = Vec::new();
        if member.read_to_end(&mut bytes).is_err() || bytes.len() as u64 != member.size() {
            blockers.push(issue(
                ModPlanBlockerKind::ManifestMalformed,
                "ZIP member could not be read completely",
            ));
            return None;
        }
        if fs::write(destination, bytes).is_err() {
            blockers.push(issue(
                ModPlanBlockerKind::PackagePathUnsafe,
                "cannot write archive staging file",
            ));
            return None;
        }
    }
    Some((guard.path().to_path_buf(), guard))
}

fn collect_tree(root: &Path, entries: &mut Vec<TreeEntry>, blockers: &mut Vec<ModPlanIssue>) {
    let root_metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) => {
            blockers.push(issue(
                ModPlanBlockerKind::PackageMissing,
                format!("package folder is unavailable: {error}"),
            ));
            return;
        }
    };
    if root_metadata.file_type().is_symlink() {
        blockers.push(issue(
            ModPlanBlockerKind::UnsafeSymlink,
            "package root must not be a symlink",
        ));
        return;
    }
    if !root_metadata.is_dir() {
        blockers.push(issue(
            ModPlanBlockerKind::PackageNotDirectory,
            "package folder is not a directory",
        ));
        return;
    }
    let mut pending = vec![(root.to_path_buf(), 0_usize)];
    let mut total = 0_u64;
    while let Some((directory, depth)) = pending.pop() {
        let read_dir = match fs::read_dir(&directory) {
            Ok(value) => value,
            Err(error) => {
                blockers.push(issue(
                    ModPlanBlockerKind::PackagePathUnsafe,
                    format!("cannot read package folder: {error}"),
                ));
                return;
            }
        };
        for item in read_dir {
            let item = match item {
                Ok(value) => value,
                Err(error) => {
                    blockers.push(issue(
                        ModPlanBlockerKind::PackagePathUnsafe,
                        format!("cannot enumerate package folder: {error}"),
                    ));
                    return;
                }
            };
            let path = item.path();
            let relative = match path.strip_prefix(root) {
                Ok(value) => value.to_path_buf(),
                Err(_) => {
                    blockers.push(issue(
                        ModPlanBlockerKind::PackagePathUnsafe,
                        "package entry escaped its root",
                    ));
                    return;
                }
            };
            if relative.to_string_lossy().len() > MAX_ARCHIVE_MOD_PATH_BYTES
                || relative.components().count() > MAX_ARCHIVE_MOD_PATH_DEPTH
            {
                blockers.push(issue(
                    ModPlanBlockerKind::PackageLimitExceeded,
                    "package path exceeds the depth or length bound",
                ));
                return;
            }
            let metadata = match fs::symlink_metadata(&path) {
                Ok(value) => value,
                Err(error) => {
                    blockers.push(issue(
                        ModPlanBlockerKind::PackagePathUnsafe,
                        format!("cannot inspect package entry: {error}"),
                    ));
                    return;
                }
            };
            if metadata.file_type().is_symlink() {
                blockers.push(issue(
                    ModPlanBlockerKind::UnsafeSymlink,
                    "package contains a symlink or hardlink",
                ));
                return;
            }
            if metadata.is_dir() {
                if depth + 1 > MAX_ARCHIVE_MOD_PATH_DEPTH {
                    blockers.push(issue(
                        ModPlanBlockerKind::PackageLimitExceeded,
                        "package directory depth exceeds the bound",
                    ));
                    return;
                }
                pending.push((path, depth + 1));
                continue;
            }
            if !metadata.is_file() {
                blockers.push(issue(
                    ModPlanBlockerKind::UnsafePackageEntry,
                    "package contains a special file",
                ));
                return;
            }
            if metadata.nlink_if_unix() > 1 {
                blockers.push(issue(
                    ModPlanBlockerKind::UnsafePackageEntry,
                    "package contains a hardlink",
                ));
                return;
            }
            if is_executable_path(&path) {
                blockers.push(issue(
                    ModPlanBlockerKind::UnsafePackageEntry,
                    format!(
                        "executable or script is not an ordinary mod file: {}",
                        relative.display()
                    ),
                ));
                return;
            }
            total = total.saturating_add(metadata.len());
            if total > MAX_ARCHIVE_MOD_EXPANDED_BYTES {
                blockers.push(issue(
                    ModPlanBlockerKind::PackageLimitExceeded,
                    "expanded package size exceeds the bound",
                ));
                return;
            }
            let Some(sha256) = hash_file(&path) else {
                blockers.push(issue(
                    ModPlanBlockerKind::PackagePathUnsafe,
                    format!("cannot hash package file: {}", relative.display()),
                ));
                return;
            };
            entries.push(TreeEntry {
                relative,
                absolute: path,
                size: metadata.len(),
                sha256,
            });
            if entries.len() > MAX_ARCHIVE_MOD_ENTRIES {
                blockers.push(issue(
                    ModPlanBlockerKind::PackageLimitExceeded,
                    "package member count exceeds the bound",
                ));
                return;
            }
        }
    }
}

fn choose_relative_paths(paths: Vec<PathBuf>) -> (Vec<PathBuf>, Option<PathBuf>) {
    let Some(first) = paths.first() else {
        return (paths, None);
    };
    let first_component = first
        .components()
        .next()
        .and_then(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        });
    let Some(first_component) = first_component else {
        return (paths, None);
    };
    if !WRAPPER_DIRECTORIES.contains(&first_component.as_str())
        || !paths.iter().all(|path| {
            path.components().count() > 1
                && path
                    .components()
                    .next()
                    .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
                    == Some(first_component.clone())
        })
    {
        return (paths, None);
    }
    let wrapper = PathBuf::from(&first_component);
    let stripped = paths
        .into_iter()
        .map(|path| path.components().skip(1).collect())
        .collect();
    (stripped, Some(wrapper))
}

fn is_metadata_path(path: &Path) -> bool {
    let lower = path
        .file_name()
        .map(|v| v.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    lower == "emuwiz.mod.json"
        || lower == "readme"
        || lower == "readme.txt"
        || lower == "readme.md"
        || lower == "install.txt"
        || lower == "changelog"
        || lower == "license"
        || lower == "license.txt"
        || lower == "license.md"
        || lower.ends_with(".nfo")
}

fn is_executable_path(path: &Path) -> bool {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    if [
        ".exe",
        ".dll",
        ".so",
        ".sh",
        ".bat",
        ".cmd",
        ".ps1",
        ".py",
        ".appimage",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
    {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(true)
    }
    #[cfg(not(unix))]
    false
}

fn safe_member_path(name: &str) -> Option<PathBuf> {
    let normalized = name.replace('\\', "/");
    if normalized.is_empty()
        || normalized.len() > MAX_ARCHIVE_MOD_PATH_BYTES
        || normalized.starts_with('/')
        || normalized.contains(':')
    {
        return None;
    }
    let mut path = PathBuf::new();
    for component in Path::new(&normalized).components() {
        match component {
            Component::Normal(value) => path.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (path.components().count() <= MAX_ARCHIVE_MOD_PATH_DEPTH && !path.as_os_str().is_empty())
        .then_some(path)
}

fn inspect_destination(
    path: &Path,
    root: &Path,
    plan: &mut LocalModPackagePlan,
) -> ProposedFileState {
    if !path.starts_with(root) {
        plan.blockers.push(issue(
            ModPlanBlockerKind::DestinationEscapesGameRoot,
            "destination escapes selected game root",
        ));
        return ProposedFileState::Unavailable;
    }
    let relative = path.strip_prefix(root).unwrap_or(path);
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                plan.blockers.push(issue(
                    ModPlanBlockerKind::UnsafeSymlink,
                    format!("destination contains symlink {}", current.display()),
                ));
                return ProposedFileState::Unavailable;
            }
            Ok(meta) if current == path && meta.is_file() => {
                return ProposedFileState::ExistingRegularFile;
            }
            Ok(meta) if current == path && meta.is_dir() => {
                return ProposedFileState::ExistingDirectory;
            }
            Ok(_) if current == path => return ProposedFileState::ExistingSpecialFile,
            Ok(meta) if !meta.is_dir() => return ProposedFileState::Unavailable,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return ProposedFileState::Missing;
            }
            Err(_) => return ProposedFileState::Unavailable,
        }
    }
    ProposedFileState::ExistingDirectory
}

fn verified_game_compatibility(identity: &GameIdentityReport) -> ModCompatibilityResult {
    ModCompatibilityResult {
        state: if has_one_verified_identity(identity) {
            crate::mod_package::ModCompatibilityState::Compatible
        } else {
            crate::mod_package::ModCompatibilityState::Unknown
        },
        selected_platform: identity.platform,
        package_platform: None,
        matching_identity: verified_game_requirement(identity),
        region_matches: None,
        revision_matches: None,
    }
}

fn has_one_verified_identity(identity: &GameIdentityReport) -> bool {
    verified_game_requirement(identity).is_some()
}

fn has_conflicting_verified_identity(identity: &GameIdentityReport) -> bool {
    [
        IdentityKind::Ps3TitleId,
        IdentityKind::Ps2Serial,
        IdentityKind::Pcsx2ExecutableCrc,
        IdentityKind::DolphinGameId,
        IdentityKind::PspDiscId,
        IdentityKind::XexTitleId,
        IdentityKind::XexMediaId,
        IdentityKind::LooseRomSha256,
    ]
    .iter()
    .any(|kind| verified_values(identity, *kind).len() > 1)
}

fn verified_game_requirement(
    identity: &GameIdentityReport,
) -> Option<crate::mod_package::ModIdentityRequirement> {
    if !identity.complete || has_conflicting_verified_identity(identity) {
        return None;
    }
    [
        IdentityKind::Ps3TitleId,
        IdentityKind::Ps2Serial,
        IdentityKind::Pcsx2ExecutableCrc,
        IdentityKind::DolphinGameId,
        IdentityKind::PspDiscId,
        IdentityKind::XexTitleId,
        IdentityKind::XexMediaId,
        IdentityKind::LooseRomSha256,
    ]
    .iter()
    .find_map(|kind| {
        let values = verified_values(identity, *kind);
        (values.len() == 1).then(|| mod_identity_requirement(*kind, Some(values[0].as_str())))
    })
    .flatten()
}

fn verified_values(identity: &GameIdentityReport, kind: IdentityKind) -> Vec<String> {
    identity
        .evidence
        .iter()
        .filter(|e| e.kind == kind && e.status == IdentityStatus::Verified)
        .filter_map(|e| e.value.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn mod_identity_requirement(
    kind: IdentityKind,
    value: Option<&str>,
) -> Option<crate::mod_package::ModIdentityRequirement> {
    let value = value?.to_owned();
    let kind = match kind {
        IdentityKind::Ps3TitleId => crate::mod_package::ModIdentityKind::Ps3TitleId,
        IdentityKind::Ps2Serial => crate::mod_package::ModIdentityKind::Ps2Serial,
        IdentityKind::Pcsx2ExecutableCrc => crate::mod_package::ModIdentityKind::Pcsx2ExecutableCrc,
        IdentityKind::DolphinGameId => crate::mod_package::ModIdentityKind::DolphinGameId,
        IdentityKind::LooseRomSha256 => crate::mod_package::ModIdentityKind::LooseRomSha256,
        IdentityKind::XexTitleId => crate::mod_package::ModIdentityKind::XexTitleId,
        IdentityKind::XexMediaId => crate::mod_package::ModIdentityKind::XexMediaId,
        IdentityKind::PspDiscId => crate::mod_package::ModIdentityKind::PspDiscId,
        _ => return None,
    };
    Some(crate::mod_package::ModIdentityRequirement { kind, value })
}

fn hash_file(path: &Path) -> Option<String> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file()
        || meta.file_type().is_symlink()
        || meta.len() > MAX_ARCHIVE_MOD_PACKAGE_BYTES
    {
        return None;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = options.open(path).ok()?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let count = file.read(&mut buffer).ok()?;
        if count == 0 {
            break;
        }
        total = total.checked_add(count as u64)?;
        if total > MAX_ARCHIVE_MOD_PACKAGE_BYTES {
            return None;
        }
        digest.update(&buffer[..count]);
    }
    Some(
        digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

fn folder_fingerprint(root: &Path) -> Option<String> {
    let mut entries = Vec::new();
    let mut blockers = Vec::new();
    collect_tree(root, &mut entries, &mut blockers);
    if !blockers.is_empty() {
        return None;
    }
    entries.sort_by(|left, right| left.relative.cmp(&right.relative));
    let mut digest = Sha256::new();
    for entry in entries {
        digest.update(entry.relative.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update(entry.sha256.as_bytes());
        digest.update([0]);
    }
    Some(
        digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

#[cfg(unix)]
trait NlinkIfUnix {
    fn nlink_if_unix(&self) -> u64;
}
#[cfg(unix)]
impl NlinkIfUnix for fs::Metadata {
    fn nlink_if_unix(&self) -> u64 {
        use std::os::unix::fs::MetadataExt;
        self.nlink()
    }
}
#[cfg(not(unix))]
trait NlinkIfUnix {
    fn nlink_if_unix(&self) -> u64;
}
#[cfg(not(unix))]
impl NlinkIfUnix for fs::Metadata {
    fn nlink_if_unix(&self) -> u64 {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_identity::{
        GameIdentityReport, IdentityConfidence, IdentityEvidence, IdentityImageFormat,
        IdentityPlatform, IdentityProvenance,
    };
    use crate::patch_manager::{
        SharedApplyConfirmation, SharedApplyOptions, SharedApplyStatus, SharedRollbackConfirmation,
        SharedRollbackOptions, execute_shared_apply, execute_shared_rollback,
        preview_shared_rollback,
    };
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    fn selected(root: &Path) -> SelectedGameForMod {
        fs::create_dir_all(root).unwrap();
        let game = root.join("game.bin");
        fs::write(&game, b"game").unwrap();
        SelectedGameForMod {
            game_root: root.to_path_buf(),
            identity: GameIdentityReport {
                archive_path: game,
                platform: IdentityPlatform::Snes,
                format: IdentityImageFormat::LooseCartridgeRom,
                evidence: vec![IdentityEvidence {
                    kind: IdentityKind::LooseRomSha256,
                    status: IdentityStatus::Verified,
                    value: Some("verified-game".into()),
                    confidence: IdentityConfidence::ExactBytes,
                    provenance: IdentityProvenance {
                        archive_path: root.join("game.bin"),
                        member_path: None,
                        member_index: None,
                        method: "test".into(),
                    },
                    diagnostic: String::new(),
                }],
                warnings: Vec::new(),
                bytes_read: 4,
                archive_members_inspected: 0,
                metadata_paths_inspected: 0,
                nested_container_depth: 0,
                complete: true,
            },
        }
    }

    #[test]
    fn folder_wrapper_and_nested_tree_are_planned_without_flattening() {
        let temp = TempDir::new().unwrap();
        let package = temp.path().join("package");
        fs::create_dir_all(package.join("mod/assets/textures")).unwrap();
        fs::write(package.join("mod/README.md"), b"docs").unwrap();
        fs::write(package.join("mod/assets/textures/a.bin"), b"texture").unwrap();
        let plan = inspect_archive_mod_package(ArchiveModPackageRequest {
            selected_game: selected(&temp.path().join("game")),
            package_path: package,
        });
        assert_eq!(
            plan.wrapper_removed,
            Some(PathBuf::from("mod")),
            "files={:?} blockers={:?}",
            plan.files,
            plan.inspection.blockers
        );
        assert!(
            plan.eligible_for_apply(),
            "blockers={:?} conflicts={:?} files={:?}",
            plan.inspection.blockers,
            plan.inspection.conflicts,
            plan.files
        );
        assert_eq!(
            plan.files[0].destination_relative_path,
            PathBuf::from("assets/textures/a.bin")
        );
    }

    #[test]
    fn zip_package_is_staged_and_metadata_is_not_applied() {
        let temp = TempDir::new().unwrap();
        let zip_path = temp.path().join("mod.zip");
        let file = fs::File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file(
                "contents/mod/file.bin",
                zip::write::FileOptions::<()>::default(),
            )
            .unwrap();
        writer.write_all(b"payload").unwrap();
        writer
            .start_file(
                "contents/README.txt",
                zip::write::FileOptions::<()>::default(),
            )
            .unwrap();
        writer.write_all(b"read me").unwrap();
        writer.finish().unwrap();
        let plan = inspect_archive_mod_package(ArchiveModPackageRequest {
            selected_game: selected(&temp.path().join("game")),
            package_path: zip_path,
        });
        assert_eq!(plan.format, ArchiveModPackageFormat::Zip);
        assert!(plan.eligible_for_apply());
        assert_eq!(plan.files.len(), 1);
        assert!(plan.normalized_root.exists());
    }

    #[test]
    fn unsafe_zip_paths_and_executables_fail_closed() {
        let temp = TempDir::new().unwrap();
        let zip_path = temp.path().join("unsafe.zip");
        let file = fs::File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("../escape.bin", zip::write::FileOptions::<()>::default())
            .unwrap();
        writer.write_all(b"x").unwrap();
        writer.finish().unwrap();
        let plan = inspect_archive_mod_package(ArchiveModPackageRequest {
            selected_game: selected(&temp.path().join("game")),
            package_path: zip_path,
        });
        assert!(!plan.eligible_for_apply());
        assert!(!plan.inspection.blockers.is_empty());
    }

    #[test]
    fn duplicate_zip_members_and_missing_identity_fail_closed() {
        let temp = TempDir::new().unwrap();
        let zip_path = temp.path().join("duplicate.zip");
        let file = fs::File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("mod/FILE.bin", zip::write::FileOptions::<()>::default())
            .unwrap();
        writer.write_all(b"one").unwrap();
        writer
            .start_file("mod/file.bin", zip::write::FileOptions::<()>::default())
            .unwrap();
        writer.write_all(b"two").unwrap();
        writer.finish().unwrap();
        let plan = inspect_archive_mod_package(ArchiveModPackageRequest {
            selected_game: selected(&temp.path().join("game")),
            package_path: zip_path,
        });
        assert!(
            plan.inspection
                .blockers
                .iter()
                .any(|blocker| blocker.kind == ModPlanBlockerKind::DuplicateDestination)
        );

        let package = temp.path().join("folder");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("file.bin"), b"one").unwrap();
        let mut selected_game = selected(&temp.path().join("game2"));
        selected_game.identity.complete = false;
        let plan = inspect_archive_mod_package(ArchiveModPackageRequest {
            selected_game,
            package_path: package,
        });
        assert!(
            plan.inspection
                .blockers
                .iter()
                .any(|blocker| blocker.kind == ModPlanBlockerKind::GameIdentityUnknown)
        );
    }

    #[cfg(unix)]
    #[test]
    fn folder_hardlinks_and_package_root_symlinks_fail_closed() {
        use std::os::unix::fs::symlink;
        let temp = TempDir::new().unwrap();
        let package = temp.path().join("folder");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("one.bin"), b"same").unwrap();
        fs::hard_link(package.join("one.bin"), package.join("two.bin")).unwrap();
        let plan = inspect_archive_mod_package(ArchiveModPackageRequest {
            selected_game: selected(&temp.path().join("game")),
            package_path: package,
        });
        assert!(
            plan.inspection
                .blockers
                .iter()
                .any(|blocker| blocker.kind == ModPlanBlockerKind::UnsafePackageEntry)
        );

        let real = temp.path().join("real");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("file.bin"), b"same").unwrap();
        let linked = temp.path().join("linked");
        symlink(&real, &linked).unwrap();
        let plan = inspect_archive_mod_package(ArchiveModPackageRequest {
            selected_game: selected(&temp.path().join("game2")),
            package_path: linked,
        });
        assert!(
            plan.inspection
                .blockers
                .iter()
                .any(|blocker| blocker.kind == ModPlanBlockerKind::UnsafeSymlink)
        );
    }

    #[test]
    fn package_transaction_applies_and_rolls_back_without_touching_unrelated_files() {
        let temp = TempDir::new().unwrap();
        let game_root = temp.path().join("game");
        let package = temp.path().join("package");
        fs::create_dir_all(&game_root).unwrap();
        fs::create_dir_all(&package).unwrap();
        let selected_game = selected(&game_root);
        fs::write(game_root.join("unrelated.txt"), b"keep").unwrap();
        fs::write(package.join("new.bin"), b"modded").unwrap();
        let inspected = inspect_archive_mod_package(ArchiveModPackageRequest {
            selected_game,
            package_path: package,
        });
        let plan = build_archive_mod_package_transaction_plan(&inspected).unwrap();
        let history_root = temp.path().join("history");
        let backup_root = temp.path().join("backups");
        let applied = execute_shared_apply(
            &plan,
            &SharedApplyOptions {
                dry_run: false,
                confirmation: Some(SharedApplyConfirmation {
                    plan_id: plan.plan_id.clone(),
                    general_approved: true,
                    replacement_approved: false,
                }),
                operation_id: "archive-package-test".into(),
                timestamp_unix_seconds: 1_700_000_000,
                current_context: plan.context.clone(),
                history_root: history_root.clone(),
                backup_root: backup_root.clone(),
            },
        );
        assert_eq!(applied.journal.status, SharedApplyStatus::Success);
        assert_eq!(fs::read(game_root.join("new.bin")).unwrap(), b"modded");
        assert_eq!(fs::read(game_root.join("unrelated.txt")).unwrap(), b"keep");
        let journal = applied.journal_path.unwrap();
        let preview = preview_shared_rollback(&journal, &game_root, &backup_root);
        assert!(preview.available);
        let rolled_back = execute_shared_rollback(
            &preview,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: preview.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "archive-package-rollback".into(),
                timestamp_unix_seconds: 1_700_000_001,
                history_root,
                backup_root,
            },
        );
        assert_eq!(rolled_back.status, SharedApplyStatus::Success);
        assert!(!game_root.join("new.bin").exists());
        assert_eq!(fs::read(game_root.join("unrelated.txt")).unwrap(), b"keep");
    }

    #[test]
    fn destination_preview_distinguishes_identical_replace_and_directory_conflict() {
        let temp = TempDir::new().unwrap();
        let package = temp.path().join("package");
        let game_root = temp.path().join("game");
        fs::create_dir_all(&package).unwrap();
        fs::create_dir_all(&game_root).unwrap();
        fs::write(package.join("same.bin"), b"same").unwrap();
        fs::write(package.join("replace.bin"), b"new").unwrap();
        fs::write(package.join("conflict.bin"), b"blocked").unwrap();
        let selected_game = selected(&game_root);
        fs::write(game_root.join("same.bin"), b"same").unwrap();
        fs::write(game_root.join("replace.bin"), b"old").unwrap();
        fs::create_dir(game_root.join("conflict.bin")).unwrap();
        let plan = inspect_archive_mod_package(ArchiveModPackageRequest {
            selected_game,
            package_path: package,
        });
        assert_eq!(
            plan.files
                .iter()
                .find(|file| file.destination_relative_path == Path::new("same.bin"))
                .unwrap()
                .action,
            ArchiveModFileAction::AlreadyInstalled
        );
        assert_eq!(
            plan.files
                .iter()
                .find(|file| file.destination_relative_path == Path::new("replace.bin"))
                .unwrap()
                .action,
            ArchiveModFileAction::Replace
        );
        assert_eq!(
            plan.files
                .iter()
                .find(|file| file.destination_relative_path == Path::new("conflict.bin"))
                .unwrap()
                .action,
            ArchiveModFileAction::Conflict
        );
        assert!(!plan.eligible_for_apply());
    }

    #[test]
    fn changed_folder_is_rejected_before_transaction_plan() {
        let temp = TempDir::new().unwrap();
        let package = temp.path().join("mod");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("file.bin"), b"one").unwrap();
        let plan = inspect_archive_mod_package(ArchiveModPackageRequest {
            selected_game: selected(&temp.path().join("game")),
            package_path: package.clone(),
        });
        fs::write(package.join("file.bin"), b"two").unwrap();
        let result = build_archive_mod_package_transaction_plan(&plan);
        assert!(
            matches!(result, Err(error) if error.kind == crate::patch_manager::SharedApplyFailureKind::SourceChanged)
        );
    }
}
