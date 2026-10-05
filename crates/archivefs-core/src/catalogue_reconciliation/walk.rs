//! Read-only enumeration of the regular files beneath configured library roots.
//!
//! Visits every regular file whatever its extension (the scan deliberately
//! skips some; this must not), never follows a symlink, does not descend into a
//! nested filesystem or a nested configured root, is cancellable between
//! directories, and returns results in a deterministic order.
use super::FileFacts;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkRoot {
    pub source_id: i64,
    pub path: PathBuf,
    /// Nested configured roots, owned by their own source and not walked here.
    pub excluded: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalkLimits {
    pub max_depth: usize,
    pub max_files: usize,
}

impl Default for WalkLimits {
    fn default() -> Self {
        Self {
            max_depth: 128,
            max_files: 5_000_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootWalkState {
    /// Every directory was read and no boundary was left unexplored.
    Complete,
    /// Some part could not be covered (see the lists on [`RootWalk`]).
    Partial,
    /// The root itself could not be read.
    Unavailable,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootWalk {
    pub source_id: i64,
    pub state: RootWalkState,
    /// Directories on another filesystem, deliberately not entered.
    pub nested_boundaries: Vec<PathBuf>,
    /// Directories or files that could not be read.
    pub inaccessible: Vec<PathBuf>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WalkReport {
    /// Sorted by path.
    pub files: Vec<FileFacts>,
    pub roots: Vec<RootWalk>,
    pub cancelled: bool,
}

/// Mount identity of a directory. Injectable so boundary behaviour can be
/// tested without privileges to mount anything.
pub type MountOf<'a> = &'a dyn Fn(&Path) -> io::Result<u64>;

pub fn default_mount_of(path: &Path) -> io::Result<u64> {
    crate::catalogue_health::directory_mount(path).map(|mount| mount.mount_id)
}

pub fn walk_roots(
    roots: &[WalkRoot],
    limits: WalkLimits,
    cancel: &AtomicBool,
    mount_of: MountOf<'_>,
) -> WalkReport {
    let mut report = WalkReport::default();
    for root in roots {
        let walk = walk_one(root, limits, cancel, mount_of, &mut report.files);
        report.cancelled |= walk.state == RootWalkState::Cancelled;
        report.roots.push(walk);
        if report.cancelled {
            break;
        }
    }
    report.files.sort_by(|a, b| a.path.cmp(&b.path));
    report
}

fn walk_one(
    root: &WalkRoot,
    limits: WalkLimits,
    cancel: &AtomicBool,
    mount_of: MountOf<'_>,
    files: &mut Vec<FileFacts>,
) -> RootWalk {
    let mut walk = RootWalk {
        source_id: root.source_id,
        state: RootWalkState::Complete,
        nested_boundaries: Vec::new(),
        inaccessible: Vec::new(),
        truncated: false,
    };
    let root_mount = match std::fs::symlink_metadata(&root.path)
        .ok()
        .filter(|m| m.is_dir() && !m.file_type().is_symlink())
        .and_then(|_| mount_of(&root.path).ok())
    {
        Some(mount) => mount,
        None => {
            walk.state = RootWalkState::Unavailable;
            return walk;
        }
    };
    let mut seen = 0usize;
    let mut stack = vec![(root.path.clone(), 0usize, root_mount)];
    while let Some((directory, depth, mount)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            walk.state = RootWalkState::Cancelled;
            return walk;
        }
        let mut entries: Vec<_> = match std::fs::read_dir(&directory) {
            Ok(read) => read.filter_map(Result::ok).collect(),
            Err(_) => {
                walk.inaccessible.push(directory);
                continue;
            }
        };
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                walk.inaccessible.push(path);
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if root.excluded.contains(&path) {
                    continue;
                }
                if depth >= limits.max_depth {
                    walk.truncated = true;
                    continue;
                }
                match mount_of(&path) {
                    Ok(child) if child == mount => stack.push((path, depth + 1, child)),
                    Ok(_) => walk.nested_boundaries.push(path),
                    // Unknown boundary: treat as unexplored, never as ordinary.
                    Err(_) => walk.inaccessible.push(path),
                }
            } else if kind.is_file() {
                if seen >= limits.max_files {
                    walk.truncated = true;
                    continue;
                }
                match entry.metadata() {
                    Ok(metadata) => {
                        seen += 1;
                        files.push(FileFacts {
                            path,
                            source_id: root.source_id,
                            size: metadata.len(),
                            hashes: Vec::new(),
                        });
                    }
                    Err(_) => walk.inaccessible.push(path),
                }
            }
        }
    }
    if walk.truncated || !walk.nested_boundaries.is_empty() || !walk.inaccessible.is_empty() {
        walk.state = RootWalkState::Partial;
    }
    walk.nested_boundaries.sort();
    walk.inaccessible.sort();
    walk
}
