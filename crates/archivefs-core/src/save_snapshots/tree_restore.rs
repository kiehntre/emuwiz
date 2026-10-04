//! Generic whole-tree save restore as one journalled transaction, with undo.
//!
//! A verified [`SaveSnapshot`] is restored into a destination directory as a
//! single user-level transaction:
//!
//! ```text
//! plan (read-only, inspectable) -> stage + verify (outside the live tree)
//!   -> publish entry by entry -> post-verify -> journal "Published"
//! ```
//!
//! # Guarantees (and what is *not* promised)
//!
//! Publishing many paths cannot be one atomic POSIX operation. Each file
//! becomes visible with one atomic `rename`/`link`; the whole set is made
//! recoverable instead: before any live object is replaced, its exact previous
//! form is preserved (hard link to the old inode, or a renamed directory tree)
//! next to the destination, and one durable journal records plan, pre-state,
//! post-state and status. A failure at any publish point rolls the destination
//! back to its exact previous state; if that rollback cannot finish, the
//! journal, staging and preserved material are kept and the transaction is
//! `RecoveryRequired` (never silently half-restored). Anything not listed by
//! the snapshot is never touched.
//!
//! Undo verifies the current destination against the journal's post-state
//! first and refuses (changing nothing) if a save was modified since.
//!
//! Unix-only (hard links, no-clobber publish). Pathname races by a hostile
//! same-user process are out of scope, as for the other save restore paths.

use super::{
    MAX_SNAPSHOT_DEPTH, MAX_SNAPSHOT_FILES, SaveArtifactType, SaveQuiescenceRequirement,
    SaveSnapshot, SaveSnapshotCompleteness, SaveSnapshotError, hash_and_metadata, safe_join,
    safe_target_path, sync_parent, verify_snapshot, write_file,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, Read, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TRANSACTION: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeFile {
    pub relative_path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
}

/// What occupies a destination path before the restore.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviousObject {
    Absent,
    File {
        size_bytes: u64,
        sha256: String,
    },
    /// A whole directory tree (only when a snapshot *file* replaces it).
    Directory {
        files: Vec<TreeFile>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryAction {
    /// Destination absent: the file is newly created.
    Create,
    /// Destination is a regular file: replaced, old inode preserved.
    ReplaceFile,
    /// Destination is a directory but the snapshot has a file there: the old
    /// tree is moved aside whole and preserved.
    ReplaceDirectoryWithFile,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeRestoreEntry {
    pub relative_path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
    pub previous: PreviousObject,
    pub action: EntryAction,
}

/// A destination *file* sitting where the snapshot needs a directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplacedFile {
    pub relative_path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TreeRestoreRefusal {
    SnapshotIncomplete,
    SnapshotUnreadable,
    SaveStateNotSupported,
    EmptySnapshot,
    TooManyEntries,
    EmulatorRunning,
    EmulatorStateUnknown,
    DestinationUnsafe(String),
    UnsafeRelativePath(PathBuf),
    DuplicateDestination(PathBuf),
    PathCollision(PathBuf),
    CaseCollision(PathBuf),
    SymlinkOrSpecialInDestination(PathBuf),
    CrossFilesystem,
    DestinationChanged,
    TransactionInProgress,
}

/// Inspectable restore plan. Building it writes nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRestorePlan {
    pub snapshot_id: String,
    pub snapshot_manifest_sha256: String,
    pub destination_root: PathBuf,
    pub root_previously_existed: bool,
    /// Directories to create, relative to the root, shallowest first.
    pub directories_to_create: Vec<PathBuf>,
    pub displaced_files: Vec<DisplacedFile>,
    pub entries: Vec<TreeRestoreEntry>,
    /// Existing destination files the restore does not touch.
    pub untouched_files: Vec<PathBuf>,
    pub quiescence: SaveQuiescenceRequirement,
    pub refusals: Vec<TreeRestoreRefusal>,
}

impl TreeRestorePlan {
    pub fn ready(&self) -> bool {
        self.refusals.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TreeRestoreStatus {
    Planned,
    Staged,
    Publishing,
    Published,
    RollingBack,
    RolledBack,
    Undoing,
    Undone,
    RecoveryRequired,
}

/// The single durable record of one restore transaction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeRestoreJournal {
    pub transaction_id: String,
    pub status: TreeRestoreStatus,
    pub snapshot_id: String,
    pub snapshot_manifest_sha256: String,
    pub destination_root: PathBuf,
    pub root_previously_existed: bool,
    pub created_directories: Vec<PathBuf>,
    pub displaced_files: Vec<DisplacedFile>,
    pub entries: Vec<TreeRestoreEntry>,
    pub untouched_files: Vec<PathBuf>,
    /// Staging and preserved material (same filesystem as the destination).
    pub work_dir: PathBuf,
    pub lock_path: PathBuf,
    pub journal_path: PathBuf,
    pub started_unix_seconds: u64,
    pub finished_unix_seconds: Option<u64>,
    pub detail: Option<String>,
    /// Set once an undo has begun, so recovery finishes it as an undo.
    #[serde(default)]
    pub undo_requested: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRestoreOptions {
    /// Directory receiving `<transaction id>.json`.
    pub journal_root: PathBuf,
    pub now_unix_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeConflict {
    pub path: PathBuf,
    pub expected: String,
    pub found: String,
}

#[derive(Debug)]
pub enum TreeRestoreError {
    Refused(Vec<TreeRestoreRefusal>),
    Snapshot(SaveSnapshotError),
    Io {
        path: PathBuf,
        detail: String,
    },
    /// Publishing failed and the destination was restored to its exact
    /// previous state.
    RolledBack {
        journal_path: PathBuf,
        cause: String,
    },
    /// Rollback itself could not finish. Journal, staging and preserved
    /// material are kept; run [`recover_tree_restore`].
    RecoveryRequired {
        journal_path: PathBuf,
        detail: String,
    },
    /// Undo refused: the destination no longer matches the journal's
    /// post-state. Nothing was changed.
    UndoConflict {
        journal_path: PathBuf,
        conflicts: Vec<TreeConflict>,
    },
}

impl std::fmt::Display for TreeRestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(reasons) => write!(f, "tree restore refused: {reasons:?}"),
            Self::Snapshot(error) => write!(f, "tree restore snapshot failed: {error}"),
            Self::Io { path, detail } => {
                write!(f, "tree restore I/O at {}: {detail}", path.display())
            }
            Self::RolledBack { cause, .. } => {
                write!(f, "tree restore failed and was rolled back: {cause}")
            }
            Self::RecoveryRequired {
                journal_path,
                detail,
            } => write!(
                f,
                "tree restore needs recovery (journal {}): {detail}",
                journal_path.display()
            ),
            Self::UndoConflict { conflicts, .. } => write!(
                f,
                "undo refused: {} path(s) changed since the restore",
                conflicts.len()
            ),
        }
    }
}
impl std::error::Error for TreeRestoreError {}

impl From<SaveSnapshotError> for TreeRestoreError {
    fn from(value: SaveSnapshotError) -> Self {
        Self::Snapshot(value)
    }
}

/// Points at which a failure can be injected (tests) or observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Stage(usize),
    Publish(usize),
    Rollback(usize),
}

type Hook<'a> = &'a dyn Fn(Phase) -> io::Result<()>;

fn no_hook(_: Phase) -> io::Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Dir,
    Other,
}

