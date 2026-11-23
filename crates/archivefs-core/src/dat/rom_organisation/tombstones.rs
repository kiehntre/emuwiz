//! Linked-library Undo leftovers (owner item 4, 2026-11-23).
//!
//! Undo of a created link never unlinks it by name: it renames the proved
//! link to a hidden tombstone (`.emuwiz-undone-<dev>-<ino>`), because an
//! unlink by name could remove an object swapped in after the check. Once the
//! whole transaction is rolled back, each recorded tombstone is retired here
//! without reintroducing that race: it is moved (no-replace) into a fresh
//! owner-only quarantine folder that no other user can enter, proved there
//! again (same link object, same exact target text), and only then unlinked.
//! Anything that does not prove is left exactly where it is (or put back) and
//! reported. Only the tombstone this transaction's journal recorded is ever
//! touched; nothing else in the folder is read or changed.

use crate::dat::rename_apply::{
    EntryState, RenameTransaction, TransactionEntry, TransactionOperation,
};
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

const RECORD: &str = "emuwiz_link_undo_tombstone_v1";

/// Tombstones retired, and those left in place with the reason.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TombstoneCleanup {
    pub removed: Vec<PathBuf>,
    pub kept: Vec<(PathBuf, String)>,
}

/// Retires the recorded tombstones of a fully rolled-back transaction.
pub fn retire_link_tombstones(transaction: &RenameTransaction) -> TombstoneCleanup {
    let mut out = TombstoneCleanup::default();
    if !transaction.is_fully_rolled_back() {
        return out;
    }
    for entry in &transaction.entries {
        let Some(recorded) = entry.unknown.get(RECORD).and_then(|v| v.as_str()) else {
            continue;
        };
        let recorded = PathBuf::from(recorded);
        match retire(entry, &recorded) {
            Ok(true) => out.removed.push(recorded),
            Ok(false) => {}
            Err(reason) => out.kept.push((recorded, reason)),
        }
    }
    out
}

fn c(name: &std::ffi::OsStr) -> Result<CString, String> {
    CString::new(name.as_bytes()).map_err(|_| "unusable name".to_string())
}

