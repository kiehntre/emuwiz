//! Filesystem capabilities for the existing single-executable updater.
//! Locks are persistent directory entries; only the kernel releases ownership.
//! Recovery never unlinks a journal-supplied executable or staging pathname.
use super::*;
use crate::catalogue_health::SourceRootBinding;
use std::ffi::{CString, OsString};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::{
    ffi::OsStrExt,
    fs::{MetadataExt, OpenOptionsExt},
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
}
impl FileIdentity {
    pub(super) fn of(file: &File) -> Result<Self, UpdateExecutionError> {
        let m = file.metadata().map_err(io_err)?;
        if !m.is_file() {
            return Err(io_err("not a regular executable"));
        }
        Ok(Self {
            device: m.dev(),
            inode: m.ino(),
        })
    }
}

fn bad(why: &str) -> UpdateExecutionError {
    UpdateExecutionError::Record(why.into())
}
fn absolute(path: &Path) -> Result<(), UpdateExecutionError> {
    if !path.is_absolute()
        || path.components().any(|c| {
            !matches!(
                c,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
    {
        return Err(bad("path is not an absolute, normal installation path"));
    }
    Ok(())
}
// The lock namespace requires case-sensitive, local inode/flock semantics.
// Refuse unverified network/FUSE/case-folding storage rather than claim that
// aliases or remote advisory locking preserve the exclusion guarantee.
fn supported_lock_storage(kind: libc::c_long, flags: libc::c_long) -> bool {
    matches!(kind as u64, 0xef53 | 0x9123_683e | 0x0102_1994) && flags & 0x4000_0000 == 0
}
fn check_lock_storage(parent: &File) -> Result<(), UpdateExecutionError> {
    let mut info = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: writable statfs storage and a live directory descriptor.
    if unsafe { libc::fstatfs(parent.as_raw_fd(), info.as_mut_ptr()) } != 0 {
        return Err(io_err(std::io::Error::last_os_error()));
    }
    let info = unsafe { info.assume_init() };
    let mut flags: libc::c_long = 0;
    if info.f_type as u64 != 0x0102_1994 {
        // SAFETY: FS_IOC_GETFLAGS writes the supplied live c_long storage.
        if unsafe { libc::ioctl(parent.as_raw_fd(), libc::FS_IOC_GETFLAGS, &mut flags) } != 0 {
            return Err(bad("filesystem lock/alias semantics cannot be verified"));
        }
    }
    if !supported_lock_storage(info.f_type, flags) {
        return Err(bad(
            "updates require verified case-sensitive local ext-family, Btrfs or tmpfs storage; other filesystems require review",
        ));
    }
    Ok(())
}
#[cfg(test)]
#[test]
fn lock_storage_refuses_casefold_network_fuse_and_unknown_filesystems() {
    assert!(supported_lock_storage(0xef53, 0));
    assert!(supported_lock_storage(0x9123_683e, 0));
    assert!(supported_lock_storage(0x0102_1994, 0));
    assert!(!supported_lock_storage(0xef53, 0x4000_0000));
    for kind in [0x6969, 0x6573_5546, 0xff53_4d42, 0x5846_5342, 0] {
        assert!(!supported_lock_storage(kind, 0));
    }
}
pub(super) fn pin_directory(path: &Path) -> Result<File, UpdateExecutionError> {
    absolute(path)?;
    #[repr(C)]
    struct How {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    let name = CString::new(path.as_os_str().as_bytes()).map_err(io_err)?;
    let how = How {
        flags: (libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NONBLOCK) as u64,
        mode: 0,
        resolve: 0x04,
    };
    // SAFETY: live NUL-terminated pathname and correctly sized openat2 storage.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            libc::AT_FDCWD,
            name.as_ptr(),
            &how,
            std::mem::size_of::<How>(),
        )
    };
    if fd < 0 {
        return Err(io_err(std::io::Error::last_os_error()));
    }
    Ok(unsafe { File::from_raw_fd(fd as i32) })
}
pub(super) struct Slot {
    parent: File,
    name: OsString,
}
impl Slot {
    pub(super) fn open(path: &Path) -> Result<Self, UpdateExecutionError> {
        absolute(path)?;
        Ok(Self {
            parent: pin_directory(path.parent().ok_or_else(|| bad("missing parent"))?)?,
            name: path
                .file_name()
                .ok_or_else(|| bad("missing filename"))?
                .to_owned(),
        })
    }
    pub(super) fn path(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.parent.as_raw_fd())).join(&self.name)
    }
    pub(super) fn read(&self) -> Result<File, UpdateExecutionError> {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(self.path())
            .map_err(io_err)
    }
    pub(super) fn create(&self) -> Result<File, UpdateExecutionError> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(self.path())
            .map_err(io_err)
    }
    pub(super) fn sync(&self) -> Result<(), UpdateExecutionError> {
        self.parent.sync_all().map_err(io_err)
    }
}
pub(super) fn move_entry(source: &Path, destination: &Path) -> Result<(), UpdateExecutionError> {
    let a = Slot::open(source)?;
    let b = Slot::open(destination)?;
    let from = CString::new(a.name.as_bytes()).map_err(io_err)?;
    let to = CString::new(b.name.as_bytes()).map_err(io_err)?;
    // SAFETY: both names are single components, and both directory FDs stay live.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            a.parent.as_raw_fd(),
            from.as_ptr(),
            b.parent.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if rc != 0 {
        return Err(io_err(std::io::Error::last_os_error()));
    }
    a.sync().and_then(|_| b.sync()).map_err(|e| UpdateExecutionError::NeedsReconciliation(format!("entry moved but directory durability is uncertain ({e}); inspect/recover the recorded operation")))
}