fn kind_of(path: &Path) -> io::Result<Option<Kind>> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => Ok(Some(Kind::Other)),
        Ok(m) if m.is_file() => Ok(Some(Kind::File)),
        Ok(m) if m.is_dir() => Ok(Some(Kind::Dir)),
        Ok(_) => Ok(Some(Kind::Other)),
        // ENOTDIR: a parent component is a file, so nothing lives at this path.
        Err(e) if e.kind() == io::ErrorKind::NotFound || e.raw_os_error() == Some(20) => Ok(None),
        Err(e) => Err(e),
    }
}

fn io_err(path: &Path, error: impl ToString) -> TreeRestoreError {
    TreeRestoreError::Io {
        path: path.to_path_buf(),
        detail: error.to_string(),
    }
}

fn manifest_sha256(snapshot: &SaveSnapshot) -> String {
    let mut digest = Sha256::new();
    for artifact in &snapshot.manifest.artifacts {
        digest.update(artifact.relative_path.as_os_str().as_encoded_bytes());
        digest.update([0]);
        digest.update(artifact.size_bytes.to_le_bytes());
        digest.update(artifact.sha256.as_bytes());
        digest.update([0]);
    }
    digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn valid_relative(path: &Path) -> bool {
    let mut depth = 0;
    for component in path.components() {
        match component {
            Component::Normal(name) if !name.as_encoded_bytes().contains(&0) => depth += 1,
            _ => return false,
        }
    }
    depth > 0 && depth <= MAX_SNAPSHOT_DEPTH
}

/// Every regular file below `root` (relative paths), plus whether anything
/// other than files and directories was met. Never follows symlinks.
fn walk_tree(root: &Path) -> io::Result<(BTreeMap<PathBuf, Kind>, usize)> {
    let mut found = BTreeMap::new();
    let mut other = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            match kind_of(&path)? {
                Some(Kind::Dir) => {
                    found.insert(relative, Kind::Dir);
                    stack.push(path);
                }
                Some(Kind::File) => {
                    found.insert(relative, Kind::File);
                }
                _ => {
                    other += 1;
                    found.insert(relative, Kind::Other);
                }
            }
            if found.len() > MAX_SNAPSHOT_FILES * 4 {
                return Err(io::Error::other("destination tree is too large to plan"));
            }
        }
    }
    Ok((found, other))
}

fn tree_files(root: &Path) -> Result<Vec<TreeFile>, String> {
    let (found, other) = walk_tree(root).map_err(|e| e.to_string())?;
    if other > 0 {
        return Err("tree contains a symlink or special file".into());
    }
    let mut files = Vec::new();
    for (relative, kind) in found {
        if kind == Kind::File {
            let (size, sha, _) =
                hash_and_metadata(&root.join(&relative)).map_err(|e| e.to_string())?;
            files.push(TreeFile {
                relative_path: relative,
                size_bytes: size,
                sha256: sha,
            });
        }
    }
    Ok(files)
}

