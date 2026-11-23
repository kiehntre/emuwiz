//! Catalogue updates for Advanced Protection renames (owner option 1,
//! 2026-11-23).
//!
//! Advanced Protection moves the files in its own recovery engine (a separate
//! process with its own journal). The library catalogue is then brought in
//! line here, file by file, with the same sealed binding Standard Apply uses
//! ([`crate::database::organisation_paths`]): the row moves only if the
//! library still records this game at the old path exactly as the preview
//! saw it, and the file now at the new path is the same object (device and
//! inode). A verified check moves with it only when the file is read in full
//! again and is byte-identical, as in Standard Apply.
//!
//! The step is safe to repeat: a row already at its new path is recognised
//! (the recovery form of the same binding) and left alone, so an interrupted
//! run is finished by running it again. A file that is not where the engine
//! put it changes nothing and is reported.
use crate::dat::rename_apply::{RenameTransaction, TransactionEntry, TransactionOperation};
use std::path::{Path, PathBuf};

/// What happened to each catalogue-backed file of one operation.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProtectedCatalogue {
    /// Library rows now at the file's new place (this run or an earlier one).
    pub updated: Vec<PathBuf>,
    /// Files still at their old place: the engine did not move them, so
    /// their library rows correctly stay where they are.
    pub not_moved: Vec<PathBuf>,
    /// Files whose library row could not be brought in line, with why.
    /// Nothing was changed for them; Check Games / Rescan resolves them.
    pub needs_review: Vec<(PathBuf, String)>,
}

impl ProtectedCatalogue {
    /// Every catalogue-backed file is consistent (updated or not moved).
    pub fn is_consistent(&self) -> bool {
        self.needs_review.is_empty()
    }

    /// Where the operation stands for the library once consistent: `None`
    /// while a file needs review; nothing moved means the step did not
    /// happen (an Apply refused, or an Undo that left the files applied).
    pub fn settled(&self, reverse: bool) -> Option<&'static str> {
        if !self.is_consistent() {
            None
        } else if self.updated.is_empty() {
            Some(if reverse { "applied" } else { "abandoned" })
        } else {
            Some(if reverse { "undone" } else { "applied" })
        }
    }
}

/// Whether any file of `transaction` has a library entry that must follow it.
pub fn involves_catalogue(transaction: &RenameTransaction) -> bool {
    transaction.entries.iter().any(bound)
}

/// The preview's catalogue bindings are still exactly current (read-only).
/// The same stale-preview check Standard Apply makes before it starts.
pub fn validate_protected_catalogue(
    transaction: &RenameTransaction,
    database: &crate::Database,
) -> Result<(), String> {
    crate::database::organisation_paths::validate_bound(transaction, database)
}

/// Holds the library catalogue's write lock while Advanced Protection moves
/// the files, so no scan or other writer can record the half-way state
/// (file moved, row not yet). Dropping it releases the lock and writes
/// nothing.
pub struct CatalogueHold {
    connection: rusqlite::Connection,
}

impl CatalogueHold {
    pub fn take(database: &Path) -> Result<Self, String> {
        let connection = crate::database::organisation_paths::open_durable_connection(database)?;
        connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| format!("the game library is busy: {e}"))?;
        Ok(Self { connection })
    }
}

impl Drop for CatalogueHold {
    fn drop(&mut self) {
        let _ = self.connection.execute_batch("ROLLBACK");
    }
}

fn bound(entry: &TransactionEntry) -> bool {
    matches!(entry.operation, TransactionOperation::RenameMove)
        && entry
            .unknown
            .contains_key(crate::database::organisation_paths::CATALOGUE_BINDING_KEY)
}

/// The previewed object is at `path` now: the engine's full identity check
/// (kind, size, device, inode, modification time to the nanosecond and a
/// full-content hash), which a rename keeps and a replacement cannot match,
/// even on a reused inode number.
fn holds(path: &Path, entry: &TransactionEntry) -> bool {
    crate::dat::rename_apply::identity::capture_identity(path).is_ok_and(|now| {
        now.kind == crate::dat::rename_apply::model::ObjectKind::RegularFile
            && crate::dat::rename_apply::identity::identity_matches(&entry.identity, &now)
    })
}

