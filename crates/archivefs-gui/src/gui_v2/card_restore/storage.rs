//! Retained files of the Blue card restore: exported save folders (PSU),
//! pre-restore card backups and leftover private snapshot copies.
//!
//! Deletion rules (fail closed):
//! * Only regular files directly inside EmuWiz's own two folders, with a name
//!   this flow creates, are ever candidates.
//! * A file named (by path or by device/inode) in ANY restore record, of any
//!   phase or review state, is referenced and kept: records are recovery
//!   evidence and may still need their backup or PSU for Undo.
//! * If the records cannot all be read, or the list is incomplete, every file
//!   is uncertain and nothing may be retired.
//! * Files changed within the last hour, and the PSU of an open preview, are
//!   kept: they may belong to a restore being prepared.
//! * Retiring re-runs the whole assessment and deletes only files whose
//!   identity and SHA-256 still equal the confirmed preview.

use super::{WorkDirs, sha256_hex};
use archivefs_core::memory_card_inventory::restore_guard::{
    discover_ps2_restore_journals, load_ps2_restore_journal,
};
use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// Files younger than this may belong to a restore being prepared.
pub(in super::super) const MIN_AGE_SECONDS: u64 = 60 * 60;
/// Larger files are never hashed, so never retired (cards are 8 MiB).
const MAX_HASH_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in super::super) enum FileKind {
    ExportedFolder,
    CardBackup,
    LeftoverCopy,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in super::super) enum Status {
    /// Kept: named by a restore record or an open preview.
    Referenced(String),
    /// Kept: EmuWiz cannot prove it is safe to remove.
    Uncertain(String),
    /// May be retired after preview and confirmation.
    Unreferenced,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in super::super) struct StoredFile {
    pub(in super::super) path: PathBuf,
    pub(in super::super) kind: FileKind,
    pub(in super::super) size_bytes: u64,
    pub(in super::super) status: Status,
    /// (device, inode, SHA-256) for unreferenced files; what retiring binds to.
    pub(in super::super) identity: Option<(u64, u64, String)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in super::super) struct StorageReport {
    pub(in super::super) files: Vec<StoredFile>,
    pub(in super::super) problems: Vec<String>,
}

impl StorageReport {
    pub(in super::super) fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.size_bytes).sum()
    }
    pub(in super::super) fn retirable(&self) -> impl Iterator<Item = &StoredFile> {
        self.files
            .iter()
            .filter(|file| file.status == Status::Unreferenced)
    }
}

fn kind_of(name: &str, in_backup_dir: bool) -> FileKind {
    match (in_backup_dir, name) {
        (false, name) if name.ends_with(".psu") => FileKind::ExportedFolder,
        (false, name) if name.ends_with(".snapshot-card") => FileKind::LeftoverCopy,
        (true, name) if name.ends_with(".before-restore.ps2") => FileKind::CardBackup,
        _ => FileKind::Other,
    }
}

/// Paths and (device, inode) pairs named by restore records.
type References = (BTreeSet<PathBuf>, BTreeSet<(u64, u64)>);

