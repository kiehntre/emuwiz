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
//! | [`session`] | Play-session journal: attempt, started, ended, what changed |
//! | [`workflow`] | The steps the CLI and Blue share |
//! | [`device`] | Offline two-device comparison and resumable transfer (no network code) |
//!
//! PlayStation memory cards are read, exported and imported only through
//! [`crate::memory_card_manager`]; this module adds no second card reader.

pub mod compat;
pub mod device;
pub mod discover;
pub mod model;
pub mod session;
pub mod status;
pub mod store;
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
