use super::cheat_install_result::{
    CheatInstallEntrySettlement, CheatInstallOutcome, CheatInstallPath, parse_cheat_install_run,
};
use super::cheat_rollback_result::*;
use super::destination_safety::assess_destination;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub const CHEAT_ROLLBACK_RUNS_DIRECTORY_NAME: &str = "cheat-rollback-runs";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FaultPoint {
    TempWrite,
    Verification,
    Rename,
    Removal,
    /// The pending rollback journal cannot be created (fail closed).
    JournalWrite,
    /// The final rollback journal write fails.
    JournalFinalize,
    /// The process dies right after entry `n` settled (test: panic).
    CrashAfterEntry(usize),
}
#[cfg(test)]
pub(crate) fn inject_rollback_fault_for_test(point: Option<FaultPoint>) {
    ROLLBACK_FAULT.with(|f| f.set(point));
}
#[cfg(test)]
thread_local! { static ROLLBACK_FAULT: std::cell::Cell<Option<FaultPoint>> = const { std::cell::Cell::new(None) }; }
#[cfg(test)]
fn should_inject(point: FaultPoint) -> bool {
    ROLLBACK_FAULT.with(|f| f.get() == Some(point))
}
#[cfg(not(test))]
fn should_inject(_point: FaultPoint) -> bool {
    false
}