/// Everything named by every readable record, or why the reference set
/// cannot be trusted.
fn references(journal_dir: &Path) -> Result<References, String> {
    let discovery = discover_ps2_restore_journals(journal_dir);
    if !discovery.is_complete() {
        return Err("Not every restore record could be listed.".into());
    }
    let mut paths = BTreeSet::new();
    let mut inodes = BTreeSet::new();
    for summary in discovery.records.iter() {
        let journal = load_ps2_restore_journal(&summary.path).map_err(|_| {
            format!(
                "The restore record {} cannot be read, so EmuWiz cannot tell which files it needs.",
                summary.path.display()
            )
        })?;
        for path in [
            Some(journal.psu_path),
            Some(journal.backup_path),
            journal.staged_path,
        ]
        .into_iter()
        .flatten()
        {
            if let Ok(metadata) = fs::metadata(&path) {
                inodes.insert((metadata.dev(), metadata.ino()));
            }
            if let Ok(canonical) = fs::canonicalize(&path) {
                paths.insert(canonical);
            }
            paths.insert(path);
        }
    }
    Ok((paths, inodes))
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Worker: list retained files and decide, fail-closed, which are retirable.
/// `keep` names files an open preview still needs. Read-only.
pub(in super::super) fn assess(
    dirs: &WorkDirs,
    journal_dir: &Path,
    keep: &[PathBuf],
) -> StorageReport {
    assess_at(dirs, journal_dir, keep, now_seconds())
}

pub(in super::super) fn assess_at(
    dirs: &WorkDirs,
    journal_dir: &Path,
    keep: &[PathBuf],
    now_unix_seconds: u64,
) -> StorageReport {
    let mut report = StorageReport::default();
    let references = references(journal_dir);
    if let Err(problem) = &references {
        report.problems.push(problem.clone());
    }
    for (dir, in_backup_dir) in [(&dirs.psu_dir, false), (&dirs.backup_dir, true)] {
        // The folder itself must be EmuWiz's real directory, never a symlink
        // into somewhere else (a user's PSU collection, a memcards folder).
        let dir_ok = match fs::symlink_metadata(dir) {
            Ok(metadata) => metadata.is_dir() && !metadata.file_type().is_symlink(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => false,
        };
        if !dir_ok {
            report.problems.push(format!(
                "{} is not a real folder (for example a link elsewhere), so nothing in it can be removed.",
                dir.display()
            ));
        }
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                report
                    .problems
                    .push(format!("{} could not be listed: {error}", dir.display()));
                continue;
            }
        };
        for entry in entries {
            let Ok(entry) = entry else {
                report
                    .problems
                    .push(format!("{} could not be fully listed.", dir.display()));
                break;
            };
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let kind = kind_of(&name, in_backup_dir);
            let metadata = fs::symlink_metadata(&path);
            let size_bytes = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
            let mut file = StoredFile {
                path: path.clone(),
                kind,
                size_bytes,
                status: Status::Unreferenced,
                identity: None,
            };
            file.status = (|| {
                let Ok(metadata) = &metadata else {
                    return Status::Uncertain("It could not be inspected.".into());
                };
                if !dir_ok {
                    return Status::Uncertain("Its folder is not EmuWiz's real folder.".into());
                }
                if !metadata.is_file() {
                    return Status::Uncertain("It is not a regular file.".into());
                }
                if metadata.len() > MAX_HASH_BYTES {
                    return Status::Uncertain("It is too large for EmuWiz to verify.".into());
                }
                if kind == FileKind::Other {
                    return Status::Uncertain(
                        "EmuWiz did not create a file with this name.".into(),
                    );
                }
                if keep.iter().any(|kept| kept == &path) {
                    return Status::Referenced(
                        "Used by the restore preview that is open now.".into(),
                    );
                }
                let (paths, inodes) = match &references {
                    Ok(references) => references,
                    Err(problem) => return Status::Uncertain(problem.clone()),
                };
                if paths.contains(&path)
                    || fs::canonicalize(&path).is_ok_and(|c| paths.contains(&c))
                    || inodes.contains(&(metadata.dev(), metadata.ino()))
                {
                    return Status::Referenced(
                        "A restore record needs it for recovery or Undo.".into(),
                    );
                }
                let modified = metadata.mtime().max(metadata.ctime()).max(0) as u64;
                if now_unix_seconds.saturating_sub(modified) < MIN_AGE_SECONDS {
                    return Status::Uncertain(
                        "It changed in the last hour and may belong to a restore being prepared."
                            .into(),
                    );
                }
                match fs::read(&path) {
                    Ok(bytes) => {
                        file.identity = Some((metadata.dev(), metadata.ino(), sha256_hex(&bytes)));
                        Status::Unreferenced
                    }
                    Err(_) => Status::Uncertain("It could not be read.".into()),
                }
            })();
            report.files.push(file);
        }
    }
    report.files.sort_by(|a, b| a.path.cmp(&b.path));
    report
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in super::super) struct RetireResult {
    pub(in super::super) removed: Vec<PathBuf>,
    pub(in super::super) freed_bytes: u64,
    /// (file, why it was kept)
    pub(in super::super) kept: Vec<(PathBuf, String)>,
}

/// Worker: retire exactly the confirmed files that are still unreferenced and
/// unchanged. Everything else is kept and explained.
pub(in super::super) fn retire(
    confirmed: &[StoredFile],
    dirs: &WorkDirs,
    journal_dir: &Path,
    keep: &[PathBuf],
) -> RetireResult {
    retire_at(confirmed, dirs, journal_dir, keep, now_seconds())
}

pub(in super::super) fn retire_at(
    confirmed: &[StoredFile],
    dirs: &WorkDirs,
    journal_dir: &Path,
    keep: &[PathBuf],
    now_unix_seconds: u64,
) -> RetireResult {
    let current = assess_at(dirs, journal_dir, keep, now_unix_seconds);
    let mut result = RetireResult::default();
    for wanted in confirmed {
        let now = current.files.iter().find(|file| file.path == wanted.path);
        match now {
            Some(file)
                if file.status == Status::Unreferenced
                    && wanted.status == Status::Unreferenced
                    && file.identity.is_some()
                    && file.identity == wanted.identity =>
            {
                // Re-check the folder and the path's identity right before
                // unlinking, so a swapped name or folder is kept.
                let folder_ok = file.path.parent().is_some_and(|parent| {
                    fs::symlink_metadata(parent)
                        .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
                });
                let same = fs::symlink_metadata(&file.path).is_ok_and(|m| {
                    m.is_file()
                        && file
                            .identity
                            .as_ref()
                            .is_some_and(|(dev, ino, _)| m.dev() == *dev && m.ino() == *ino)
                });
                if !(folder_ok && same) {
                    result
                        .kept
                        .push((file.path.clone(), "It changed just before removal.".into()));
                    continue;
                }
                match fs::remove_file(&file.path) {
                    Ok(()) => {
                        result.freed_bytes += file.size_bytes;
                        result.removed.push(file.path.clone());
                    }
                    Err(error) => result.kept.push((
                        file.path.clone(),
                        format!("It could not be removed: {error}"),
                    )),
                }
            }
            Some(file) => result.kept.push((
                file.path.clone(),
                match &file.status {
                    Status::Referenced(why) | Status::Uncertain(why) => why.clone(),
                    Status::Unreferenced => "It changed since the preview.".into(),
                },
            )),
            None => result
                .kept
                .push((wanted.path.clone(), "It is no longer there.".into())),
        }
    }
    for dir in [&dirs.psu_dir, &dirs.backup_dir] {
        let _ = fs::File::open(dir).and_then(|dir| dir.sync_all());
    }
    result
}