/// Build the restore plan. Reads the snapshot and the destination; writes
/// nothing.
pub fn plan_tree_restore(
    snapshot: &SaveSnapshot,
    destination_root: &Path,
    quiescence: SaveQuiescenceRequirement,
) -> TreeRestorePlan {
    let mut refusals = Vec::new();
    let mut plan = TreeRestorePlan {
        snapshot_id: snapshot.manifest.snapshot_id.clone(),
        snapshot_manifest_sha256: manifest_sha256(snapshot),
        destination_root: destination_root.to_path_buf(),
        root_previously_existed: false,
        directories_to_create: Vec::new(),
        displaced_files: Vec::new(),
        entries: Vec::new(),
        untouched_files: Vec::new(),
        quiescence,
        refusals: Vec::new(),
    };

    match quiescence {
        SaveQuiescenceRequirement::ConfirmedClosed => {}
        SaveQuiescenceRequirement::Running => refusals.push(TreeRestoreRefusal::EmulatorRunning),
        SaveQuiescenceRequirement::Unknown => {
            refusals.push(TreeRestoreRefusal::EmulatorStateUnknown)
        }
    }
    if snapshot.manifest.completeness != SaveSnapshotCompleteness::Complete {
        refusals.push(TreeRestoreRefusal::SnapshotIncomplete);
    }
    if snapshot.manifest.artifact_type == SaveArtifactType::SaveState {
        refusals.push(TreeRestoreRefusal::SaveStateNotSupported);
    }
    if snapshot.manifest.artifacts.is_empty() {
        refusals.push(TreeRestoreRefusal::EmptySnapshot);
    }
    if snapshot.manifest.artifacts.len() > MAX_SNAPSHOT_FILES {
        refusals.push(TreeRestoreRefusal::TooManyEntries);
        plan.refusals = refusals;
        return plan;
    }
    if verify_snapshot(snapshot).is_err() {
        refusals.push(TreeRestoreRefusal::SnapshotUnreadable);
    }

    // --- destination root -------------------------------------------------
    let root = destination_root;
    let parent = root.parent();
    let parent_ok = safe_target_path(root)
        && parent.is_some_and(|p| {
            matches!(kind_of(p), Ok(Some(Kind::Dir))) && super::safe_existing_parent(p)
        });
    if !parent_ok {
        refusals.push(TreeRestoreRefusal::DestinationUnsafe(
            "destination root must be absolute with an existing real parent directory".into(),
        ));
        plan.refusals = refusals;
        return plan;
    }
    match kind_of(root) {
        Ok(Some(Kind::Dir)) => plan.root_previously_existed = true,
        Ok(None) => {}
        _ => {
            refusals.push(TreeRestoreRefusal::DestinationUnsafe(
                "destination root is not a real directory".into(),
            ));
            plan.refusals = refusals;
            return plan;
        }
    }
    if plan.root_previously_existed {
        let same_device = fs::metadata(root).ok().map(|m| m.dev())
            == fs::metadata(parent.unwrap()).ok().map(|m| m.dev());
        if !same_device {
            refusals.push(TreeRestoreRefusal::CrossFilesystem);
        }
    }

    // --- snapshot entries: shape, duplicates, collisions -----------------
    let files_root = snapshot.storage_path.join("files");
    let mut seen = BTreeSet::new();
    let mut folded: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut file_paths: BTreeSet<PathBuf> = BTreeSet::new();
    let mut dir_paths: BTreeSet<PathBuf> = BTreeSet::new();
    let mut artifacts: Vec<_> = snapshot.manifest.artifacts.iter().collect();
    artifacts.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    for artifact in &artifacts {
        let relative = &artifact.relative_path;
        if !valid_relative(relative) || safe_join(&files_root, relative).is_err() {
            refusals.push(TreeRestoreRefusal::UnsafeRelativePath(relative.clone()));
            continue;
        }
        if !seen.insert(relative.clone()) {
            refusals.push(TreeRestoreRefusal::DuplicateDestination(relative.clone()));
            continue;
        }
        let key = relative.to_string_lossy().to_lowercase();
        if let Some(other) = folded.insert(key, relative.clone())
            && other != *relative
        {
            refusals.push(TreeRestoreRefusal::CaseCollision(relative.clone()));
        }
        match kind_of(&files_root.join(relative)) {
            Ok(Some(Kind::File)) => {}
            _ => refusals.push(TreeRestoreRefusal::SnapshotUnreadable),
        }
        file_paths.insert(relative.clone());
        for ancestor in relative.ancestors().skip(1) {
            if !ancestor.as_os_str().is_empty() {
                dir_paths.insert(ancestor.to_path_buf());
            }
        }
    }
    for dir in &dir_paths {
        if file_paths.contains(dir) {
            refusals.push(TreeRestoreRefusal::PathCollision(dir.clone()));
        }
    }
    if !refusals.is_empty() {
        plan.refusals = refusals;
        return plan;
    }

    // --- destination state under the root ----------------------------------
    let mut existing: BTreeMap<PathBuf, Kind> = BTreeMap::new();
    if plan.root_previously_existed {
        match walk_tree(root) {
            Ok((found, _)) => existing = found,
            Err(error) => {
                refusals.push(TreeRestoreRefusal::DestinationUnsafe(error.to_string()));
                plan.refusals = refusals;
                return plan;
            }
        }
    }
    // Case-only differences against existing siblings are unsafe to publish.
    let mut existing_folded: BTreeMap<String, &PathBuf> = BTreeMap::new();
    for path in existing.keys() {
        existing_folded.insert(path.to_string_lossy().to_lowercase(), path);
    }
    for relative in file_paths.iter().chain(dir_paths.iter()) {
        if let Some(found) = existing_folded.get(&relative.to_string_lossy().to_lowercase())
            && *found != relative
        {
            refusals.push(TreeRestoreRefusal::CaseCollision(relative.clone()));
        }
    }

    let mut directories = Vec::new();
    for dir in &dir_paths {
        match existing.get(dir) {
            Some(Kind::Dir) => {}
            Some(Kind::File) => {
                let path = root.join(dir);
                match hash_and_metadata(&path) {
                    Ok((size, sha, _)) => plan.displaced_files.push(DisplacedFile {
                        relative_path: dir.clone(),
                        size_bytes: size,
                        sha256: sha,
                    }),
                    Err(_) => {
                        refusals.push(TreeRestoreRefusal::SymlinkOrSpecialInDestination(
                            dir.clone(),
                        ));
                    }
                }
                directories.push(dir.clone());
            }
            Some(Kind::Other) => refusals.push(TreeRestoreRefusal::SymlinkOrSpecialInDestination(
                dir.clone(),
            )),
            None => directories.push(dir.clone()),
        }
    }
    directories.sort_by_key(|p| (p.components().count(), p.clone()));
    plan.directories_to_create = directories;

    for artifact in artifacts {
        let relative = &artifact.relative_path;
        let path = root.join(relative);
        let (previous, action) = match existing.get(relative) {
            None => (PreviousObject::Absent, EntryAction::Create),
            Some(Kind::File) => match hash_and_metadata(&path) {
                Ok((size, sha, _)) => (
                    PreviousObject::File {
                        size_bytes: size,
                        sha256: sha,
                    },
                    EntryAction::ReplaceFile,
                ),
                Err(_) => {
                    refusals.push(TreeRestoreRefusal::SymlinkOrSpecialInDestination(
                        relative.clone(),
                    ));
                    continue;
                }
            },
            Some(Kind::Dir) => match tree_files(&path) {
                Ok(files) => (
                    PreviousObject::Directory { files },
                    EntryAction::ReplaceDirectoryWithFile,
                ),
                Err(_) => {
                    refusals.push(TreeRestoreRefusal::SymlinkOrSpecialInDestination(
                        relative.clone(),
                    ));
                    continue;
                }
            },
            Some(Kind::Other) => {
                refusals.push(TreeRestoreRefusal::SymlinkOrSpecialInDestination(
                    relative.clone(),
                ));
                continue;
            }
        };
        plan.entries.push(TreeRestoreEntry {
            relative_path: relative.clone(),
            size_bytes: artifact.size_bytes,
            sha256: artifact.sha256.clone(),
            previous,
            action,
        });
    }

    // Anything below an existing symlink/special object must not be touched
    // either; the walk records those as `Other`, never descended into.
    let touched: BTreeSet<&PathBuf> = file_paths.iter().chain(dir_paths.iter()).collect();
    for (path, kind) in &existing {
        if *kind == Kind::File
            && !touched.contains(path)
            && !plan
                .entries
                .iter()
                .any(|entry| path.starts_with(&entry.relative_path))
        {
            plan.untouched_files.push(path.clone());
        }
    }
    plan.refusals = refusals;
    plan
}