pub struct CheatRollbackOptions {
    pub journal_path: PathBuf,
    pub destination_root: PathBuf,
    pub backup_directory: PathBuf,
    pub rollback_journal_directory: PathBuf,
    pub dry_run: bool,
    pub confirmed: bool,
    pub run_id: String,
    pub started_at_unix_seconds: u64,
}
pub struct CheatRollbackRunOutcome {
    pub run: CheatRollbackRun,
    pub journal_path: Option<PathBuf>,
    pub journal_error: Option<String>,
}
fn hash_file(p: &Path) -> Result<String, String> {
    let mut f = fs::File::open(p).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut b = [0u8; 8192];
    loop {
        let n = f.read(&mut b).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        };
        h.update(&b[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{:02x}", b)).collect())
}
fn safe_relative_path(
    root: &Path,
    encoded_root: &CheatInstallPath,
    encoded: &CheatInstallPath,
) -> Result<(PathBuf, Vec<String>), String> {
    if encoded_root.lossy != CheatInstallPath::from_path(root).lossy
        || encoded_root.display != CheatInstallPath::from_path(root).display
    {
        return Err("journal destination root does not match supplied root".into());
    }
    let rel = Path::new(&encoded.display)
        .strip_prefix(Path::new(&encoded_root.display))
        .map_err(|_| "journal path is outside supplied root".to_string())?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| match c {
            Component::Normal(v) => Ok(v.to_string_lossy().into_owned()),
            _ => Err("journal path contains absolute or traversal component".to_string()),
        })
        .collect::<Result<_, _>>()?;
    if parts.len() != 2 || parts.iter().any(|p| p.is_empty()) {
        return Err(
            "journal destination must contain exactly platform and filename components".into(),
        );
    }
    let actual = root.join(&parts[0]).join(&parts[1]);
    if CheatInstallPath::from_path(&actual) != *encoded {
        return Err("journal destination disagrees with safely reconstructed destination".into());
    }
    Ok((actual, parts))
}

fn safe_backup_path(root: &Path, encoded: &CheatInstallPath) -> Result<PathBuf, String> {
    let encoded_root = CheatInstallPath::from_path(root);
    let rel = Path::new(&encoded.display)
        .strip_prefix(Path::new(&encoded_root.display))
        .map_err(|_| "backup path is outside expected backup root".to_string())?;
    let mut out = root.to_path_buf();
    for c in rel.components() {
        match c {
            Component::Normal(v) => out.push(v),
            _ => return Err("backup path contains absolute or traversal component".into()),
        }
    }
    if out == *root {
        return Err("backup path is empty".into());
    }
    if CheatInstallPath::from_path(&out) != *encoded {
        return Err("backup path cannot be reconstructed losslessly".into());
    }
    let mut cur = root.to_path_buf();
    let md = fs::symlink_metadata(&cur).map_err(|e| e.to_string())?;
    if md.file_type().is_symlink() || !md.is_dir() {
        return Err("backup root is not a plain directory".into());
    }
    for c in rel.components() {
        cur.push(c.as_os_str());
        let md = fs::symlink_metadata(&cur).map_err(|_| "backup path is missing".to_string())?;
        if md.file_type().is_symlink() {
            return Err("backup path contains symlink".into());
        }
    }
    Ok(out)
}

/// Removes temporary files an interrupted install left behind, and only
/// those: plain files whose names start with the exact prefix that run
/// recorded (`.archivefs-cheat-install-<pid>-` next to the destination,
/// `.archivefs-cheat-backup-<pid>-` in the backup directory). Anything
/// else - other runs' temp files, symlinks, directories - is left alone.
/// Returns how many files were removed.
fn sweep_interrupted_temp_files(
    prefix: Option<&str>,
    destination_parent: Option<&Path>,
    backup_directory: &Path,
) -> usize {
    let Some(prefix) = prefix else {
        return 0;
    };
    if !prefix.starts_with(".archivefs-cheat-install-") {
        return 0;
    }
    let backup_prefix = prefix.replacen(".archivefs-cheat-install-", ".archivefs-cheat-backup-", 1);
    let mut removed = 0;
    for (directory, prefix) in [
        (destination_parent, prefix),
        (Some(backup_directory), backup_prefix.as_str()),
    ] {
        let Some(directory) = directory else { continue };
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.starts_with(prefix) {
                continue;
            }
            let path = entry.path();
            let Ok(md) = fs::symlink_metadata(&path) else {
                continue;
            };
            if md.file_type().is_symlink() || !md.is_file() {
                continue;
            }
            if fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// The identity half of one rollback entry (what the install journal
/// says about it), with outcome `pending` - the state every entry has in
/// the pending rollback journal written before the first mutation.
fn planned_entry(
    index: usize,
    e: &super::cheat_install_result::CheatInstallEntryResult,
    orig: &super::cheat_install_result::CheatInstallRun,
) -> (CheatRollbackEntryResult, CheatInstallEntrySettlement, bool) {
    let dest = e.destination_path.clone();
    // An interrupted (`in_progress`) journal is read exactly as the
    // installer wrote it: settled entries are facts, the one in-flight
    // entry is resolved from the destination's current content, and
    // entries after it were never attempted.
    let settlement = orig.entry_settlement(index);
    let actionable = matches!(
        e.outcome,
        CheatInstallOutcome::InstalledNew | CheatInstallOutcome::ReplacedWithBackup
    );
    let in_flight = settlement == CheatInstallEntrySettlement::InFlight && actionable;
    let r = CheatRollbackEntryResult {
        original_outcome: e.outcome,
        destination_path: dest.clone(),
        expected_installed_hash: e
            .resulting_destination_hash
            .clone()
            .or(e.expected_source_hash.clone()),
        expected_previous_hash: e.previous_destination_hash.clone(),
        observed_current_hash: None,
        backup_path: if in_flight {
            e.backup_path.clone().or(e.planned_backup_path.clone())
        } else {
            e.backup_path.clone()
        },
        outcome: CheatRollbackOutcome::Pending,
        wrote: false,
        error_code: None,
        message: "not reached yet".into(),
        retryable: false,
    };
    (r, settlement, in_flight)
}

/// Rolls back one entry. Every destination mutation is atomic with its
/// own content check (see [`remove_verified`] and [`restore_verified`]):
/// a file rewritten by anything else between the earlier hash check and
/// the mutation is never clobbered or deleted.
fn rollback_one_entry(
    index: usize,
    e: &super::cheat_install_result::CheatInstallEntryResult,
    orig: &super::cheat_install_result::CheatInstallRun,
    options: &CheatRollbackOptions,
    effective_dry: bool,
    mut r: CheatRollbackEntryResult,
    settlement: CheatInstallEntrySettlement,
    in_flight: bool,
) -> CheatRollbackEntryResult {
    let dest = r.destination_path.clone();
    let actionable = matches!(
        e.outcome,
        CheatInstallOutcome::InstalledNew | CheatInstallOutcome::ReplacedWithBackup
    );
    r.outcome = CheatRollbackOutcome::NoChangeRequired;
    r.message.clear();
    if settlement == CheatInstallEntrySettlement::NotStarted {
        r.message = "install run was interrupted before this entry was attempted".into();
        return r;
    }
    if !in_flight && (!e.applied || !actionable) {
        r.message = "original install made no change".into();
        return r;
    }
    let Some(cp) = dest else {
        r.outcome = CheatRollbackOutcome::FailedInvalidJournal;
        r.message = "missing destination".into();
        return r;
    };
    let Some(journal_root) = orig.destination_root.as_ref() else {
        r.outcome = CheatRollbackOutcome::FailedInvalidJournal;
        r.message = "install journal has no destination root".into();
        return r;
    };
    if r.expected_installed_hash.is_none() {
        r.outcome = CheatRollbackOutcome::FailedInvalidJournal;
        r.message = if in_flight {
            "interrupted install entry has no expected source hash".into()
        } else {
            "applied install entry has no resulting destination hash".into()
        };
        return r;
    }
    let (p, parts) = match safe_relative_path(&options.destination_root, journal_root, &cp) {
        Ok(v) => v,
        Err(msg) => {
            r.outcome = CheatRollbackOutcome::FailedUnsafeDestination;
            r.error_code = Some("unsafe_destination".into());
            r.message = msg;
            return r;
        }
    };
    if assess_destination(
        &options.destination_root,
        std::ffi::OsStr::new(&parts[0]),
        std::ffi::OsStr::new(&parts[1]),
    )
    .is_err()
    {
        r.outcome = CheatRollbackOutcome::FailedUnsafeDestination;
        r.error_code = Some("unsafe_destination".into());
        return r;
    }
    let md = match fs::symlink_metadata(&p) {
        Ok(m) => Some(m),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            r.outcome = CheatRollbackOutcome::FailedIo;
            r.message = e.to_string();
            return r;
        }
    };
    if md
        .as_ref()
        .is_some_and(|m| m.file_type().is_symlink() || m.is_dir())
    {
        r.outcome = CheatRollbackOutcome::FailedUnsafeDestination;
        return r;
    }
    let cur = if md.is_some() {
        hash_file(&p).ok()
    } else {
        None
    };
    r.observed_current_hash = cur.clone();
    if in_flight {
        if !effective_dry {
            let swept = sweep_interrupted_temp_files(
                orig.temp_file_prefix.as_deref(),
                p.parent(),
                &options.backup_directory,
            );
            if swept > 0 {
                r.message =
                    format!("removed {swept} temporary file(s) left by the interrupted install; ");
            }
        }
        let applied_now = cur.is_some() && cur.as_deref() == r.expected_installed_hash.as_deref();
        if !applied_now {
            let untouched = match e.outcome {
                CheatInstallOutcome::InstalledNew => md.is_none(),
                _ => cur.is_some() && cur.as_deref() == e.previous_destination_hash.as_deref(),
            };
            if untouched {
                r.message.push_str(
                    "install was interrupted before this entry was published; \
                     destination untouched",
                );
            } else {
                r.outcome = CheatRollbackOutcome::FailedDestinationChanged;
                r.message.push_str(
                    "install was interrupted while this entry was in flight and the \
                     destination now matches neither the catalogue content nor the \
                     pre-install content; left unchanged",
                );
            }
            return r;
        }
        r.message.push_str(
            "install was interrupted after this entry was published but before it was \
             recorded; resolved from the destination's current content. ",
        );
    }
    if e.outcome == CheatInstallOutcome::InstalledNew {
        if md.is_none() {
            r.outcome = CheatRollbackOutcome::AlreadyRestored;
        } else if cur.as_deref() != r.expected_installed_hash.as_deref() {
            r.outcome = CheatRollbackOutcome::FailedDestinationChanged;
            r.message = "destination changed".into();
        } else if effective_dry {
            r.outcome = CheatRollbackOutcome::WouldRemoveInstalledFile;
        } else if should_inject(FaultPoint::Removal) {
            r.outcome = CheatRollbackOutcome::FailedIo;
            r.error_code = Some("injected_removal_failure".into());
        } else {
            let expected = r.expected_installed_hash.clone().unwrap_or_default();
            let parked = p.parent().unwrap_or(Path::new(".")).join(format!(
                ".archivefs-rollback-{}-{}-{index}.removed",
                options.run_id,
                std::process::id()
            ));
            match remove_verified(&p, &expected, &parked) {
                Ok(()) => {
                    r.outcome = CheatRollbackOutcome::RemovedInstalledFile;
                    r.wrote = true
                }
                Err(MutationError::DestinationChanged(message)) => {
                    r.outcome = CheatRollbackOutcome::FailedDestinationChanged;
                    r.message = message;
                }
                Err(MutationError::Io(message) | MutationError::Verification(message)) => {
                    r.outcome = CheatRollbackOutcome::FailedIo;
                    r.message = message;
                }
            }
        }
    } else {
        let Some(bp_enc) = r.backup_path.clone() else {
            r.outcome = CheatRollbackOutcome::FailedBackupMissing;
            return r;
        };
        let bp = match safe_backup_path(&options.backup_directory, &bp_enc) {
            Ok(p) => p,
            Err(msg) => {
                r.outcome = CheatRollbackOutcome::FailedUnsafeBackupPath;
                r.message = msg;
                return r;
            }
        };
        let bmd = fs::symlink_metadata(&bp).ok();
        if bmd.is_none() {
            r.outcome = CheatRollbackOutcome::FailedBackupMissing;
            return r;
        }
        if bmd
            .as_ref()
            .is_some_and(|m| m.file_type().is_symlink() || !m.is_file())
        {
            r.outcome = CheatRollbackOutcome::FailedUnsafeBackupPath;
            return r;
        }
        let bh = hash_file(&bp).ok();
        if bh.as_deref() != e.previous_destination_hash.as_deref() {
            r.outcome = CheatRollbackOutcome::FailedBackupChanged;
            return r;
        }
        if cur.as_deref() == e.previous_destination_hash.as_deref() {
            r.outcome = CheatRollbackOutcome::AlreadyRestored;
        } else if cur.as_deref() != r.expected_installed_hash.as_deref() {
            r.outcome = CheatRollbackOutcome::FailedDestinationChanged;
        } else if effective_dry {
            r.outcome = CheatRollbackOutcome::WouldRestoreBackup;
        } else {
            let parent = p.parent().unwrap_or(Path::new("."));
            let tmp = parent.join(format!(
                ".archivefs-rollback-{}-{}",
                options.run_id,
                std::process::id()
            ));
            let expected_installed = r.expected_installed_hash.clone().unwrap_or_default();
            let result = (|| -> Result<(), MutationError> {
                if should_inject(FaultPoint::TempWrite) {
                    return Err(MutationError::Io(
                        "injected temporary restore write failure".into(),
                    ));
                }
                let bytes = fs::read(&bp).map_err(|e| MutationError::Io(e.to_string()))?;
                let mut f = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&tmp)
                    .map_err(|e| MutationError::Io(e.to_string()))?;
                std::io::Write::write_all(&mut f, &bytes)
                    .map_err(|e| MutationError::Io(e.to_string()))?;
                f.sync_all().map_err(|e| MutationError::Io(e.to_string()))?;
                if should_inject(FaultPoint::Verification) {
                    return Err(MutationError::Verification(
                        "injected restore verification failure".into(),
                    ));
                }
                if hash_file(&tmp).ok().as_deref() != e.previous_destination_hash.as_deref() {
                    return Err(MutationError::Verification(
                        "restore verification hash mismatch".into(),
                    ));
                }
                if should_inject(FaultPoint::Rename) {
                    return Err(MutationError::Io("injected restore rename failure".into()));
                }
                restore_verified(&tmp, &p, &expected_installed)?;
                let _ = fs::File::open(parent).and_then(|f| f.sync_all());
                Ok(())
            })();
            match result {
                Ok(_) => {
                    r.outcome = CheatRollbackOutcome::RestoredBackup;
                    r.wrote = true
                }
                Err(err) => {
                    if !matches!(err, MutationError::DestinationChanged(_)) {
                        let _ = fs::remove_file(&tmp);
                    }
                    let (outcome, message) = match err {
                        MutationError::Verification(m) => {
                            (CheatRollbackOutcome::FailedVerification, m)
                        }
                        MutationError::DestinationChanged(m) => {
                            (CheatRollbackOutcome::FailedDestinationChanged, m)
                        }
                        MutationError::Io(m) => (CheatRollbackOutcome::FailedIo, m),
                    };
                    r.outcome = outcome;
                    r.message = message;
                }
            }
        }
    }
    r
}

enum MutationError {
    Verification(String),
    DestinationChanged(String),
    Io(String),
}

#[cfg(test)]
thread_local! {
    static BEFORE_MUTATION_HOOK: std::cell::RefCell<Option<Box<dyn Fn(&Path)>>> =
        const { std::cell::RefCell::new(None) };
}

/// Test-only: runs `hook` with the destination path immediately before each
/// destination mutation (after the earlier hash check), modelling RetroArch
/// saving cheat edits inside that window.
#[cfg(test)]
pub(crate) fn set_before_destination_mutation_hook_for_test(hook: Option<Box<dyn Fn(&Path)>>) {
    BEFORE_MUTATION_HOOK.with(|cell| *cell.borrow_mut() = hook);
}

fn run_before_mutation_hook(path: &Path) {
    #[cfg(test)]
    BEFORE_MUTATION_HOOK.with(|cell| {
        if let Some(hook) = cell.borrow().as_ref() {
            hook(path);
        }
    });
    #[cfg(not(test))]
    let _ = path;
}

/// Removes `destination` only if it still holds `expected_hash`, with no
/// window in which a newer file can be deleted: the file is first moved
/// aside atomically (`RENAME_NOREPLACE` into `parked`), hashed there, and
/// only then unlinked. Content that no longer matches is moved back (or,
/// if its name was reused meanwhile, kept at `parked` and reported).
fn remove_verified(
    destination: &Path,
    expected_hash: &str,
    parked: &Path,
) -> Result<(), MutationError> {
    run_before_mutation_hook(destination);
    super::cheat_installer::rename_no_replace(destination, parked)
        .map_err(|e| MutationError::Io(format!("could not move the file aside: {e}")))?;
    match hash_file(parked) {
        Ok(hash) if hash == expected_hash => {
            fs::remove_file(parked).map_err(|e| MutationError::Io(e.to_string()))
        }
        _ => match super::cheat_installer::rename_no_replace(parked, destination) {
            Ok(()) => Err(MutationError::DestinationChanged(
                "destination changed after the hash check; left in place".into(),
            )),
            Err(_) => Err(MutationError::DestinationChanged(format!(
                "destination changed after the hash check and its name was reused; the newer content is preserved at {}",
                parked.display()
            ))),
        },
    }
}

/// Publishes the verified restore file `tmp` at `destination` only if the
/// destination still holds `expected_installed_hash`. On Linux the two
/// names are exchanged atomically and the displaced content is hashed
/// afterwards (swapped back if it is not the installed cheat), so a
/// concurrent rewrite is never lost; where exchange is unsupported the
/// destination is re-hashed immediately before a plain rename.
fn restore_verified(
    tmp: &Path,
    destination: &Path,
    expected_installed_hash: &str,
) -> Result<(), MutationError> {
    run_before_mutation_hook(destination);
    match super::cheat_installer::rename_exchange(tmp, destination) {
        Ok(()) => {
            // `tmp` now holds whatever was at the destination.
            match hash_file(tmp) {
                Ok(hash) if hash == expected_installed_hash => {
                    let _ = fs::remove_file(tmp);
                    Ok(())
                }
                _ => match super::cheat_installer::rename_exchange(tmp, destination) {
                    Ok(()) => {
                        let _ = fs::remove_file(tmp);
                        Err(MutationError::DestinationChanged(
                            "destination changed after the hash check; left unchanged".into(),
                        ))
                    }
                    Err(e) => Err(MutationError::DestinationChanged(format!(
                        "destination changed after the hash check and could not be swapped back ({e}); the newer content is preserved at {}",
                        tmp.display()
                    ))),
                },
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::Unsupported => {
            if hash_file(destination).ok().as_deref() != Some(expected_installed_hash) {
                let _ = fs::remove_file(tmp);
                return Err(MutationError::DestinationChanged(
                    "destination changed after the hash check; left unchanged".into(),
                ));
            }
            fs::rename(tmp, destination).map_err(|e| MutationError::Io(e.to_string()))
        }
        Err(e) => Err(MutationError::Io(e.to_string())),
    }
}

fn write_rollback_journal_temp(run: &CheatRollbackRun, dir: &Path) -> Result<PathBuf, String> {
    let json = serde_json::to_string_pretty(run).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!(
        ".archivefs-rollback-journal-{}-{}",
        std::process::id(),
        run.run_id
    ));
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| e.to_string())?;
    std::io::Write::write_all(&mut f, json.as_bytes()).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())?;
    Ok(tmp)
}