fn commit(
    entry: &TransactionEntry,
    reverse: bool,
    recovery: bool,
    checked: &[i64],
) -> Result<(), String> {
    use crate::database::organisation_paths::{begin, begin_protected};
    let change = if recovery {
        begin(entry, reverse, true)?
    } else {
        begin_protected(entry, reverse, checked)?
    };
    match change {
        Some(change) => change.commit(),
        None => Ok(()),
    }
}

/// The saved checks still current for each catalogue-backed file at the
/// place it is about to leave, by entry index. Taken (read-only) under the
/// [`CatalogueHold`] immediately before the engine moves the files, and
/// kept with the operation's record so an interrupted run carries the same.
pub fn checks_before_protected(
    transaction: &RenameTransaction,
    reverse: bool,
) -> std::collections::BTreeMap<usize, Vec<i64>> {
    transaction
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| bound(entry))
        .map(|(index, entry)| {
            (
                index,
                crate::database::organisation_paths::current_protected_checks(entry, reverse),
            )
        })
        .collect()
}

/// Brings the catalogue in line after Advanced Protection's Apply, or its
/// Undo (`reverse`), of `transaction` (the preview's own transaction, with
/// its sealed bindings).
pub fn complete_protected_catalogue(
    transaction: &RenameTransaction,
    reverse: bool,
    checked: &std::collections::BTreeMap<usize, Vec<i64>>,
) -> ProtectedCatalogue {
    let mut out = ProtectedCatalogue::default();
    for (index, entry) in transaction.entries.iter().enumerate() {
        if !bound(entry) {
            continue;
        }
        let checked = checked.get(&index).map(Vec::as_slice).unwrap_or_default();
        let (from, to) = if reverse {
            (&entry.destination_path, &entry.source_path)
        } else {
            (&entry.source_path, &entry.destination_path)
        };
        let moved = holds(to, entry);
        let stayed = holds(from, entry);
        if stayed && !moved {
            out.not_moved.push(from.clone());
            continue;
        }
        if !moved {
            out.needs_review.push((
                to.clone(),
                "the file is not at its new place as the same file".into(),
            ));
            continue;
        }
        // First as a normal move (which carries a still-valid check); if the
        // row is already at its new place from an earlier interrupted run,
        // the recovery form recognises that and changes nothing.
        match commit(entry, reverse, false, checked)
            .or_else(|_| commit(entry, reverse, true, checked))
        {
            Ok(()) => out.updated.push(to.clone()),
            Err(reason) => out.needs_review.push((to.clone(), reason)),
        }
    }
    out
}

// ------------------------------------------------------------------
// Pending records (owner 2026-11-23: a rescan between the engine's move and
// the library update must never mark the game missing and re-add it
// unverified). Each Advanced Protection operation keeps its record in the
// private workspace beside the library folder; every library scan first
// finishes pending ones from that evidence, or refuses to run.

/// Name of the private workspace folder beside a library folder.
pub const WORKSPACE_FOLDER: &str = ".emuwiz-protection";

/// One Advanced Protection operation's library changes. `state`:
/// apply_pending → applied | abandoned; applied → undo_pending → undone.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PendingRecord {
    pub version: u32,
    pub created_ns: u128,
    pub state: String,
    pub transaction: RenameTransaction,
    /// Checks current just before the engine's move, by entry index.
    #[serde(default)]
    pub checked: std::collections::BTreeMap<usize, Vec<i64>>,
}

impl PendingRecord {
    /// `Some(reverse)` while the record's library step is unfinished.
    pub fn pending(&self) -> Option<bool> {
        match self.state.as_str() {
            "apply_pending" => Some(false),
            "undo_pending" => Some(true),
            _ => None,
        }
    }
}

/// Excludes every other Advanced Protection step on one workspace while
/// held (an advisory lock on the workspace folder itself).
pub struct WorkspaceLock {
    _folder: std::fs::File,
}

impl WorkspaceLock {
    /// `Ok(None)` when another step holds it.
    pub fn try_take(workspace: &Path) -> Result<Option<Self>, String> {
        use std::os::fd::AsRawFd;
        let folder = std::fs::File::open(workspace).map_err(|e| e.to_string())?;
        // SAFETY: valid descriptor; result checked.
        if unsafe { libc::flock(folder.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = io_error();
            if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
                return Ok(None);
            }
            return Err(error.to_string());
        }
        Ok(Some(Self { _folder: folder }))
    }
}