// ---------------------------------------------------------------------------
// Journal persistence, lock, work dir
// ---------------------------------------------------------------------------

fn persist_journal(journal: &TreeRestoreJournal) -> Result<(), TreeRestoreError> {
    let bytes = serde_json::to_vec_pretty(journal).map_err(|e| io_err(&journal.journal_path, e))?;
    let partial = journal.journal_path.with_extension("json.partial");
    write_file(&partial, &bytes)?;
    fs::rename(&partial, &journal.journal_path).map_err(|e| io_err(&journal.journal_path, e))?;
    if let Some(parent) = journal.journal_path.parent() {
        sync_parent(parent).map_err(|e| io_err(parent, e))?;
    }
    Ok(())
}

/// Reads a transaction journal (survives process restarts).
pub fn load_tree_restore_journal(path: &Path) -> Result<TreeRestoreJournal, TreeRestoreError> {
    let bytes = fs::read(path).map_err(|e| io_err(path, e))?;
    serde_json::from_slice(&bytes).map_err(|e| io_err(path, e))
}

fn lock_path_for(root: &Path) -> PathBuf {
    let mut digest = Sha256::new();
    digest.update(root.as_os_str().as_encoded_bytes());
    let token: String = digest
        .finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect();
    root.parent()
        .unwrap_or(Path::new("/"))
        .join(format!(".emuwiz-restore-lock-{token}"))
}

fn acquire_lock(path: &Path, transaction_id: &str) -> Result<(), TreeRestoreError> {
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                TreeRestoreError::Refused(vec![TreeRestoreRefusal::TransactionInProgress])
            } else {
                io_err(path, error)
            }
        })?;
    file.write_all(transaction_id.as_bytes())
        .map_err(|e| io_err(path, e))
}

fn release_lock(path: &Path) {
    let _ = fs::remove_file(path);
}

fn new_transaction_id(now: u64) -> String {
    format!(
        "tree-restore-{now}-{}-{}",
        std::process::id(),
        NEXT_TRANSACTION.fetch_add(1, Ordering::Relaxed)
    )
}

fn rel(root: &Path, relative: &Path) -> PathBuf {
    root.join(relative)
}

fn make_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(parent) => fs::create_dir_all(parent),
        None => Ok(()),
    }
}

