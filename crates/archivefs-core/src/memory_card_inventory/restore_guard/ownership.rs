//! Serialize EmuWiz operations and refuse metadata we cannot preserve.
use super::*;
use std::os::fd::AsRawFd;

/// Exclusive directory lock for one card operation, released at its end.
///
/// `flock` belongs to the open file description, not the descriptor. A
/// concurrent fork can keep a copy of that description until it execs, so
/// closing our `File` alone may not end the operation. Unlock explicitly, but
/// only in the process that took the lock: a forked child that unwinds a copy
/// of this guard must not release its parent's live operation.
pub(super) struct OperationLock {
    directory: fs::File,
    owner: u32,
}

impl Drop for OperationLock {
    fn drop(&mut self) {
        if std::process::id() == self.owner {
            // Failure leaves the lock to close with the description, as before.
            let _ = self.directory.unlock();
        }
    }
}

pub(super) fn lock(card: &Path) -> Result<OperationLock, Ps2PsuRestoreError> {
    let parent = card
        .parent()
        .ok_or_else(|| Ps2PsuRestoreError::InvalidPlan("Card has no parent directory".into()))?;
    let directory = fs::File::open(parent).map_err(restore_error)?;
    // A directory lock needs no sidecar. It serializes cooperating restores,
    // undo and recovery; exchange verification still protects foreign writers.
    if unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(Ps2PsuRestoreError::RecoveryRequired(
            "Another card operation is active in this directory. Retry after it finishes.".into(),
        ));
    }
    Ok(OperationLock {
        directory,
        owner: std::process::id(),
    })
}

pub(super) fn validate(file: &fs::File) -> Result<fs::Metadata, Ps2PsuRestoreError> {
    let metadata = file.metadata().map_err(restore_error)?;
    if metadata.nlink() != 1 {
        return Err(Ps2PsuRestoreError::InvalidPlan("Cards with multiple hard links are refused: publication would leave the other links on the old card.".into()));
    }
    // ACLs are also xattrs on Linux. Refuse rather than silently discard them.
    let attrs = unsafe { libc::flistxattr(file.as_raw_fd(), std::ptr::null_mut(), 0) };
    if attrs > 0
        || (attrs < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOTSUP))
    {
        return Err(Ps2PsuRestoreError::InvalidPlan("Card extended attributes or ACLs cannot be preserved by this restore. The card was not changed.".into()));
    }
    Ok(metadata)
}

pub(super) fn preserve(file: &fs::File, metadata: &fs::Metadata) -> Result<(), Ps2PsuRestoreError> {
    if unsafe { libc::fchown(file.as_raw_fd(), metadata.uid(), metadata.gid()) } != 0 {
        return Err(restore_error(std::io::Error::last_os_error()));
    }
    file.set_permissions(metadata.permissions())
        .map_err(restore_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_refused(card: &Path) -> bool {
        matches!(lock(card), Err(Ps2PsuRestoreError::RecoveryRequired(_)))
    }

    #[test]
    fn operation_end_releases_lock_despite_inherited_description() {
        let dir = tempfile::tempdir().unwrap();
        let card = dir.path().join("synthetic.ps2");
        let operation = lock(&card).unwrap();
        // dup and fork share one open file description. Keep a duplicate alive
        // as a concurrent pre-exec child can.
        let inherited = operation.directory.try_clone().unwrap();
        assert!(is_refused(&card), "a live operation stays exclusive");
        drop(operation);
        let next = lock(&card).expect("a completed operation must release its lock");
        // Closing the stale copy must not release the next owner's lock.
        drop(inherited);
        assert!(is_refused(&card), "the next operation still owns the lock");
        drop(next);
        lock(&card).expect("the last owner releases the lock");
    }

    #[test]
    fn forked_child_dropping_a_guard_copy_keeps_the_parent_lock() {
        let dir = tempfile::tempdir().unwrap();
        let card = dir.path().join("synthetic.ps2");
        let operation = lock(&card).unwrap();
        // SAFETY: the child runs only the guard's Drop (one flock syscall or
        // none, no allocation) and then `_exit`s without unwinding.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            let status = if std::process::id() == operation.owner {
                2 // fork did not change the pid: the guard cannot tell
            } else {
                0
            };
            drop(operation);
            unsafe { libc::_exit(status) };
        }
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
        assert!(
            is_refused(&card),
            "a forked child released its parent's lock"
        );
        drop(operation);
        lock(&card).expect("the owner still releases its own lock");
    }
}