fn io_error() -> std::io::Error {
    std::io::Error::last_os_error()
}

/// Writes a record durably (owner-only file, then an atomic rename).
pub fn write_record(path: &Path, record: &PendingRecord) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let bytes = serde_json::to_vec(record).map_err(|e| e.to_string())?;
    let parent = path.parent().ok_or("record has no folder")?;
    let name = path.file_name().ok_or("record has no name")?;
    let temporary = parent.join(format!(
        ".write-{}-{}-{}",
        name.to_string_lossy(),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    std::fs::rename(&temporary, path).map_err(|e| e.to_string())?;
    std::fs::File::open(parent)
        .and_then(|d| d.sync_all())
        .map_err(|e| e.to_string())
}

/// The workspace's records, oldest first. One that cannot be read is an
/// error (kept for review), never skipped.
pub fn records(workspace: &Path) -> Result<Vec<(PathBuf, PendingRecord)>, String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(workspace).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let named = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("catalogue-") && n.ends_with(".json"));
        if !named {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        let record: PendingRecord = serde_json::from_slice(&bytes)
            .map_err(|_| format!("{} cannot be read; it is kept for review", path.display()))?;
        out.push((path, record));
    }
    out.sort_by_key(|(_, r)| r.created_ns);
    Ok(out)
}

/// Plain lines saying what a library step did.
pub fn describe(out: &ProtectedCatalogue) -> Vec<String> {
    let mut lines = Vec::new();
    if !out.updated.is_empty() {
        lines.push(format!(
            "Library updated for {} file(s); verified checks moved with unchanged files.",
            out.updated.len()
        ));
    }
    if !out.not_moved.is_empty() {
        lines.push(format!(
            "{} file(s) were not moved, so their library entries stay as they were.",
            out.not_moved.len()
        ));
    }
    for (path, why) in &out.needs_review {
        lines.push(format!(
            "Library entry for {} was not changed ({why}).",
            path.display()
        ));
    }
    lines
}

/// Runs one record's library step and records where it now stands.
pub fn settle_record(path: &Path, record: &mut PendingRecord) -> Result<Vec<String>, String> {
    let Some(reverse) = record.pending() else {
        return Ok(Vec::new());
    };
    let out = complete_protected_catalogue(&record.transaction, reverse, &record.checked);
    if let Some(state) = out.settled(reverse) {
        record.state = state.into();
        write_record(path, record)?;
    }
    Ok(describe(&out))
}

/// Finishes every pending record of one workspace (caller holds its lock).
pub fn reconcile_workspace(workspace: &Path) -> Result<Vec<String>, String> {
    let mut notes = Vec::new();
    for (path, mut record) in records(workspace)? {
        notes.extend(settle_record(&path, &mut record)?);
    }
    Ok(notes)
}

/// An operation whose library step cannot be finished from its evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    /// The record (kept; never deleted).
    pub record: PathBuf,
    /// The game folder whose files the operation moved.
    pub folder: PathBuf,
    /// Why, in plain words.
    pub reasons: Vec<String>,
}

/// The game folder an operation worked in (its files are in one folder).
fn folder_of(record: &PendingRecord) -> Option<PathBuf> {
    record
        .transaction
        .entries
        .first()
        .and_then(|e| e.source_path.parent())
        .map(Path::to_path_buf)
}

/// Finishes what it can in one workspace (caller holds its lock) and
/// returns what is left. A record that cannot be read is left too.
pub fn unresolved_in(workspace: &Path) -> Vec<Unresolved> {
    let mut left = Vec::new();
    let Ok(entries) = std::fs::read_dir(workspace) else {
        return left;
    };
    let mut names: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("catalogue-") && n.ends_with(".json"))
        })
        .collect();
    names.sort();
    for path in names {
        let parsed = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<PendingRecord>(&b).ok());
        let Some(mut record) = parsed else {
            left.push(Unresolved {
                folder: workspace.to_path_buf(),
                reasons: vec!["its saved record cannot be read".into()],
                record: path,
            });
            continue;
        };
        if record.pending().is_none() {
            continue;
        }
        let reasons = match settle_record(&path, &mut record) {
            Ok(notes) => notes,
            Err(e) => vec![e],
        };
        if record.pending().is_some() {
            left.push(Unresolved {
                folder: folder_of(&record).unwrap_or_else(|| workspace.to_path_buf()),
                reasons,
                record: path,
            });
        }
    }
    left
}

