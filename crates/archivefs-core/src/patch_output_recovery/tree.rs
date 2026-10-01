//! One new patch-output directory, published as a unit. Backend code still
//! owns patch decoding and independent semantic verification. No merging into
//! existing directories, deletion, automatic cleanup, or source writes.
//!
//! A sealed, immutable receipt is durable before rename. Recovery determines
//! state from the complete bound tree at its two possible locations, so a
//! crash after rename needs no guessed completion marker. Undo moves the tree
//! back to staging and retains its bytes. Failed preparation is retained too.
//! This is preservation against stale plans, not an OS security sandbox: the
//! existing pathname check/use windows and trusted same-user journal model
//! apply. Cooperating recovery calls serialize on the immutable receipt.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::dat::rename_apply::identity::capture_identity;
use crate::dat::rename_apply::model::{ObjectIdentity, ObjectKind};
use crate::dat::rename_apply::noclobber::rename_noreplace;

const MAX_ENTRIES: usize = 16_384;
const MAX_BYTES: u64 = 512 * 1024 * 1024;
const MAX_JOURNAL_BYTES: u64 = 16 * 1024 * 1024;
const MAX_DEPTH: usize = 64;

fn refuse(message: &str) -> io::Error {
    io::Error::other(message)
}

/// Directory object binding deliberately excludes mtime: creating children
/// and moving the root legitimately change directory timestamps.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct Directory(u64, u64);

fn directory(path: &Path) -> io::Result<Directory> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_dir() || m.file_type().is_symlink() {
        return Err(refuse("expected a real directory"));
    }
    Ok(Directory(m.dev(), m.ino()))
}