/// `Ok(true)` removed, `Ok(false)` already gone, `Err` kept (with why).
fn retire(entry: &TransactionEntry, recorded: &Path) -> Result<bool, String> {
    let TransactionOperation::CreateSymlink {
        expected_target,
        destination_root,
    } = &entry.operation
    else {
        return Err("not a created link".into());
    };
    if entry.state != EntryState::RolledBack {
        return Err("the link's Undo is not complete".into());
    }
    let proof = entry
        .created_link_identity()
        .ok_or("the journal has no proof of the created link")?;
    let expected = crate::dat::rename_apply::dirsync::link_undo_tombstone_path(
        &entry.destination_path,
        &proof,
    );
    if expected != recorded {
        return Err("the recorded tombstone name does not match the journal proof".into());
    }
    let folder = recorded.parent().ok_or("no folder")?;
    // The folder must be the real one, inside this operation's own root.
    if std::fs::canonicalize(folder).ok().as_deref() != Some(folder)
        || !folder.starts_with(destination_root)
    {
        return Err("the tombstone's folder is not this operation's real folder".into());
    }
    let name = c(recorded.file_name().ok_or("no name")?)?;
    let folder_c = c(folder.as_os_str())?;
    // SAFETY: valid C string; result checked.
    let fd = unsafe {
        libc::open(
            folder_c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(format!(
            "folder cannot be opened: {}",
            io::Error::last_os_error()
        ));
    }
    // SAFETY: fd is a fresh, owned descriptor.
    let dir = unsafe { std::fs::File::from_raw_fd(fd) };
    // The same proof the Undo itself used (kind, size, device, inode and the
    // link's own modification time to the nanosecond, which survives renames
    // but differs for a link made later, even on a reused inode number), plus
    // the exact target text. `at` is the object's path; `dirfd`/`name` read it.
    let proved = |at: &Path, dirfd: libc::c_int, name: &CString| -> Result<bool, String> {
        let actual = match crate::dat::rename_apply::identity::capture_identity(at) {
            Ok(actual) => actual,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.to_string()),
        };
        if !crate::dat::rename_apply::identity::link_identity_matches(&proof, &actual) {
            return Err("a different object has the tombstone's name".into());
        }
        let mut buf = vec![0u8; 4096];
        // SAFETY: buffer length passed; result checked.
        let n =
            unsafe { libc::readlinkat(dirfd, name.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 || n as usize >= buf.len() {
            return Err("the tombstone's target cannot be read".into());
        }
        if std::ffi::OsStr::from_bytes(&buf[..n as usize]) != expected_target.as_os_str() {
            return Err("the tombstone points somewhere else".into());
        }
        Ok(true)
    };
    if !proved(recorded, dir.as_raw_fd(), &name)? {
        return Ok(false);
    }
    // A fresh owner-only quarantine on the same filesystem.
    let mut bytes = [0u8; 16];
    std::io::Read::read_exact(
        &mut std::fs::File::open("/dev/urandom").map_err(|e| e.to_string())?,
        &mut bytes,
    )
    .map_err(|e| e.to_string())?;
    let qname = CString::new(format!(
        ".emuwiz-undo-q-{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
    .expect("hex");
    // SAFETY: valid descriptor and name.
    if unsafe { libc::mkdirat(dir.as_raw_fd(), qname.as_ptr(), 0o700) } < 0 {
        return Err(format!("quarantine folder: {}", io::Error::last_os_error()));
    }
    let remove_q = || {
        // SAFETY: valid descriptor and name.
        unsafe { libc::unlinkat(dir.as_raw_fd(), qname.as_ptr(), libc::AT_REMOVEDIR) }
    };
    // SAFETY: valid descriptor and name; result checked.
    let qfd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            qname.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if qfd < 0 {
        let e = io::Error::last_os_error();
        remove_q();
        return Err(format!("quarantine folder: {e}"));
    }
    // SAFETY: fresh owned descriptor.
    let q = unsafe { std::fs::File::from_raw_fd(qfd) };
    {
        use std::os::unix::fs::MetadataExt;
        let m = q.metadata().map_err(|e| e.to_string())?;
        // SAFETY: plain syscall.
        if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
            remove_q();
            return Err("the quarantine folder is not private".into());
        }
    }
    let held = CString::new("link").expect("literal");
    // SAFETY: both descriptors and names valid.
    if unsafe {
        libc::renameat2(
            dir.as_raw_fd(),
            name.as_ptr(),
            q.as_raw_fd(),
            held.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } < 0
    {
        let e = io::Error::last_os_error();
        remove_q();
        return Err(format!("the tombstone could not be moved aside: {e}"));
    }
    let held_path = folder.join(qname.to_string_lossy().as_ref()).join("link");
    match proved(&held_path, q.as_raw_fd(), &held) {
        Ok(true) => {}
        other => {
            // Swapped between the checks: put it back if its name is free.
            // SAFETY: as above.
            let back = unsafe {
                libc::renameat2(
                    q.as_raw_fd(),
                    held.as_ptr(),
                    dir.as_raw_fd(),
                    name.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            let why = match other {
                Err(e) => e,
                _ => "the tombstone vanished".into(),
            };
            if back == 0 {
                remove_q();
                return Err(format!("{why}; it was put back unchanged"));
            }
            return Err(format!(
                "{why}; it was kept in {} for review",
                folder.join(qname.to_string_lossy().as_ref()).display()
            ));
        }
    }
    // SAFETY: valid descriptor and name.
    if unsafe { libc::unlinkat(q.as_raw_fd(), held.as_ptr(), 0) } < 0 {
        return Err(format!(
            "the proved tombstone could not be removed: {}",
            io::Error::last_os_error()
        ));
    }
    drop(q);
    if remove_q() < 0 {
        return Err("the tombstone was removed but its empty quarantine folder remains".into());
    }
    let _ = dir.sync_all();
    Ok(true)
}
