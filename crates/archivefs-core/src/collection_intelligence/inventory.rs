//! A metadata-only listing of the library folders, with an honest account of
//! whether it is complete.
//!
//! No file is opened: every fact comes from `readdir`, `lstat` and `readlink`.
//! A folder on another filesystem is not entered, except a symbolic link that
//! is a direct child of a library folder and points at a folder. The library
//! scan skips every link, so files moved behind such a link are invisible to
//! it; reconciliation looks there to find them. Deeper linked folders are
//! never entered. A listing that hit an error, a limit or a cancel is
//! marked incomplete, and reconciliation refuses to treat an incomplete
//! listing as proof that a file is gone.
//!
//! Nothing is kept between runs: every run reads every folder and looks up
//! every entry afresh, and nothing is written.
//!
//! EmuWiz's private recovery folders are not game files and are skipped, as
//! the library scan skips them. No folder is entered twice, so a link that
//! points back into the library cannot list its games a second time.

use super::model::{InventorySummary, RootSummary};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Default ceiling on entries held in memory.
pub const DEFAULT_MAX_ENTRIES: u64 = 5_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Link,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub kind: EntryKind,
    pub size: u64,
    /// Whole seconds, as the library records it.
    pub mtime: i64,
    pub inode: u64,
    pub device: u64,
    /// When the inode was created, where the filesystem records it.
    pub born: Option<i64>,
    pub link_target: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InventoryError {
    Cancelled,
    NoFolders,
}

impl std::fmt::Display for InventoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "the listing was cancelled"),
            Self::NoFolders => write!(f, "there is no library folder to list"),
        }
    }
}

pub struct InventoryRequest<'a> {
    pub roots: &'a [PathBuf],
    pub max_entries: u64,
}

#[derive(Debug, Clone, Default)]
pub struct Inventory {
    /// Sorted by path, so results do not depend on directory order.
    pub entries: Vec<Entry>,
    pub summary: InventorySummary,
    by_path: HashMap<PathBuf, usize>,
    /// Linked folders that were looked into. Everything under one of these
    /// paths was reached through a link.
    followed_links: Vec<PathBuf>,
}

impl Inventory {
    pub fn get(&self, path: &Path) -> Option<&Entry> {
        self.by_path.get(path).map(|index| &self.entries[*index])
    }

    /// True when `path` lies under a folder that was listed completely, and
    /// not under a nested filesystem the listing did not enter.
    pub fn covers(&self, path: &Path) -> bool {
        self.summary.roots.iter().any(|root| {
            root.complete
                && path.starts_with(&root.root)
                && !root
                    .other_filesystems_not_entered
                    .iter()
                    .any(|skipped| path.starts_with(skipped))
        })
    }

    /// True when `path` was reached through a linked folder.
    pub fn through_followed_link(&self, path: &Path) -> bool {
        self.followed_links
            .iter()
            .any(|link| path.starts_with(link))
    }

    /// True when `path` is one of the listed library folders themselves.
    pub fn is_root(&self, path: &Path) -> bool {
        self.summary.roots.iter().any(|root| root.root == path)
    }

    #[cfg(test)]
    pub(crate) fn from_entries(mut entries: Vec<Entry>, roots: Vec<RootSummary>) -> Self {
        entries.sort_by(|a, b| {
            a.path
                .as_os_str()
                .as_bytes()
                .cmp(b.path.as_os_str().as_bytes())
        });
        let by_path = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.path.clone(), index))
            .collect();
        let complete = roots.iter().all(|root| root.complete);
        Self {
            summary: InventorySummary {
                entries: entries.len() as u64,
                complete,
                roots,
                ..InventorySummary::default()
            },
            entries,
            by_path,
            followed_links: Vec::new(),
        }
    }
}

fn seconds(time: std::io::Result<SystemTime>) -> Option<i64> {
    let time = time.ok()?;
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => Some(after.as_secs() as i64),
        Err(before) => Some(-(before.duration().as_secs() as i64)),
    }
}

fn kind_of(metadata: &fs::Metadata) -> Option<EntryKind> {
    if metadata.file_type().is_symlink() {
        Some(EntryKind::Link)
    } else if metadata.is_dir() {
        Some(EntryKind::Directory)
    } else if metadata.is_file() {
        Some(EntryKind::File)
    } else {
        None
    }
}

fn entry_from(path: PathBuf, metadata: &fs::Metadata, kind: EntryKind) -> Entry {
    Entry {
        link_target: (kind == EntryKind::Link)
            .then(|| fs::read_link(&path).ok())
            .flatten(),
        path,
        kind,
        size: metadata.len(),
        mtime: metadata.mtime(),
        inode: metadata.ino(),
        device: metadata.dev(),
        born: seconds(metadata.created()),
    }
}

struct Walk<'a> {
    request: &'a InventoryRequest<'a>,
    cancel: &'a AtomicBool,
    entries: Vec<Entry>,
    /// Folders already entered, by device and inode, across all roots.
    visited: HashSet<(u64, u64)>,
    /// Linked folders found directly inside a library folder: (root index,
    /// link path, device of its target). Walked last, so a real folder is
    /// always listed under its real path first.
    linked_folders: Vec<(usize, PathBuf, u64)>,
    read: u64,
}