pub(super) struct TargetLock {
    _file: File,
    slot: Slot,
    identity: (u64, u64),
    parent_path: PathBuf,
}
impl TargetLock {
    pub(super) fn check(&self) -> Result<(), UpdateExecutionError> {
        let current_parent = pin_directory(&self.parent_path)?
            .metadata()
            .map_err(io_err)?;
        let pinned_parent = self.slot.parent.metadata().map_err(io_err)?;
        if (current_parent.dev(), current_parent.ino())
            != (pinned_parent.dev(), pinned_parent.ino())
        {
            return Err(bad("locked target directory was replaced"));
        }
        let now = fs::symlink_metadata(self.slot.path()).map_err(io_err)?;
        if !now.is_file() || (now.dev(), now.ino()) != self.identity {
            return Err(UpdateExecutionError::Concurrent(
                "the kernel lock's directory entry changed; nothing further may be mutated".into(),
            ));
        }
        Ok(())
    }
}
// File closure releases flock, including on process death. Never unlink a lock:
// unlinking lets a second process lock a different inode under the same name.
pub(super) fn acquire(target: &Path) -> Result<TargetLock, UpdateExecutionError> {
    let name = target
        .file_name()
        .ok_or_else(|| bad("target has no filename"))?;
    let hash = Sha256::digest(name.as_bytes());
    let key: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    let slot = Slot::open(&target.with_file_name(format!(".emuwiz-update-lock-{key}")))?;
    check_lock_storage(&slot.parent)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(slot.path())
        .map_err(io_err)?;
    let m = file.metadata().map_err(io_err)?;
    if !m.is_file() || m.nlink() != 1 || m.uid() != unsafe { libc::geteuid() } {
        return Err(bad("unsafe lock ownership or kind"));
    }
    // SAFETY: the descriptor is live; nonblocking exclusive advisory flock.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        return Err(if error.kind() == std::io::ErrorKind::WouldBlock {
            UpdateExecutionError::Concurrent(
                "the installation target is locked by a live Apply, Undo or recovery operation"
                    .into(),
            )
        } else {
            io_err(error)
        });
    }
    let guard = TargetLock {
        _file: file,
        slot,
        identity: (m.dev(), m.ino()),
        parent_path: target.parent().unwrap().into(),
    };
    guard.check()?;
    Ok(guard)
}