fn sync_dir(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = sync_parent(parent);
    }
}

// ---------------------------------------------------------------------------
// Apply
// ---------------------------------------------------------------------------

fn copy_verified(source: &Path, destination: &Path, sha256: &str) -> io::Result<()> {
    make_parent(destination)?;
    let mut input = File::open(source)?;
    let mut output = File::options()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count])?;
        digest.update(&buffer[..count]);
    }
    output.sync_all()?;
    let actual: String = digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual != sha256 {
        let _ = fs::remove_file(destination);
        return Err(io::Error::other("staged copy hash mismatch"));
    }
    Ok(())
}

fn file_matches(path: &Path, sha256: &str) -> bool {
    matches!(kind_of(path), Ok(Some(Kind::File)))
        && hash_and_metadata(path).is_ok_and(|(_, sha, _)| sha == sha256)
}

/// Restore a snapshot into `plan.destination_root` as one transaction.
pub fn apply_tree_restore(
    snapshot: &SaveSnapshot,
    plan: &TreeRestorePlan,
    options: &TreeRestoreOptions,
) -> Result<TreeRestoreJournal, TreeRestoreError> {
    apply_with_hook(snapshot, plan, options, &no_hook)
}

pub(crate) fn apply_with_hook(
    snapshot: &SaveSnapshot,
    plan: &TreeRestorePlan,
    options: &TreeRestoreOptions,
    hook: Hook<'_>,
) -> Result<TreeRestoreJournal, TreeRestoreError> {
    if !plan.ready() {
        return Err(TreeRestoreError::Refused(plan.refusals.clone()));
    }
    // The plan must still describe reality (and the snapshot must be intact).
    let current = plan_tree_restore(snapshot, &plan.destination_root, plan.quiescence);
    if !current.ready() {
        return Err(TreeRestoreError::Refused(current.refusals));
    }
    if current != *plan {
        return Err(TreeRestoreError::Refused(vec![
            TreeRestoreRefusal::DestinationChanged,
        ]));
    }
    fs::create_dir_all(&options.journal_root).map_err(|e| io_err(&options.journal_root, e))?;

    let root = plan.destination_root.clone();
    let parent = root
        .parent()
        .expect("plan guarantees a parent")
        .to_path_buf();
    let transaction_id = new_transaction_id(options.now_unix_seconds);
    let lock_path = lock_path_for(&root);
    acquire_lock(&lock_path, &transaction_id)?;
    let work_dir = parent.join(format!(".emuwiz-restore-{transaction_id}"));
    let mut journal = TreeRestoreJournal {
        transaction_id: transaction_id.clone(),
        status: TreeRestoreStatus::Planned,
        snapshot_id: plan.snapshot_id.clone(),
        snapshot_manifest_sha256: plan.snapshot_manifest_sha256.clone(),
        destination_root: root.clone(),
        root_previously_existed: plan.root_previously_existed,
        created_directories: plan.directories_to_create.clone(),
        displaced_files: plan.displaced_files.clone(),
        entries: plan.entries.clone(),
        untouched_files: plan.untouched_files.clone(),
        work_dir: work_dir.clone(),
        lock_path: lock_path.clone(),
        journal_path: options.journal_root.join(format!("{transaction_id}.json")),
        started_unix_seconds: options.now_unix_seconds,
        finished_unix_seconds: None,
        detail: None,
        undo_requested: false,
    };

    // ---- stage (nothing live is touched) ----------------------------------
    let staged = (|| -> Result<(), TreeRestoreError> {
        fs::create_dir(&work_dir).map_err(|e| io_err(&work_dir, e))?;
        persist_journal(&journal)?;
        let files_root = snapshot.storage_path.join("files");
        for (index, entry) in journal.entries.iter().enumerate() {
            hook(Phase::Stage(index)).map_err(|e| io_err(&work_dir, e))?;
            let source = safe_join(&files_root, &entry.relative_path)?;
            let target = work_dir.join("stage").join(&entry.relative_path);
            copy_verified(&source, &target, &entry.sha256).map_err(|e| io_err(&target, e))?;
            if !file_matches(&target, &entry.sha256) {
                return Err(io_err(&target, "staged file failed verification"));
            }
        }
        Ok(())
    })();
    if let Err(error) = staged {
        let _ = fs::remove_dir_all(&work_dir);
        journal.status = TreeRestoreStatus::RolledBack;
        journal.detail = Some(format!("staging failed; destination untouched: {error}"));
        journal.finished_unix_seconds = Some(options.now_unix_seconds);
        let _ = persist_journal(&journal);
        release_lock(&lock_path);
        return Err(error);
    }
    journal.status = TreeRestoreStatus::Staged;
    persist_journal(&journal)?;

    // ---- publish ------------------------------------------------------------
    journal.status = TreeRestoreStatus::Publishing;
    persist_journal(&journal)?;
    if let Err(cause) = publish(&journal, hook).and_then(|()| verify_published(&journal)) {
        return Err(roll_back(
            &mut journal,
            cause,
            TreeRestoreStatus::RolledBack,
            hook,
            options.now_unix_seconds,
        ));
    }
    let _ = fs::remove_dir_all(work_dir.join("stage"));
    journal.status = TreeRestoreStatus::Published;
    journal.finished_unix_seconds = Some(options.now_unix_seconds);
    persist_journal(&journal)?;
    release_lock(&lock_path);
    Ok(journal)
}

