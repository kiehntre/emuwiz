//! Kernel evidence of file access, independent of process identity.
//!
//! A Linux read lease (`F_SETLEASE`, `F_RDLCK`) on a card inode is granted
//! only while no process (any user, any container, inspectable or not) has
//! that inode open for writing, including writable shared mappings. While it
//! is held, any open of the inode for writing, or a truncate, breaks it: the
//! opener is held back by the kernel until we release the lease, and we see
//! the break. The lease says nothing about which programs are running; it
//! only proves that the inode was not opened for writing in the window.
//!
//! The card operations hold these leases across their whole publication
//! window and only release them after any reversal (see `restore_guard`).
use super::*;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;

/// Sent to us when a lease is broken. Its default action is to be ignored;
/// the kernel's default lease-break signal, SIGIO, would end the process.
const LEASE_BREAK_SIGNAL: libc::c_int = libc::SIGURG;
/// `F_SETSIG` (asm-generic/fcntl.h, every Linux architecture); not exported
/// by the `libc` crate.
const F_SETSIG: libc::c_int = 10;

/// A held read lease on one inode. Released on drop.
pub struct CardWriteLease {
    file: fs::File,
}

pub enum CardLeaseAttempt {
    /// Nobody had the inode open for writing when the lease was granted.
    Held(CardWriteLease),
    /// The kernel refused: the inode is open for writing (or leased) now.
    Writers,
    /// The path no longer names the expected inode.
    Changed,
    /// No kernel evidence: not the owner, no lease support, leases
    /// disabled, a link, or the break signal is not safe to receive.
    Unavailable,
}

impl CardWriteLease {
    /// Lease the regular file at `path`, never following a link. With
    /// `expected`, the opened inode must be that file.
    pub fn acquire(path: &Path, expected: Option<FileIdentity>) -> CardLeaseAttempt {
        if !signal_is_default_or_ignored(LEASE_BREAK_SIGNAL) {
            return CardLeaseAttempt::Unavailable;
        }
        // Read-only (a read lease requires it), and never blocking on
        // someone else's lease.
        let file = match fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_CLOEXEC)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.raw_os_error() == Some(libc::EWOULDBLOCK) => {
                return CardLeaseAttempt::Writers;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return CardLeaseAttempt::Changed;
            }
            Err(_) => return CardLeaseAttempt::Unavailable,
        };
        let Ok(metadata) = file.metadata() else {
            return CardLeaseAttempt::Unavailable;
        };
        if !metadata.is_file() {
            return CardLeaseAttempt::Unavailable;
        }
        if let Some(expected) = expected
            && (metadata.dev() != expected.device || metadata.ino() != expected.inode)
        {
            return CardLeaseAttempt::Changed;
        }
        let fd = file.as_raw_fd();
        // SAFETY: fcntl on a descriptor we own, with integer arguments.
        if unsafe { libc::fcntl(fd, F_SETSIG, LEASE_BREAK_SIGNAL) } != 0 {
            return CardLeaseAttempt::Unavailable;
        }
        // SAFETY: as above.
        if unsafe { libc::fcntl(fd, libc::F_SETLEASE, libc::F_RDLCK) } == 0 {
            return CardLeaseAttempt::Held(Self { file });
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::EAGAIN) => CardLeaseAttempt::Writers,
            // EACCES (not the owner), EINVAL (no lease support), ...
            _ => CardLeaseAttempt::Unavailable,
        }
    }

    /// Still an unbroken read lease: nobody opened the inode for writing.
    pub fn intact(&self) -> bool {
        // SAFETY: fcntl on a descriptor we own.
        (unsafe { libc::fcntl(self.file.as_raw_fd(), libc::F_GETLEASE) }) == libc::F_RDLCK
    }

    /// (device, inode) of the leased file.
    pub fn inode(&self) -> Option<(u64, u64)> {
        self.file.metadata().ok().map(|m| (m.dev(), m.ino()))
    }
}

impl Drop for CardWriteLease {
    fn drop(&mut self) {
        // Closing would release it too; releasing first frees any waiting
        // opener without depending on close ordering.
        // SAFETY: fcntl on a descriptor we own.
        unsafe { libc::fcntl(self.file.as_raw_fd(), libc::F_SETLEASE, libc::F_UNLCK) };
    }
}

fn signal_is_default_or_ignored(signal: libc::c_int) -> bool {
    // SAFETY: querying (not changing) a disposition into zeroed storage.
    let mut current: libc::sigaction = unsafe { std::mem::zeroed() };
    if unsafe { libc::sigaction(signal, std::ptr::null(), &mut current) } != 0 {
        return false;
    }
    current.sa_sigaction == libc::SIG_DFL || current.sa_sigaction == libc::SIG_IGN
}

/// Observe under a lease on the card held only for this observation. The
/// caller must protect its own later window (re-check, or hold a lease).
/// Without kernel evidence this is exactly `observe` (strict).
pub fn observe_card_now(
    quiescence: &dyn QuiescenceProvider,
    binding: &DirectorySaveBinding,
    card: &Path,
) -> EmulatorQuiescence {
    match CardWriteLease::acquire(card, None) {
        CardLeaseAttempt::Held(lease) => quiescence.observe_under_card_lease(binding, &lease),
        CardLeaseAttempt::Writers => EmulatorQuiescence::Running,
        CardLeaseAttempt::Changed | CardLeaseAttempt::Unavailable => quiescence.observe(binding),
    }
}

/// Leases on both inodes taking part in one atomic exchange: the live card
/// and the prepared replacement. Held until the operation, including any
/// reversal, is finished.
pub(super) struct PublicationLeases {
    /// The card being replaced. After the exchange it sits at the staging
    /// path; an opener that resolved the card path before the exchange holds
    /// it, so a break here means the exchange must be reversed.
    pub(super) outgoing: CardWriteLease,
    /// The replacement. A break after the exchange means a program opened
    /// the new live card: it is held back until release and then sees the
    /// new card, exactly as if it had opened it after the operation.
    pub(super) incoming: CardWriteLease,
}

pub(super) enum PublicationLeaseAttempt {
    Held(PublicationLeases),
    Writers,
    Changed,
    Unavailable,
}

impl PublicationLeases {
    pub(super) fn acquire(
        card: &Path,
        card_identity: FileIdentity,
        replacement: &Path,
        replacement_identity: FileIdentity,
    ) -> PublicationLeaseAttempt {
        let outgoing = match CardWriteLease::acquire(card, Some(card_identity)) {
            CardLeaseAttempt::Held(lease) => lease,
            CardLeaseAttempt::Writers => return PublicationLeaseAttempt::Writers,
            CardLeaseAttempt::Changed => return PublicationLeaseAttempt::Changed,
            CardLeaseAttempt::Unavailable => return PublicationLeaseAttempt::Unavailable,
        };
        match CardWriteLease::acquire(replacement, Some(replacement_identity)) {
            CardLeaseAttempt::Held(incoming) => {
                PublicationLeaseAttempt::Held(Self { outgoing, incoming })
            }
            CardLeaseAttempt::Writers => PublicationLeaseAttempt::Writers,
            CardLeaseAttempt::Changed => PublicationLeaseAttempt::Changed,
            CardLeaseAttempt::Unavailable => PublicationLeaseAttempt::Unavailable,
        }
    }
}