pub(super) struct Paths {
    pub root: PathBuf,
    pub directory: PathBuf,
    pub target: PathBuf,
    pub backup: PathBuf,
    pub staging: PathBuf,
    pub retained: PathBuf,
    pub displaced: PathBuf,
    pub binding: SourceRootBinding,
    pub parent_binding: SourceRootBinding,
    root_file: File,
}
fn component(text: &str) -> bool {
    !text.is_empty()
        && Path::new(text).file_name() == Some(std::ffi::OsStr::new(text))
        && Path::new(text).components().count() == 1
        && matches!(
            Path::new(text).components().next(),
            Some(std::path::Component::Normal(_))
        )
}
impl Paths {
    pub(super) fn new(
        root: &Path,
        target: &Path,
        tx: &str,
        emulator: InventoryEmulator,
        old: &str,
        new: &str,
    ) -> Result<Self, UpdateExecutionError> {
        if tx.len() != 24
            || !tx
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !component(old)
            || !component(new)
        {
            return Err(bad("invalid transaction identity or version component"));
        }
        absolute(root)?;
        absolute(target)?;
        // One single-executable installation has one journal namespace. An
        // alternate enclosing root could hide a newer record for the same
        // target, even though its kernel lock still excludes simultaneous work.
        if target.parent() != Some(root) {
            return Err(bad(
                "single-executable updates require the installation root to be the executable directory; alternate enclosing roots require review",
            ));
        }
        let root_file = pin_directory(root)?;
        let m = root_file.metadata().map_err(io_err)?;
        if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o022 != 0 {
            return Err(bad("installation root is not privately owned"));
        }
        let binding = SourceRootBinding::inspect(root)
            .ok_or_else(|| bad("installation root cannot be bound"))?;
        let directory = root.join(ROLLBACK_DIR);
        let backup = directory.join(format!("{}-{old}-{tx}", emulator.label()));
        Ok(Self {
            root: root.into(),
            target: target.into(),
            backup,
            staging: target.with_file_name(format!(".emuwiz-update-staging-{tx}")),
            retained: directory.join(format!("{tx}.retained-staging")),
            displaced: directory.join(format!("{}-{new}-{tx}.displaced", emulator.label())),
            directory,
            binding,
            parent_binding: SourceRootBinding::inspect(target.parent().unwrap())
                .ok_or_else(|| bad("target parent cannot be bound"))?,
            root_file,
        })
    }
    pub(super) fn check(&self) -> Result<(), UpdateExecutionError> {
        let m = self.root_file.metadata().map_err(io_err)?;
        if SourceRootBinding::inspect(&self.root) != Some(self.binding.clone())
            || (m.dev(), m.ino()) != (self.binding.device, self.binding.inode)
        {
            return Err(bad("installation root changed or became unavailable"));
        }
        let dir = pin_directory(&self.directory)?;
        let dm = dir.metadata().map_err(io_err)?;
        if dm.uid() != unsafe { libc::geteuid() } || dm.mode() & 0o022 != 0 || dm.dev() != m.dev() {
            return Err(bad(
                "rollback directory has unsafe ownership, permissions or storage identity",
            ));
        }
        let parent = pin_directory(self.target.parent().unwrap())?;
        let pm = parent.metadata().map_err(io_err)?;
        if pm.uid() != unsafe { libc::geteuid() } || pm.mode() & 0o022 != 0 {
            return Err(bad("target directory is not privately owned"));
        }
        if SourceRootBinding::inspect(self.target.parent().unwrap())
            != Some(self.parent_binding.clone())
        {
            return Err(bad("target parent identity changed"));
        }
        if pm.dev() != m.dev() {
            return Err(bad("target crosses an installation filesystem boundary"));
        }
        Ok(())
    }
    pub(super) fn journal(record: &Path, j: &UpdateJournal) -> Result<Self, UpdateExecutionError> {
        let directory = record
            .parent()
            .ok_or_else(|| bad("record has no directory"))?;
        if directory.file_name() != Some(std::ffi::OsStr::new(ROLLBACK_DIR)) {
            return Err(bad("record is not in an installation rollback directory"));
        }
        let root = directory
            .parent()
            .ok_or_else(|| bad("record has no installation root"))?;
        let p = Self::new(
            root,
            &j.target_path,
            &j.transaction_id,
            j.emulator,
            &j.old_version,
            &j.new_version,
        )?;
        if record != record_path(&p.directory, &j.transaction_id)
            || j.rollback_path != p.backup
            || j.staged_path
                .as_ref()
                .is_some_and(|v| v != &p.staging && v != &p.retained)
            || j.displaced_path.as_ref().is_some_and(|v| v != &p.displaced)
        {
            return Err(bad(
                "journal paths contradict the derived transaction-owned paths",
            ));
        }
        if j.target_parent_binding
            .as_ref()
            .is_some_and(|v| v != &p.parent_binding)
        {
            return Err(bad("recorded target directory identity changed"));
        }
        if j.root_binding.as_ref().is_some_and(|v| v != &p.binding) {
            return Err(bad("recorded installation root identity changed"));
        }
        if ![&j.original_sha256, &j.published_sha256]
            .iter()
            .all(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(bad("record lacks valid content fingerprints"));
        }
        p.check()?;
        Ok(p)
    }
}