fn safe_path(path: &Path) -> io::Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|p| !matches!(p, Component::RootDir | Component::Normal(_)))
    {
        return Err(refuse("absolute path without traversal required"));
    }
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part);
        if fs::symlink_metadata(&current)?.file_type().is_symlink() {
            return Err(refuse("symlink path refused"));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum Entry {
    Directory(Directory),
    File(ObjectIdentity),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct Snapshot(BTreeMap<PathBuf, Entry>);

fn snapshot(root: &Path) -> io::Result<Snapshot> {
    fn visit(
        root: &Path,
        relative: &Path,
        entries: &mut BTreeMap<PathBuf, Entry>,
        bytes: &mut u64,
        depth: usize,
    ) -> io::Result<()> {
        if entries.len() >= MAX_ENTRIES || depth > MAX_DEPTH {
            return Err(refuse("patch tree entry/depth bound exceeded"));
        }
        let path = if relative.as_os_str().is_empty() {
            root.to_owned()
        } else {
            root.join(relative)
        };
        let meta = fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            let binding = directory(&path)?;
            entries.insert(relative.to_owned(), Entry::Directory(binding.clone()));
            // Collect only after enforcing the total entry bound; never follow links.
            for child in fs::read_dir(&path)? {
                visit(
                    root,
                    &relative.join(child?.file_name()),
                    entries,
                    bytes,
                    depth + 1,
                )?;
            }
            if directory(&path)? != binding {
                return Err(refuse("directory changed during inspection"));
            }
        } else if meta.is_file() && !meta.file_type().is_symlink() && meta.nlink() == 1 {
            *bytes = bytes
                .checked_add(meta.len())
                .ok_or_else(|| refuse("tree size overflow"))?;
            if *bytes > MAX_BYTES {
                return Err(refuse("patch tree byte bound exceeded"));
            }
            let identity = capture_identity(&path)?;
            if identity.size_bytes != meta.len()
                || identity.freshness.is_none()
                || identity.kind != ObjectKind::RegularFile
                || identity.ino != meta.ino()
                || identity.dev != meta.dev()
            {
                return Err(refuse("file changed during inspection"));
            }
            entries.insert(relative.to_owned(), Entry::File(identity));
        } else {
            return Err(refuse("symlink, hardlink or special file refused"));
        }
        Ok(())
    }
    safe_path(root)?;
    let mut entries = BTreeMap::new();
    visit(root, Path::new(""), &mut entries, &mut 0, 0)?;
    Ok(Snapshot(entries))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Input {
    path: PathBuf,
    snapshot: Snapshot,
}

/// Preview binding: pass the whole source tree for DCP, the complete component
/// set (or dedicated source root) for Saturn, and every patch/package input.
/// The backend remains responsible for selecting that complete dependency set.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TreePatchPlan {
    destination: PathBuf,
    parent: Directory,
    inputs: Vec<Input>,
}

impl TreePatchPlan {
    pub fn review(inputs: &[PathBuf], destination: &Path) -> io::Result<Self> {
        if inputs.is_empty() || inputs.len() > MAX_ENTRIES || destination.file_name().is_none() {
            return Err(refuse(
                "bounded nonempty input set and destination required",
            ));
        }
        let parent = destination
            .parent()
            .ok_or_else(|| refuse("destination has no parent"))?;
        safe_path(parent)?;
        if !destination.is_absolute()
            || destination
                .components()
                .any(|p| matches!(p, Component::ParentDir | Component::CurDir))
        {
            return Err(refuse("unsafe destination"));
        }
        absent(destination)?;
        let mut bound = Vec::new();
        let mut total_entries = 0;
        let mut total_bytes = 0u64;
        for path in inputs {
            if parent.starts_with(path) || path.starts_with(destination) {
                return Err(refuse("output staging overlaps source"));
            }
            let observed = snapshot(path)?;
            total_entries += observed.0.len();
            for entry in observed.0.values() {
                if let Entry::File(id) = entry {
                    total_bytes = total_bytes.saturating_add(id.size_bytes);
                }
            }
            if total_entries > MAX_ENTRIES || total_bytes > MAX_BYTES {
                return Err(refuse("input set exceeds bounds"));
            }
            bound.push(Input {
                path: path.clone(),
                snapshot: observed,
            });
        }
        Ok(Self {
            destination: destination.to_owned(),
            parent: directory(parent)?,
            inputs: bound,
        })
    }

    fn revalidate(&self) -> io::Result<()> {
        self.check_parent()?;
        for input in &self.inputs {
            if snapshot(&input.path)? != input.snapshot {
                return Err(refuse("patch input changed since preview"));
            }
        }
        Ok(())
    }

    fn check_parent(&self) -> io::Result<()> {
        let parent = self
            .destination
            .parent()
            .ok_or_else(|| refuse("missing parent"))?;
        safe_path(parent)?;
        if directory(parent)? != self.parent {
            return Err(refuse("output parent replaced"));
        }
        Ok(())
    }
}

fn absent(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
        Ok(_) => Err(refuse("destination already exists")),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Receipt {
    version: u32,
    plan: TreePatchPlan,
    staging: PathBuf,
    output: Snapshot,
}

/// A fully verified tree remains at staging until `publish` is explicitly
/// called. The journal path is the durable recovery handle; no mutable state
/// file is needed to distinguish pre/post-rename crashes.
#[derive(Debug)]
pub struct PreparedTreePatch {
    pub journal_path: PathBuf,
}

/// Backend callbacks may only mutate staging. `verify` must independently
/// prove expected component membership and domain facts (IP.BIN or optical
/// layout); hashing alone is not semantic verification. Verification itself
/// must be read-only. Failures retain staging for inspection, never publish it.
pub fn prepare<F, V>(plan: &TreePatchPlan, produce: F, verify: V) -> io::Result<PreparedTreePatch>
where
    F: FnOnce(&Path) -> io::Result<()>,
    V: FnOnce(&Path) -> io::Result<()>,
{
    plan.revalidate()?;
    absent(&plan.destination)?;
    let parent = plan.destination.parent().unwrap();
    let staging = tempfile::Builder::new()
        .prefix(".emuwiz-patch-tree-")
        .tempdir_in(parent)?
        .keep();
    let owned = directory(&staging)?;
    let result = (|| {
        produce(&staging)?;
        if directory(&staging)? != owned {
            return Err(refuse("staging directory replaced"));
        }
        let output = snapshot(&staging)?;
        if !output.0.values().any(|e| matches!(e, Entry::File(_))) {
            return Err(refuse("empty output tree"));
        }
        verify(&staging)?;
        if snapshot(&staging)? != output {
            return Err(refuse("output changed during verification"));
        }
        plan.revalidate()?;
        // File content and every directory entry reach storage before intent.
        for (relative, entry) in output.0.iter().rev() {
            let path = staging.join(relative);
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path)?;
            let _ = entry;
            file.sync_all()?;
        }
        let receipt = Receipt {
            version: 1,
            plan: plan.clone(),
            staging: staging.clone(),
            output,
        };
        let journal_path = staging.with_extension("json");
        let bytes = serde_json::to_vec(&receipt).map_err(io::Error::other)?;
        if bytes.len() as u64 > MAX_JOURNAL_BYTES {
            return Err(refuse("journal bound exceeded"));
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&journal_path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        File::open(parent)?.sync_all()?;
        Ok(PreparedTreePatch { journal_path })
    })();
    result.map_err(|e| {
        refuse(&format!(
            "{e}; unpublished staging retained at {}",
            staging.display()
        ))
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreePatchState {
    Staged,
    Published,
}

fn load(path: &Path) -> io::Result<(File, Receipt)> {
    safe_path(path)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    file.try_lock().map_err(io::Error::other)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > MAX_JOURNAL_BYTES {
        return Err(refuse("unsafe journal"));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_JOURNAL_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let receipt: Receipt = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if receipt.version != 1
        || receipt.staging.parent() != path.parent()
        || receipt.plan.destination.parent() != path.parent()
        || receipt.staging.with_extension("json") != path
        || !receipt
            .staging
            .file_name()
            .is_some_and(|s| s.to_string_lossy().starts_with(".emuwiz-patch-tree-"))
        || receipt.plan.destination == receipt.staging
        || receipt.plan.destination == path
    {
        return Err(refuse("invalid tree journal paths/version"));
    }
    receipt.plan.check_parent()?;
    Ok((file, receipt))
}

fn state(receipt: &Receipt) -> io::Result<TreePatchState> {
    if absent(&receipt.plan.destination).is_ok() && snapshot(&receipt.staging)? == receipt.output {
        return Ok(TreePatchState::Staged);
    }
    if absent(&receipt.staging).is_ok() && snapshot(&receipt.plan.destination)? == receipt.output {
        return Ok(TreePatchState::Published);
    }
    Err(refuse(
        "tree changed, incomplete, or present at conflicting locations",
    ))
}

pub fn inspect(journal: &Path) -> io::Result<TreePatchState> {
    let (_lease, receipt) = load(journal)?;
    state(&receipt)
}

/// Resume is the same operation as initial publication. Source/patch bindings
/// and the entire verified tree are rechecked. Existing destinations always
/// refuse, including empty directories and dangling symlinks.
pub fn publish(journal: &Path) -> io::Result<()> {
    let (_lease, receipt) = load(journal)?;
    receipt.plan.revalidate()?;
    if state(&receipt)? != TreePatchState::Staged {
        return Err(refuse("tree is not staged"));
    }
    rename_noreplace(&receipt.staging, &receipt.plan.destination).map_err(io::Error::other)?;
    File::open(receipt.staging.parent().unwrap())?.sync_all()?;
    if state(&receipt)? != TreePatchState::Published {
        return Err(refuse("publication verification failed; recovery required"));
    }
    Ok(())
}

/// Non-destructive undo: retains the complete verified tree at its original
/// staging path. Changed/replaced trees are refused, never recursively removed.
pub fn undo(journal: &Path) -> io::Result<()> {
    let (_lease, receipt) = load(journal)?;
    if state(&receipt)? != TreePatchState::Published {
        return Err(refuse("tree is not published"));
    }
    rename_noreplace(&receipt.plan.destination, &receipt.staging).map_err(io::Error::other)?;
    File::open(receipt.staging.parent().unwrap())?.sync_all()?;
    if state(&receipt)? != TreePatchState::Staged {
        return Err(refuse("undo verification failed; recovery required"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