/// The explicit way out, chosen in Advanced Protection after review: the
/// library step of this operation is given up, so the next scan treats its
/// files as moved outside EmuWiz (the old entry, with its checks, is marked
/// missing; the file is added again and Check Games verifies it again). The
/// record is kept, renamed to `reviewed-…`; nothing is deleted.
pub fn release_for_rescan(record: &Path) -> Result<PathBuf, String> {
    let name = record
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| n.starts_with("catalogue-"))
        .ok_or("not an Advanced Protection library record")?;
    let parent = record.parent().ok_or("record has no folder")?;
    let reviewed = parent.join(format!("reviewed-{name}"));
    let from =
        std::ffi::CString::new(record.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?;
    let to = std::ffi::CString::new(reviewed.as_os_str().as_encoded_bytes())
        .map_err(|e| e.to_string())?;
    // SAFETY: valid C strings; result checked. No-replace: never overwrites.
    if unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } != 0
    {
        return Err(io_error().to_string());
    }
    std::fs::File::open(parent)
        .and_then(|d| d.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(reviewed)
}

/// Before a library scan of `libraries`: every Advanced Protection
/// operation on files in them whose library step is unfinished is finished
/// now from its recorded evidence. The scan is refused (fail closed) while
/// one is in progress or cannot be finished, because it would otherwise mark
/// the game missing and add it again as a new, unverified entry. A library
/// folder switched off in Settings is not scanned, so its records do not
/// block scans of the others.
pub fn finish_pending_before_scan<'a>(
    libraries: impl IntoIterator<Item = &'a Path>,
) -> Result<(), String> {
    let libraries: Vec<&Path> = libraries.into_iter().collect();
    let concerns = |record: &PendingRecord| {
        record.transaction.entries.iter().any(|e| {
            libraries
                .iter()
                .any(|l| e.source_path.starts_with(l) || e.destination_path.starts_with(l))
        })
    };
    let mut beside_dirs = std::collections::BTreeSet::new();
    for library in &libraries {
        if let Some(parent) = library.parent() {
            beside_dirs.insert(parent.join(WORKSPACE_FOLDER));
        }
    }
    let mut problems = Vec::new();
    for beside in beside_dirs {
        let Ok(workspaces) = std::fs::read_dir(&beside) else {
            continue; // No Advanced Protection workspace beside this library.
        };
        for workspace in workspaces {
            let workspace = workspace.map_err(|e| e.to_string())?.path();
            if !std::fs::symlink_metadata(&workspace).is_ok_and(|m| m.is_dir()) {
                continue;
            }
            // Only records for the folders being scanned; one that cannot be
            // read could concern them, so it counts.
            let relevant = match records(&workspace) {
                Ok(list) => list
                    .iter()
                    .any(|(_, r)| r.pending().is_some() && concerns(r)),
                Err(_) => true,
            };
            if !relevant {
                continue;
            }
            let Some(_lock) = WorkspaceLock::try_take(&workspace)? else {
                return Err(
                    "Advanced Protection is renaming files right now. Scan again when it has finished. Nothing was scanned."
                        .into(),
                );
            };
            problems.extend(unresolved_in(&workspace));
        }
    }
    if problems.is_empty() {
        return Ok(());
    }
    let detail = problems
        .iter()
        .map(|u| format!("{}: {}", u.folder.display(), u.reasons.join(" ")))
        .collect::<Vec<_>>()
        .join("; ");
    Err(format!(
        "Library scan refused to protect verified games. An Advanced Protection rename moved files whose library entries could not be updated: {detail}. Open that folder in Advanced Protection to review it: if the files can be proved there the update finishes; otherwise choose \"Let the library rescan these files\" (their games are then re-checked with Check Games). Or switch that library folder off in Settings to scan the others. Nothing was scanned."
    ))
}