/// Creates the pending rollback journal at its final path, never
/// clobbering an existing one.
fn create_pending_rollback_journal(
    run: &CheatRollbackRun,
    dir: &Path,
    path: &Path,
) -> Result<(), String> {
    if should_inject(FaultPoint::JournalWrite) {
        return Err("injected rollback journal write failure".into());
    }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    if fs::symlink_metadata(path).is_ok() {
        return Err("rollback journal already exists".into());
    }
    let tmp = write_rollback_journal_temp(run, dir)?;
    if let Err(e) = super::cheat_installer::rename_no_replace(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    let _ = fs::File::open(dir).and_then(|f| f.sync_all());
    Ok(())
}

/// Atomically replaces this run's own rollback journal with a newer state.
fn rewrite_rollback_journal(run: &CheatRollbackRun, dir: &Path, path: &Path) -> Result<(), String> {
    let tmp = write_rollback_journal_temp(run, dir)?;
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    let _ = fs::File::open(dir).and_then(|f| f.sync_all());
    Ok(())
}

/// Rolls back one install journal. For a real (confirmed, non-dry) run the
/// rollback journal is written *before* the first mutation (every entry
/// `pending`, status `in_progress`), rewritten after each entry, and
/// finalised once - so a rollback killed mid-way leaves a record of what
/// it had done, and a rollback whose record cannot be created changes
/// nothing. Dry runs write no journal.
pub fn execute_cheat_rollback(
    journal_path: &Path,
    options: &CheatRollbackOptions,
) -> CheatRollbackRunOutcome {
    let txt = match fs::read_to_string(journal_path) {
        Ok(t) => t,
        Err(e) => {
            return failed_run(journal_path, options, format!("{e}"));
        }
    };
    let orig = match parse_cheat_install_run(&txt) {
        Ok(r) => r,
        Err(e) => {
            return failed_run(journal_path, options, e.to_string());
        }
    };
    let effective_dry = options.dry_run || !options.confirmed;
    let planned: Vec<_> = orig
        .entries
        .iter()
        .enumerate()
        .map(|(index, e)| planned_entry(index, e, &orig))
        .collect();
    let entries: Vec<CheatRollbackEntryResult> =
        planned.iter().map(|(r, _, _)| r.clone()).collect();
    let summary = CheatRollbackSummary::from_entries(&entries);
    let mut run = CheatRollbackRun {
        schema_version: CHEAT_ROLLBACK_RUN_SCHEMA_VERSION,
        run_id: options.run_id.clone(),
        original_install_run_id: orig.run_id.clone(),
        original_journal_path: CheatInstallPath::from_path(journal_path),
        started_at_unix_seconds: options.started_at_unix_seconds,
        completed_at_unix_seconds: None,
        dry_run: effective_dry,
        confirmed: options.confirmed,
        destination_root: CheatInstallPath::from_path(&options.destination_root),
        entries,
        summary,
        status: if effective_dry {
            CheatRollbackRunStatus::DryRun
        } else {
            CheatRollbackRunStatus::InProgress
        },
        rollback_journal_path: None,
        journal_write_error: None,
    };
    let dir = &options.rollback_journal_directory;
    let path = dir.join(format!("{}.json", options.run_id));
    let mut journal_error: Option<String> = None;
    if !effective_dry {
        run.rollback_journal_path = Some(CheatInstallPath::from_path(&path));
        if let Err(error) = create_pending_rollback_journal(&run, dir, &path) {
            // Fail closed: no rollback record, no mutation.
            for r in &mut run.entries {
                r.outcome = CheatRollbackOutcome::FailedIo;
                r.error_code = Some("journal_unavailable".into());
                r.message = "not attempted: the rollback record could not be created before the first change, so nothing was changed".into();
            }
            finalize_run(&mut run, effective_dry);
            run.rollback_journal_path = None;
            run.journal_write_error = Some(error.clone());
            return CheatRollbackRunOutcome {
                run,
                journal_path: None,
                journal_error: Some(format!(
                    "nothing was changed: the rollback record could not be created: {error}"
                )),
            };
        }
    }
    for (index, (e, (r, settlement, in_flight))) in orig.entries.iter().zip(planned).enumerate() {
        if let Some(error) = &journal_error {
            let entry = &mut run.entries[index];
            entry.outcome = CheatRollbackOutcome::FailedIo;
            entry.error_code = Some("journal_update_failed".into());
            entry.message =
                format!("not attempted: an earlier rollback-record update failed ({error})");
            continue;
        }
        run.entries[index] = rollback_one_entry(
            index,
            e,
            &orig,
            options,
            effective_dry,
            r,
            settlement,
            in_flight,
        );
        if !effective_dry {
            run.summary = CheatRollbackSummary::from_entries(&run.entries);
            if let Err(error) = rewrite_rollback_journal(&run, dir, &path) {
                journal_error = Some(format!(
                    "rollback record could not be updated after entry {index}: {error}"
                ));
            }
        }
        if should_inject(FaultPoint::CrashAfterEntry(index)) {
            panic!("injected crash after rollback entry {index} was recorded");
        }
    }
    finalize_run(&mut run, effective_dry);
    if effective_dry {
        return CheatRollbackRunOutcome {
            run,
            journal_path: None,
            journal_error: None,
        };
    }
    let finalize = if should_inject(FaultPoint::JournalFinalize) {
        Err("injected rollback journal finalize failure".to_string())
    } else {
        rewrite_rollback_journal(&run, dir, &path)
    };
    match finalize {
        Ok(()) => {
            run.journal_write_error = journal_error.clone();
            CheatRollbackRunOutcome {
                run,
                journal_path: Some(path),
                journal_error,
            }
        }
        Err(error) => {
            let message = match journal_error {
                Some(earlier) => {
                    format!("{earlier}; final rollback-record write also failed: {error}")
                }
                None => format!(
                    "the rollback was applied but its record could not be finalised (it still reads in_progress): {error}"
                ),
            };
            run.journal_write_error = Some(message.clone());
            CheatRollbackRunOutcome {
                run,
                journal_path: None,
                journal_error: Some(message),
            }
        }
    }
}

fn finalize_run(run: &mut CheatRollbackRun, effective_dry: bool) {
    run.summary = CheatRollbackSummary::from_entries(&run.entries);
    run.status = CheatRollbackRunStatus::derive(&run.summary, effective_dry);
    run.completed_at_unix_seconds = Some(run.started_at_unix_seconds);
}

pub fn execute_cheat_rollback_run(
    journal_path: &Path,
    options: &CheatRollbackOptions,
) -> CheatRollbackRunOutcome {
    execute_cheat_rollback(journal_path, options)
}
fn failed_run(path: &Path, o: &CheatRollbackOptions, msg: String) -> CheatRollbackRunOutcome {
    let e = CheatRollbackEntryResult {
        original_outcome: CheatInstallOutcome::FailedWrite,
        destination_path: None,
        expected_installed_hash: None,
        expected_previous_hash: None,
        observed_current_hash: None,
        backup_path: None,
        outcome: CheatRollbackOutcome::FailedInvalidJournal,
        wrote: false,
        error_code: Some("invalid_journal".into()),
        message: msg,
        retryable: false,
    };
    let s = CheatRollbackSummary::from_entries(std::slice::from_ref(&e));
    let dry_run = o.dry_run || !o.confirmed;
    let run = CheatRollbackRun {
        schema_version: 1,
        run_id: o.run_id.clone(),
        original_install_run_id: String::new(),
        original_journal_path: CheatInstallPath::from_path(path),
        started_at_unix_seconds: o.started_at_unix_seconds,
        completed_at_unix_seconds: Some(o.started_at_unix_seconds),
        dry_run,
        confirmed: o.confirmed,
        destination_root: CheatInstallPath::from_path(&o.destination_root),
        entries: vec![e],
        summary: s,
        status: CheatRollbackRunStatus::derive(&s, dry_run),
        rollback_journal_path: None,
        journal_write_error: None,
    };
    CheatRollbackRunOutcome {
        run,
        journal_path: None,
        journal_error: None,
    }
}
