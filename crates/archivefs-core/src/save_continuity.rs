//! Save continuity: find the saves on this machine, say which game each one
//! belongs to and how that is known, keep content-verified snapshots of them,
//! tie snapshots to play sessions, and plan a safe way to continue on another
//! emulator or another device.
//!
//! Everything here follows three rules:
//!
//! - **The original save is only ever read.** Snapshots are new objects in
//!   EmuWiz's own store; a recovery writes a new folder; a continuation copy
//!   is a new file. Nothing in this module can overwrite or delete a save.
//! - **A claim is only as strong as its evidence.** A save is tied to a game
//!   by what is written inside it (a product code in a memory-card entry, a
//!   GameCube header, a `PARAM.SFO`), or failing that only "by the emulator's
//!   naming rule", or failing that not at all. A file name is never proof.
//!   Newer or larger never means "more progress".
//! - **Nothing is deleted or synchronised automatically.** Pruning and device
//!   transfer produce proposals for a person to read.
//!
//! The pieces:
//!
//! | Module | Role |
//! |---|---|
//! | [`model`] | The typed record for one discovered save and its evidence |
//! | [`discover`] | Read-only, bounded, cancellable discovery (reuses `memory_card_manager`) |
//! | [`compat`] | Five-level compatibility verdicts and conversion plans |
//! | [`store`] | Content-addressed snapshot store: capture, verify, recover, pruning proposals |
//! | [`status`] | Capture safety, structure, game integrity and last-exit context, kept separate |
//! | [`crash_evidence`] | Read-only lookup of the system's own record that a process was ended by a fatal signal |
//! | [`session`] | Play-session journal: attempt, started, ended, what changed |
//! | [`workflow`] | The steps the CLI and Blue share |
//! | [`device`] | Offline two-device comparison and resumable transfer (no network code) |
//!
//! PlayStation memory cards are read, exported and imported only through
//! [`crate::memory_card_manager`]; this module adds no second card reader.

pub mod compat;
pub mod crash_evidence;
pub mod device;
pub mod diagnostics;
pub mod discover;
pub mod installation;
pub mod model;
pub mod session;
pub mod status;
pub mod store;
pub mod tracker;
pub mod workflow;

#[cfg(test)]
mod tests;

use std::fmt;
use std::path::PathBuf;

/// `<data dir>/save-continuity`: snapshots, sessions and this device's identity.
pub fn default_root() -> crate::Result<PathBuf> {
    Ok(crate::app_dirs::data_dir()?.join("save-continuity"))
}

/// Why an operation was refused or could not complete. Every variant is a
/// stop: nothing partial is left published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContinuityError {
    /// The request itself is not allowed (a link, a path outside its root, an
    /// existing destination...). The text says which rule.
    Refused(String),
    /// The emulator is running, or some program has the save open.
    InUse(String),
    /// Whether the emulator is closed could not be established.
    ClosureUnproven(String),
    /// The save changed while it was being read.
    Unstable(String),
    /// Stored data does not match its recorded SHA-256.
    Corrupt(String),
    NotFound(String),
    /// Not enough free space, or the store's size limit would be exceeded.
    NoSpace(String),
    Cancelled,
    Io(String),
}

impl fmt::Display for ContinuityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(text) => write!(f, "refused: {text}"),
            Self::InUse(text) => write!(f, "in use: {text}"),
            Self::ClosureUnproven(text) => write!(f, "not proven closed: {text}"),
            Self::Unstable(text) => write!(f, "changed while being read: {text}"),
            Self::Corrupt(text) => write!(f, "does not match its checksum: {text}"),
            Self::NotFound(text) => write!(f, "not found: {text}"),
            Self::NoSpace(text) => write!(f, "not enough space: {text}"),
            Self::Cancelled => f.write_str("cancelled"),
            Self::Io(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for ContinuityError {}

pub(crate) fn io(context: &str, path: &std::path::Path, error: std::io::Error) -> ContinuityError {
    ContinuityError::Io(format!("{context} {}: {error}", path.display()))
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub(crate) fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

static STATE_WORK_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Replaces one of EmuWiz's own small state files (a session record, the
/// discovery cache, the device or pairing file) with new content, safely:
/// the new content goes into a work file that is created here and only here
/// (never an existing file, never through a link), is flushed, takes the
/// place of the old file in one step, and the folder is flushed so the
/// change survives a power cut. If creating the work file fails, nothing is
/// removed. Only ever used on files inside the save-continuity folder.
pub(crate) fn write_state_file(
    path: &std::path::Path,
    bytes: &[u8],
) -> Result<(), ContinuityError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let parent = path
        .parent()
        .ok_or_else(|| ContinuityError::Refused("a state file needs a folder".into()))?;
    let name = path
        .file_name()
        .ok_or_else(|| ContinuityError::Refused("a state file needs a name".into()))?
        .to_string_lossy()
        .into_owned();
    let work = parent.join(format!(
        ".{name}.{}-{}.part",
        std::process::id(),
        STATE_WORK_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&work)
        .map_err(|error| io("cannot create", &work, error))?;
    let written = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| std::fs::rename(&work, path))
        .and_then(|()| std::fs::File::open(parent)?.sync_all());
    if let Err(error) = written {
        // The work file was created by this call; removing it is safe. After
        // a successful rename it no longer exists under this name.
        let _ = std::fs::remove_file(&work);
        return Err(io("cannot write", path, error));
    }
    Ok(())
}

/// One fingerprint for a save made of several files: each file's name, size
/// and SHA-256, in name order. Discovery and snapshots compute it the same
/// way, so a folder that changed between the two is noticed.
pub(crate) fn folder_fingerprint<'a>(
    files: impl Iterator<Item = (&'a str, u64, &'a str)>,
) -> String {
    let mut text = Vec::new();
    for (name, size, sha256) in files {
        text.extend_from_slice(name.as_bytes());
        text.push(0);
        text.extend_from_slice(&size.to_le_bytes());
        text.extend_from_slice(sha256.as_bytes());
    }
    sha256_hex(&text)
}