/// Journal update scratch files are created exclusively, never pre-unlinked.
pub(super) fn persist(j: &UpdateJournal) -> Result<(), UpdateExecutionError> {
    let record = j.record_path()?;
    let paths = Paths::journal(&record, j)?;
    let nonce = update_transaction_id(&record, "journal");
    let temp = paths
        .directory
        .join(format!(".{}-{nonce}.journal.tmp", j.transaction_id));
    let slot = Slot::open(&temp)?;
    let mut file = slot.create()?;
    let bytes = serde_json::to_vec_pretty(j).map_err(io_err)?;
    file.write_all(&bytes).map_err(io_err)?;
    file.sync_all().map_err(io_err)?;
    let destination = Slot::open(&record)?;
    // Only this validated record is replaced. Existing non-record entries are
    // refused; abandoned scratch files remain preserved for explicit review.
    let prior_identity = match fs::symlink_metadata(destination.path()) {
        Ok(m) => Some((m.dev(), m.ino())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(io_err(e)),
    };
    if prior_identity.is_some() {
        let prior = load_journal(&record)?;
        if prior.transaction_id != j.transaction_id
            || prior.target_path != j.target_path
            || prior.rollback_path != j.rollback_path
            || prior.original_sha256 != j.original_sha256
            || prior.published_sha256 != j.published_sha256
            || prior.root_binding != j.root_binding
            || prior.target_parent_binding != j.target_parent_binding
            || prior.original_identity != j.original_identity
            || prior.staged_identity != j.staged_identity
            || prior.sequence != j.sequence
        {
            return Err(bad("durable record identity changed"));
        }
    }
    paths.check()?;
    if file_identity(&temp)? != FileIdentity::of(&file)? {
        return Err(bad("journal scratch ownership changed; evidence preserved"));
    }
    if let Some(prior_identity) = prior_identity {
        // Exchange rather than unlink-replace: even a concurrently substituted
        // record remains byte-for-byte preserved at the unique scratch name.
        let from = CString::new(slot.name.as_bytes()).map_err(io_err)?;
        let to = CString::new(destination.name.as_bytes()).map_err(io_err)?;
        // SAFETY: names are single components and directory FDs remain live.
        if unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                slot.parent.as_raw_fd(),
                from.as_ptr(),
                destination.parent.as_raw_fd(),
                to.as_ptr(),
                libc::RENAME_EXCHANGE,
            )
        } != 0
        {
            return Err(io_err(std::io::Error::last_os_error()));
        }
        destination.sync().map_err(|e| {
            UpdateExecutionError::NeedsReconciliation(format!(
                "journal exchanged but durability is uncertain: {e}"
            ))
        })?;
        let saved = fs::symlink_metadata(slot.path()).map_err(io_err)?;
        if (saved.dev(), saved.ino()) != prior_identity
            || file_identity(&record)? != FileIdentity::of(&file)?
        {
            return Err(UpdateExecutionError::NeedsReconciliation(
                "journal entries changed during exchange; both entries were preserved for review"
                    .into(),
            ));
        }
        Ok(())
    } else {
        move_entry(&temp, &record)
    }
}