fn publish(journal: &TreeRestoreJournal, hook: Hook<'_>) -> Result<(), String> {
    let root = &journal.destination_root;
    let work = &journal.work_dir;
    let mut step = 0usize;
    let next = |step: &mut usize| -> Result<(), String> {
        let n = *step;
        *step += 1;
        hook(Phase::Publish(n)).map_err(|e| e.to_string())
    };
    let err = |path: &Path, e: io::Error| format!("{}: {e}", path.display());

    if !journal.root_previously_existed {
        next(&mut step)?;
        fs::create_dir(root).map_err(|e| err(root, e))?;
    }
    for displaced in &journal.displaced_files {
        next(&mut step)?;
        let from = rel(root, &displaced.relative_path);
        let to = work
            .join("preserved/displaced-files")
            .join(&displaced.relative_path);
        make_parent(&to).map_err(|e| err(&to, e))?;
        fs::rename(&from, &to).map_err(|e| err(&from, e))?;
    }
    for dir in &journal.created_directories {
        next(&mut step)?;
        let path = rel(root, dir);
        fs::create_dir(&path).map_err(|e| err(&path, e))?;
    }
    for entry in &journal.entries {
        next(&mut step)?;
        let dest = rel(root, &entry.relative_path);
        let staged = work.join("stage").join(&entry.relative_path);
        match entry.action {
            EntryAction::Create => {
                publish_new(&staged, &dest).map_err(|e| err(&dest, e))?;
            }
            EntryAction::ReplaceFile => {
                let keep = work.join("preserved/files").join(&entry.relative_path);
                make_parent(&keep).map_err(|e| err(&keep, e))?;
                if fs::hard_link(&dest, &keep).is_err() {
                    let PreviousObject::File { sha256, .. } = &entry.previous else {
                        return Err("journal entry lost its previous file state".into());
                    };
                    copy_verified(&dest, &keep, sha256).map_err(|e| err(&keep, e))?;
                }
                let PreviousObject::File { sha256, .. } = &entry.previous else {
                    return Err("journal entry lost its previous file state".into());
                };
                if !file_matches(&keep, sha256) {
                    return Err(format!(
                        "{} changed after it was planned; refusing to replace it",
                        dest.display()
                    ));
                }
                fs::rename(&staged, &dest).map_err(|e| err(&dest, e))?;
            }
            EntryAction::ReplaceDirectoryWithFile => {
                let keep = work.join("preserved/dirs").join(&entry.relative_path);
                make_parent(&keep).map_err(|e| err(&keep, e))?;
                fs::rename(&dest, &keep).map_err(|e| err(&dest, e))?;
                publish_new(&staged, &dest).map_err(|e| err(&dest, e))?;
            }
        }
    }
    sync_dir(root);
    Ok(())
}

/// Publish a staged file at an absent destination without clobbering.
fn publish_new(staged: &Path, dest: &Path) -> io::Result<()> {
    match fs::hard_link(staged, dest) {
        Ok(()) => fs::remove_file(staged),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(error),
        Err(_) if kind_of(dest)?.is_none() => fs::rename(staged, dest),
        Err(error) => Err(error),
    }
}

