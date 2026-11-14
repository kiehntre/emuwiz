//! Interprocess resource locks.
//!
//! Every transformation (apply, recovery, Undo) holds an exclusive `flock` on
//! a lock file per resource path it may touch, plus one on its own journal
//! directory. Locks are taken in sorted order with `LOCK_NB`, so two
//! processes attempting overlapping transformations never both proceed and
//! never deadlock: the second is refused as busy. The kernel releases locks
//! when a process dies, so a crashed holder never wedges recovery.
//!
//! Library scans do not take these locks. Concurrent catalogue writers are
//! detected by the catalogue compare-and-swap instead.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

pub const LOCK_DIR: &str = "locks";

#[derive(Debug)]
pub enum LockError {
    Busy(PathBuf),
    Io(io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::Busy(p) => write!(
                f,
                "{} is in use by another transformation; nothing was changed",
                p.display()
            ),
            LockError::Io(e) => write!(f, "lock unavailable: {e}"),
        }
    }
}

pub struct Held {
    _files: Vec<File>,
}

fn try_flock(file: &File) -> io::Result<bool> {
    // SAFETY: valid descriptor.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(false)
    } else {
        Err(error)
    }
}

/// Locks every path in `resources` (deduplicated, sorted) under
/// `<journal_root>/locks/`.
pub fn acquire(journal_root: &Path, resources: &[PathBuf]) -> Result<Held, LockError> {
    let dir = journal_root.join(LOCK_DIR);
    super::journal::ensure_root(journal_root).map_err(LockError::Io)?;
    super::journal::ensure_root(&dir).map_err(LockError::Io)?;
    let mut sorted: Vec<&PathBuf> = resources.iter().collect();
    sorted.sort();
    sorted.dedup();
    let mut files = Vec::new();
    for resource in sorted {
        let name = super::journal::hex(&Sha256::digest(resource.as_os_str().as_bytes()));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(dir.join(format!("{name}.lock")))
            .map_err(LockError::Io)?;
        if !try_flock(&file).map_err(LockError::Io)? {
            return Err(LockError::Busy(resource.clone()));
        }
        files.push(file);
    }
    Ok(Held { _files: files })
}

/// Locks one transaction's journal directory.
pub fn lock_transaction(txn_dir: &Path) -> Result<Held, LockError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(txn_dir)
        .map_err(LockError::Io)?;
    if !try_flock(&file).map_err(LockError::Io)? {
        return Err(LockError::Busy(txn_dir.to_path_buf()));
    }
    Ok(Held { _files: vec![file] })
}

/// Whether another process currently holds the transaction's lock.
pub fn transaction_busy(txn_dir: &Path) -> bool {
    matches!(lock_transaction(txn_dir), Err(LockError::Busy(_)))
}