impl Walk<'_> {
    /// Lists `start` and everything below it on the same device.
    fn folder_tree(
        &mut self,
        root_index: usize,
        summary: &mut RootSummary,
        start: (PathBuf, u64, bool),
        progress: &mut dyn FnMut(u64),
    ) -> Result<(), InventoryError> {
        // (folder, device it may not leave, is the library folder itself)
        let mut stack = vec![start];
        while let Some((dir, boundary, is_root)) = stack.pop() {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(InventoryError::Cancelled);
            }
            if self.entries.len() as u64 > self.request.max_entries {
                summary.errors.push(format!(
                    "stopped at the limit of {} entries",
                    self.request.max_entries
                ));
                summary.complete = false;
                return Ok(());
            }
            let metadata = match fs::metadata(&dir) {
                Ok(metadata) => metadata,
                Err(error) => {
                    summary
                        .errors
                        .push(format!("{}: {}", dir.display(), error.kind()));
                    summary.complete = false;
                    continue;
                }
            };
            if !self.visited.insert((metadata.dev(), metadata.ino())) {
                continue; // reached a second time through a link
            }
            summary.directories += 1;
            self.read += 1;
            let start_index = self.entries.len();
            let reader = match fs::read_dir(&dir) {
                Ok(reader) => reader,
                Err(error) => {
                    summary
                        .errors
                        .push(format!("{}: {}", dir.display(), error.kind()));
                    summary.complete = false;
                    continue;
                }
            };
            for item in reader {
                let item = match item {
                    Ok(item) => item,
                    Err(error) => {
                        summary
                            .errors
                            .push(format!("{}: {}", dir.display(), error.kind()));
                        summary.complete = false;
                        continue;
                    }
                };
                let path = item.path();
                if crate::dat::rename_apply::is_private_recovery_path(&path) {
                    continue;
                }
                let metadata = match fs::symlink_metadata(&path) {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        summary
                            .errors
                            .push(format!("{}: {}", path.display(), error.kind()));
                        summary.complete = false;
                        continue;
                    }
                };
                let Some(kind) = kind_of(&metadata) else {
                    continue;
                };
                self.entries.push(entry_from(path, &metadata, kind));
            }
            for entry in &self.entries[start_index..] {
                match entry.kind {
                    EntryKind::File => summary.files += 1,
                    EntryKind::Link => {
                        summary.links += 1;
                        // A linked folder directly inside the library folder is
                        // looked into, later; deeper ones never are. One that
                        // leads back to the library folder or above it is not.
                        if is_root
                            && let Ok(target) = fs::metadata(&entry.path)
                            && target.is_dir()
                            && let Ok(real) = fs::canonicalize(&entry.path)
                            && !summary.real_root.starts_with(&real)
                            && !crate::dat::rename_apply::is_private_recovery_path(&real)
                        {
                            self.linked_folders.push((
                                root_index,
                                entry.path.clone(),
                                target.dev(),
                            ));
                        }
                    }
                    EntryKind::Directory => {
                        if entry.device == boundary {
                            stack.push((entry.path.clone(), boundary, false));
                        } else {
                            summary
                                .other_filesystems_not_entered
                                .push(entry.path.clone());
                        }
                    }
                }
            }
            progress(self.entries.len() as u64);
        }
        Ok(())
    }
}

/// Lists every library folder. `progress` receives the number of entries so far.
pub fn take_inventory(
    request: &InventoryRequest<'_>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
) -> Result<Inventory, InventoryError> {
    if request.roots.is_empty() {
        return Err(InventoryError::NoFolders);
    }
    let started = Instant::now();
    let mut walk = Walk {
        request,
        cancel,
        entries: Vec::new(),
        visited: HashSet::new(),
        linked_folders: Vec::new(),
        read: 0,
    };
    let mut roots: Vec<RootSummary> = Vec::new();
    for (index, root) in request.roots.iter().enumerate() {
        let mut summary = RootSummary {
            root: root.clone(),
            real_root: fs::canonicalize(root).unwrap_or_else(|_| root.clone()),
            directories: 0,
            files: 0,
            links: 0,
            other_filesystems_not_entered: Vec::new(),
            errors: Vec::new(),
            complete: true,
        };
        match fs::metadata(root) {
            Ok(metadata) if metadata.is_dir() => {
                walk.folder_tree(
                    index,
                    &mut summary,
                    (root.clone(), metadata.dev(), true),
                    progress,
                )?;
            }
            Ok(_) => {
                summary.errors.push("not a folder".into());
                summary.complete = false;
            }
            Err(error) => {
                summary
                    .errors
                    .push(format!("cannot be read: {}", error.kind()));
                summary.complete = false;
            }
        }
        roots.push(summary);
    }
    // Linked folders last: every real folder has been listed by now, so a link
    // that points back into a library cannot take a folder's place.
    let mut followed_links = Vec::new();
    for (index, path, device) in std::mem::take(&mut walk.linked_folders) {
        followed_links.push(path.clone());
        walk.folder_tree(index, &mut roots[index], (path, device, false), progress)?;
    }
    for summary in &mut roots {
        summary.other_filesystems_not_entered.sort();
    }

    let Walk {
        mut entries, read, ..
    } = walk;
    let complete = roots.iter().all(|root| root.complete);
    entries.sort_by(|a, b| {
        a.path
            .as_os_str()
            .as_bytes()
            .cmp(b.path.as_os_str().as_bytes())
    });
    entries.dedup_by(|a, b| a.path == b.path);
    let by_path = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.path.clone(), index))
        .collect();
    Ok(Inventory {
        summary: InventorySummary {
            roots,
            complete,
            entries: entries.len() as u64,
            directories_listed: read,
            seconds: started.elapsed().as_secs(),
        },
        entries,
        by_path,
        followed_links,
    })
}