fn verify_published(journal: &TreeRestoreJournal) -> Result<(), String> {
    for entry in &journal.entries {
        let path = rel(&journal.destination_root, &entry.relative_path);
        if !file_matches(&path, &entry.sha256) {
            return Err(format!(
                "{} did not verify after publication",
                path.display()
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rollback (shared by failed publish, undo and recovery). It inspects what is
// really on disk, so it is idempotent and safe after a crash.
// ---------------------------------------------------------------------------

fn roll_back(
    journal: &mut TreeRestoreJournal,
    cause: String,
    done: TreeRestoreStatus,
    hook: Hook<'_>,
    now: u64,
) -> TreeRestoreError {
    journal.status = TreeRestoreStatus::RollingBack;
    let _ = persist_journal(journal);
    match rollback_state(journal, hook) {
        Ok(()) => {
            let _ = fs::remove_dir_all(&journal.work_dir);
            journal.status = done;
            journal.detail = Some(cause.clone());
            journal.finished_unix_seconds = Some(now);
            let _ = persist_journal(journal);
            release_lock(&journal.lock_path);
            TreeRestoreError::RolledBack {
                journal_path: journal.journal_path.clone(),
                cause,
            }
        }
        Err(detail) => {
            journal.status = TreeRestoreStatus::RecoveryRequired;
            journal.detail = Some(format!("{cause}; rollback incomplete: {detail}"));
            let _ = persist_journal(journal);
            TreeRestoreError::RecoveryRequired {
                journal_path: journal.journal_path.clone(),
                detail: journal.detail.clone().unwrap_or_default(),
            }
        }
    }
}

fn rollback_state(journal: &TreeRestoreJournal, hook: Hook<'_>) -> Result<(), String> {
    let root = &journal.destination_root;
    let work = &journal.work_dir;
    let mut problems: Vec<String> = Vec::new();
    let mut step = 0usize;
    let allowed = |step: &mut usize| -> Result<(), String> {
        let n = *step;
        *step += 1;
        hook(Phase::Rollback(n)).map_err(|e| e.to_string())
    };
    macro_rules! attempt {
        ($what:expr, $body:expr) => {{
            let outcome: Result<(), String> = (|| {
                allowed(&mut step)?;
                $body
            })();
            if let Err(error) = outcome {
                problems.push(format!("{}: {error}", $what));
            }
        }};
    }

    for entry in journal.entries.iter().rev() {
        let dest = rel(root, &entry.relative_path);
        let label = entry.relative_path.display().to_string();
        let kind = kind_of(&dest).map_err(|e| e.to_string())?;
        match entry.action {
            EntryAction::Create => match kind {
                None => {}
                Some(Kind::File) if file_matches(&dest, &entry.sha256) => {
                    attempt!(&label, fs::remove_file(&dest).map_err(|e| e.to_string()));
                }
                // Publishing never clobbers: a file here with other content was
                // not written by this transaction (or is newer user data), so it
                // is left exactly as found.
                _ => {}
            },
            EntryAction::ReplaceFile => {
                let PreviousObject::File { sha256: old, .. } = &entry.previous else {
                    problems.push(format!("{label}: journal lost the previous state"));
                    continue;
                };
                let keep = work.join("preserved/files").join(&entry.relative_path);
                if file_matches(&dest, &entry.sha256) && entry.sha256 != *old {
                    if !file_matches(&keep, old) {
                        problems.push(format!("{label}: preserved original is missing"));
                        continue;
                    }
                    attempt!(&label, {
                        let tmp =
                            dest.with_extension(format!("restore-undo-{}", journal.transaction_id));
                        let _ = fs::remove_file(&tmp);
                        if fs::hard_link(&keep, &tmp).is_err() {
                            copy_verified(&keep, &tmp, old).map_err(|e| e.to_string())?;
                        }
                        fs::rename(&tmp, &dest).map_err(|e| e.to_string())
                    });
                } else if !file_matches(&dest, old) {
                    problems.push(format!("{label}: changed during the transaction"));
                }
            }
            EntryAction::ReplaceDirectoryWithFile => {
                let keep = work.join("preserved/dirs").join(&entry.relative_path);
                match kind {
                    Some(Kind::File) if file_matches(&dest, &entry.sha256) => {
                        attempt!(&label, fs::remove_file(&dest).map_err(|e| e.to_string()));
                    }
                    Some(Kind::Dir) | None => {}
                    _ => {
                        problems.push(format!("{label}: changed during the transaction"));
                        continue;
                    }
                }
                if kind_of(&dest).map_err(|e| e.to_string())?.is_none()
                    && kind_of(&keep).map_err(|e| e.to_string())?.is_some()
                {
                    attempt!(&label, fs::rename(&keep, &dest).map_err(|e| e.to_string()));
                }
            }
        }
    }
    for dir in journal.created_directories.iter().rev() {
        let path = rel(root, dir);
        match kind_of(&path).map_err(|e| e.to_string())? {
            Some(Kind::Dir) => {
                attempt!(
                    dir.display().to_string(),
                    fs::remove_dir(&path).map_err(|e| format!("not removable: {e}"))
                );
            }
            None => {}
            // The displaced original file is still in place (the displacement
            // step never ran or was already reverted): nothing to undo here.
            Some(Kind::File)
                if journal
                    .displaced_files
                    .iter()
                    .any(|displaced| displaced.relative_path == *dir) => {}
            _ => problems.push(format!("{}: changed during the transaction", dir.display())),
        }
    }
    for displaced in journal.displaced_files.iter().rev() {
        let dest = rel(root, &displaced.relative_path);
        let keep = work
            .join("preserved/displaced-files")
            .join(&displaced.relative_path);
        let label = displaced.relative_path.display().to_string();
        match kind_of(&dest).map_err(|e| e.to_string())? {
            None if kind_of(&keep).map_err(|e| e.to_string())?.is_some() => {
                attempt!(&label, fs::rename(&keep, &dest).map_err(|e| e.to_string()));
            }
            Some(Kind::File) if file_matches(&dest, &displaced.sha256) => {}
            _ => problems.push(format!("{label}: original file could not be restored")),
        }
    }
    if !journal.root_previously_existed
        && kind_of(root).map_err(|e| e.to_string())? == Some(Kind::Dir)
    {
        attempt!(
            "destination root",
            fs::remove_dir(root).map_err(|e| format!("created root not empty: {e}"))
        );
    }
    sync_dir(root);
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}

// ---------------------------------------------------------------------------
// Undo and recovery
// ---------------------------------------------------------------------------

/// Undo a published transaction. Refuses, changing nothing, if any restored
/// path was modified since, or if the preserved originals are missing.
pub fn undo_tree_restore(
    journal_path: &Path,
    quiescence: SaveQuiescenceRequirement,
    now_unix_seconds: u64,
) -> Result<TreeRestoreJournal, TreeRestoreError> {
    undo_with_hook(journal_path, quiescence, now_unix_seconds, &no_hook)
}

pub(crate) fn undo_with_hook(
    journal_path: &Path,
    quiescence: SaveQuiescenceRequirement,
    now: u64,
    hook: Hook<'_>,
) -> Result<TreeRestoreJournal, TreeRestoreError> {
    let mut journal = load_tree_restore_journal(journal_path)?;
    match quiescence {
        SaveQuiescenceRequirement::ConfirmedClosed => {}
        SaveQuiescenceRequirement::Running => {
            return Err(TreeRestoreError::Refused(vec![
                TreeRestoreRefusal::EmulatorRunning,
            ]));
        }
        SaveQuiescenceRequirement::Unknown => {
            return Err(TreeRestoreError::Refused(vec![
                TreeRestoreRefusal::EmulatorStateUnknown,
            ]));
        }
    }
    if journal.status != TreeRestoreStatus::Published {
        return Err(TreeRestoreError::Refused(vec![
            TreeRestoreRefusal::DestinationChanged,
        ]));
    }
    let conflicts = undo_conflicts(&journal);
    if !conflicts.is_empty() {
        return Err(TreeRestoreError::UndoConflict {
            journal_path: journal.journal_path.clone(),
            conflicts,
        });
    }
    acquire_lock(&journal.lock_path, &journal.transaction_id)?;
    journal.status = TreeRestoreStatus::Undoing;
    journal.undo_requested = true;
    if let Err(error) = persist_journal(&journal) {
        release_lock(&journal.lock_path);
        return Err(error);
    }
    match roll_back(
        &mut journal,
        "undone by request".into(),
        TreeRestoreStatus::Undone,
        hook,
        now,
    ) {
        TreeRestoreError::RolledBack { .. } => Ok(journal),
        other => Err(other),
    }
}

fn undo_conflicts(journal: &TreeRestoreJournal) -> Vec<TreeConflict> {
    let root = &journal.destination_root;
    let mut conflicts = Vec::new();
    let mut conflict = |path: &Path, expected: &str, found: String| {
        conflicts.push(TreeConflict {
            path: path.to_path_buf(),
            expected: expected.to_string(),
            found,
        });
    };
    for entry in &journal.entries {
        let dest = rel(root, &entry.relative_path);
        if !file_matches(&dest, &entry.sha256) {
            let found = match kind_of(&dest) {
                Ok(None) => "missing".to_string(),
                Ok(Some(Kind::File)) => "modified since the restore".to_string(),
                _ => "replaced by another kind of object".to_string(),
            };
            conflict(&entry.relative_path, "restored file unchanged", found);
        }
        // The preserved originals must still be exact, or undo cannot be exact.
        match (&entry.previous, entry.action) {
            (PreviousObject::File { sha256, .. }, EntryAction::ReplaceFile) => {
                let keep = journal
                    .work_dir
                    .join("preserved/files")
                    .join(&entry.relative_path);
                if !file_matches(&keep, sha256) {
                    conflict(
                        &entry.relative_path,
                        "preserved original intact",
                        "preserved original missing or altered".into(),
                    );
                }
            }
            (PreviousObject::Directory { files }, _) => {
                let keep = journal
                    .work_dir
                    .join("preserved/dirs")
                    .join(&entry.relative_path);
                if tree_files(&keep).ok().as_ref() != Some(files) {
                    conflict(
                        &entry.relative_path,
                        "preserved directory intact",
                        "preserved directory missing or altered".into(),
                    );
                }
            }
            _ => {}
        }
    }
    for displaced in &journal.displaced_files {
        let keep = journal
            .work_dir
            .join("preserved/displaced-files")
            .join(&displaced.relative_path);
        if !file_matches(&keep, &displaced.sha256) {
            conflict(
                &displaced.relative_path,
                "preserved original file intact",
                "missing or altered".into(),
            );
        }
    }
    // Directories the restore created may contain only restored files.
    let restored: BTreeSet<&Path> = journal
        .entries
        .iter()
        .map(|entry| entry.relative_path.as_path())
        .collect();
    let created: BTreeSet<&Path> = journal
        .created_directories
        .iter()
        .map(PathBuf::as_path)
        .collect();
    for dir in &journal.created_directories {
        let path = rel(root, dir);
        match fs::read_dir(&path) {
            Ok(read) => {
                for child in read.flatten() {
                    let child_relative = dir.join(child.file_name());
                    if !restored.contains(child_relative.as_path())
                        && !created.contains(child_relative.as_path())
                    {
                        conflict(
                            &child_relative,
                            "only restored files",
                            "new file added since the restore".into(),
                        );
                    }
                }
            }
            Err(_) => conflict(dir, "created directory present", "missing".into()),
        }
    }
    conflicts
}

/// Finish or roll back a transaction left behind by a crash or by a failed
/// rollback. Idempotent; inspects real on-disk state.
pub fn recover_tree_restore(
    journal_path: &Path,
    now_unix_seconds: u64,
) -> Result<TreeRestoreJournal, TreeRestoreError> {
    recover_with_hook(journal_path, now_unix_seconds, &no_hook)
}

pub(crate) fn recover_with_hook(
    journal_path: &Path,
    now: u64,
    hook: Hook<'_>,
) -> Result<TreeRestoreJournal, TreeRestoreError> {
    let mut journal = load_tree_restore_journal(journal_path)?;
    let done = match journal.status {
        TreeRestoreStatus::Published
        | TreeRestoreStatus::RolledBack
        | TreeRestoreStatus::Undone => return Ok(journal),
        _ if journal.undo_requested => TreeRestoreStatus::Undone,
        _ => TreeRestoreStatus::RolledBack,
    };
    let cause = journal
        .detail
        .clone()
        .unwrap_or_else(|| "recovered an interrupted transaction".into());
    match roll_back(&mut journal, cause, done, hook, now) {
        TreeRestoreError::RolledBack { .. } => Ok(journal),
        other => Err(other),
    }
}

#[cfg(test)]
mod tests;
