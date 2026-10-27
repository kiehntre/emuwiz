//! Serialize EmuWiz operations and refuse metadata we cannot preserve.
use super::*;
use std::os::fd::AsRawFd;

pub(super) struct OperationLock {
    directory: fs::File,
}

impl Drop for OperationLock {
    fn drop(&mut self) {
        // flock belongs to the open file description. A concurrent fork can
        // retain a copy until exec, so close alone may outlive this operation.
        // Unlock only our description, after all guarded work has finished.
        let _ = self.directory.unlock();
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
    Ok(OperationLock { directory })
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

    #[test]
    fn completed_operation_releases_lock_with_inherited_descriptor() {
        let dir = tempfile::tempdir().unwrap();
        let card = dir.path().join("synthetic.ps2");
        let owner = lock(&card).unwrap();
        // dup and fork share the same open file description. Keep a duplicate
        // alive to reproduce a sibling thread's helper waiting to exec.
        let inherited = owner.directory.try_clone().unwrap();
        assert!(matches!(
            lock(&card),
            Err(Ps2PsuRestoreError::RecoveryRequired(_))
        ));
        drop(owner);
        let next =
            lock(&card).expect("completed operation must release its lock before helper exec");
        // Closing an old inherited handle must not release the next owner's lock.
        drop(inherited);
        assert!(matches!(
            lock(&card),
            Err(Ps2PsuRestoreError::RecoveryRequired(_))
        ));
        drop(next);
        lock(&card).unwrap();
    }
}
