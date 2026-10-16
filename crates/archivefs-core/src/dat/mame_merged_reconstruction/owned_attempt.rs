//! MAME-only staging ownership. Named artifacts are retained, never cleaned
//! or quarantined by pathname. Descriptor-relative creation cannot be
//! redirected through a substituted staging parent.

use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::FileExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::dat::rename_apply::identity::{capture_identity, identity_matches};
use crate::dat::rename_apply::model::{
    EntryState, ObjectFreshness, ObjectIdentity, ObjectKind, RenameTransaction, TransactionState,
};
use sha2::{Digest, Sha256};

pub(super) struct Directory {
    pub(super) file: File,
    pub(super) path: PathBuf,
    bindings: Vec<(PathBuf, u64, u64)>,
}

impl Drop for Directory {
    fn drop(&mut self) {
        // The publication flock belongs to this open file description. A fork()
        // elsewhere in the process (any helper spawn) gives its child another
        // reference until exec, so merely closing our descriptor would keep the
        // lock held and falsely block the next attempt. LOCK_UN acts on the
        // shared description and releases it for every copy; it never affects
        // another attempt's separately opened description. The lock is
        // advisory EmuWiz coordination only, never proof of leaf ownership.
        // SAFETY: self.file is a valid open descriptor until after this drop.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn owned(fd: i32) -> Result<File, String> {
    if fd < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: a successful open/openat returned a fresh owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

impl Directory {
    pub(super) fn open(path: &Path) -> Result<Self, String> {
        Self::open_mode(path, true)?.ok_or_else(|| "directory was not created".into())
    }

    pub(super) fn existing(path: &Path) -> Result<Option<Self>, String> {
        Self::open_mode(path, false)
    }

    fn open_mode(path: &Path, create: bool) -> Result<Option<Self>, String> {
        if !path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        {
            return Err(format!(
                "unsafe staging/recovery directory: {}",
                path.display()
            ));
        }
        // SAFETY: static valid path, flags have no memory effects.
        let mut file = owned(unsafe {
            libc::open(
                c"/".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        })?;
        let mut current = PathBuf::from("/");
        let mut bindings = Vec::new();
        for component in path.components() {
            let Component::Normal(part) = component else {
                continue;
            };
            current.push(part);
            let name = CString::new(part.as_encoded_bytes()).map_err(|e| e.to_string())?;
            let open = || unsafe {
                // SAFETY: live parent fd and single NUL-terminated component;
                // O_NOFOLLOW refuses symlinks, including intermediate ones.
                libc::openat(
                    file.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            let mut fd = open();
            if fd < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
                if !create {
                    return Ok(None);
                }
                // SAFETY: live parent fd and validated single component.
                let rc = unsafe { libc::mkdirat(file.as_raw_fd(), name.as_ptr(), 0o700) };
                if rc != 0
                    && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
                {
                    return Err(format!(
                        "cannot create {}: {}",
                        current.display(),
                        std::io::Error::last_os_error()
                    ));
                }
                fd = open();
            }
            file = owned(fd).map_err(|e| {
                format!(
                    "unsafe staging/recovery ancestor {}: {e}; no file was followed",
                    current.display()
                )
            })?;
            let m = file.metadata().map_err(|e| e.to_string())?;
            bindings.push((current.clone(), m.dev(), m.ino()));
        }
        let dir = Self {
            file,
            path: path.to_owned(),
            bindings,
        };
        dir.require_binding()?;
        Ok(Some(dir))
    }

    pub(super) fn require_binding(&self) -> Result<(), String> {
        for (path, dev, ino) in &self.bindings {
            let m = std::fs::symlink_metadata(path).map_err(|e| {
                format!(
                    "staging/recovery directory binding unavailable at {}: {e}",
                    path.display()
                )
            })?;
            if !m.is_dir() || m.is_symlink() || (m.dev(), m.ino()) != (*dev, *ino) {
                return Err(format!(
                    "staging/recovery directory binding changed at {}; retained artifacts were not moved or deleted",
                    path.display()
                ));
            }
        }
        Ok(())
    }

    pub(super) fn lock(&self) -> Result<(), String> {
        self.require_binding()?;
        // SAFETY: retained valid directory fd. flock coordinates EmuWiz only;
        // it is deliberately never treated as proof of leaf ownership.
        if unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(format!(
                "another MAME publication is in progress in {}; retry after it finishes; nothing was staged or moved",
                self.path.display()
            ));
        }
        Ok(())
    }

    fn create(&self, leaf: &str) -> Result<File, std::io::Error> {
        self.create_mode(leaf, 0o600)
    }

    fn create_mode(&self, leaf: &str, mode: libc::mode_t) -> Result<File, std::io::Error> {
        let name = CString::new(leaf)?;
        // SAFETY: live pinned directory and internally generated basename.
        let fd = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                mode,
            )
        };
        if fd < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            // SAFETY: exclusive openat returned this fresh descriptor.
            Ok(unsafe { File::from_raw_fd(fd) })
        }
    }

    pub(super) fn require_file_binding(&self, leaf: &str, file: &File) -> Result<(), String> {
        let name = CString::new(leaf).map_err(|e| e.to_string())?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: retained directory, single basename and writable stat;
        // symlinks are inspected without following their targets.
        if unsafe {
            libc::fstatat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(format!(
                "owned diagnostic/journal binding unavailable for {leaf}: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: successful fstatat initialized stat.
        let stat = unsafe { stat.assume_init() };
        let held = file.metadata().map_err(|e| e.to_string())?;
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG
            || (stat.st_dev, stat.st_ino) != (held.dev(), held.ino())
            || stat.st_nlink != 1
            || held.nlink() != 1
        {
            return Err(format!(
                "owned diagnostic/journal binding changed for {leaf}; foreign entries were not moved, overwritten or deleted"
            ));
        }
        Ok(())
    }

    pub(super) fn read_json(&self, leaf: &str) -> Result<serde_json::Value, String> {
        let name = CString::new(leaf).map_err(|e| e.to_string())?;
        // SAFETY: pinned directory and basename from its enumeration;
        // never follow a symlink or wait on a substituted special file.
        let file = owned(unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        })?;
        let before = file.metadata().map_err(|e| e.to_string())?;
        if !before.is_file() || before.len() > 16 * 1024 * 1024 {
            return Err("recovery record is not a bounded regular file (16 MiB limit)".into());
        }
        let mut body = Vec::new();
        (&file)
            .take(before.len() + 1)
            .read_to_end(&mut body)
            .map_err(|e| e.to_string())?;
        let after = file.metadata().map_err(|e| e.to_string())?;
        if body.len() as u64 != before.len()
            || (
                before.len(),
                before.mtime(),
                before.mtime_nsec(),
                before.ctime(),
                before.ctime_nsec(),
            ) != (
                after.len(),
                after.mtime(),
                after.mtime_nsec(),
                after.ctime(),
                after.ctime_nsec(),
            )
        {
            return Err("recovery record changed while reading".into());
        }
        serde_json::from_slice(&body).map_err(|e| e.to_string())
    }

    pub(super) fn transactions(&self) -> Result<Vec<RenameTransaction>, String> {
        self.require_binding()?;
        let mut records = Vec::new();
        // Enumeration and reads refer to the same pinned directory object.
        let anchor = PathBuf::from(format!("/proc/self/fd/{}", self.file.as_raw_fd()));
        for entry in std::fs::read_dir(anchor).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.path().extension().is_some_and(|e| e == "json") {
                continue;
            }
            let leaf = entry.file_name();
            let leaf = leaf
                .to_str()
                .ok_or("recovery journal has an unsupported filename")?;
            let value = self.read_json(leaf).map_err(|e| format!("unreadable recovery journal {}: {e}; inspect Recovery before publishing; publication refused", self.path.join(leaf).display()))?;
            let record = serde_json::from_value(value).map_err(|e| format!("malformed recovery journal {}: {e}; inspect Recovery before publishing; publication refused", self.path.join(leaf).display()))?;
            records.push(record);
        }
        self.require_binding()?;
        Ok(records)
    }

    pub(super) fn anonymous(&self) -> Result<File, String> {
        #[cfg(target_os = "linux")]
        {
            // SAFETY: live directory fd, '.' is a static single component.
            // No directory entry exists, so releasing this scratch handle
            // cannot unlink a substituted foreign pathname.
            owned(unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    c".".as_ptr(),
                    libc::O_RDWR | libc::O_TMPFILE | libc::O_CLOEXEC,
                    0o600,
                )
            })
            .map_err(|e| {
                format!(
                    "owned anonymous MAME scratch is unavailable in {}: {e}; no unsafe fallback",
                    self.path.display()
                )
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err("owned MAME staging requires Linux anonymous scratch support".into())
        }
    }
}

pub(super) fn token() -> Result<String, String> {
    #[cfg(test)]
    if let Some(value) = TOKENS.with(|tokens| tokens.borrow_mut().pop_front()) {
        return Ok(value);
    }
    let mut random = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut random))
        .map_err(|e| e.to_string())?;
    Ok(random.iter().map(|b| format!("{b:02x}")).collect())
}

pub(super) struct StagedOutput {
    pub path: PathBuf,
    pub file: File,
    pub root: Arc<Directory>,
    name: String,
    created: (u64, u64),
}

impl StagedOutput {
    pub(super) fn create(root: Arc<Directory>) -> Result<Self, String> {
        root.require_binding()?;
        for _ in 0..64 {
            let name = format!("mame-{}.zip.staged", token()?);
            // Preserve the original archive creation mode and caller umask;
            // journal/diagnostic files remain private. Ownership comes from
            // exclusive creation and the held fd, never from permissions.
            match root.create_mode(&name, 0o666) {
                Ok(file) => {
                    let m = file.metadata().map_err(|e| e.to_string())?;
                    let value = Self {
                        path: root.path.join(&name),
                        file,
                        root,
                        name,
                        created: (m.dev(), m.ino()),
                    };
                    event("created", &value.path)?;
                    value.require_owned()?;
                    return Ok(value);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("cannot exclusively create MAME stage: {e}")),
            }
        }
        Err("could not reserve a MAME stage; existing artifacts were not changed".into())
    }

    pub(super) fn require_owned(&self) -> Result<(), String> {
        self.root.require_binding()?;
        let name = CString::new(self.name.as_str()).map_err(|e| e.to_string())?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: pinned root fd, validated basename, writable stat output;
        // AT_SYMLINK_NOFOLLOW inspects the entry, never its target.
        if unsafe {
            libc::fstatat(
                self.root.file.as_raw_fd(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(format!(
                "owned stage binding unavailable at {}: {}",
                self.path.display(),
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: successful fstatat initialized stat.
        let m = unsafe { stat.assume_init() };
        let held = self.file.metadata().map_err(|e| e.to_string())?;
        if m.st_mode & libc::S_IFMT != libc::S_IFREG
            || (m.st_dev, m.st_ino) != self.created
            || (held.dev(), held.ino()) != self.created
            || m.st_nlink != 1
            || held.nlink() != 1
        {
            return Err(format!(
                "owned stage binding changed at {}; foreign entries and retained output were not moved or deleted",
                self.path.display()
            ));
        }
        self.root.require_binding()
    }

    pub(super) fn identity(&self) -> Result<ObjectIdentity, String> {
        self.require_owned()?;
        let before = self.file.metadata().map_err(|e| e.to_string())?;
        let mut sha = Sha256::new();
        let mut offset = 0u64;
        let mut buffer = [0u8; 65536];
        loop {
            let length = buffer
                .len()
                .min(before.len().saturating_add(1).saturating_sub(offset) as usize);
            if length == 0 {
                break;
            }
            let n = self
                .file
                .read_at(&mut buffer[..length], offset)
                .map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            sha.update(&buffer[..n]);
            offset += n as u64;
        }
        let after = self.file.metadata().map_err(|e| e.to_string())?;
        if offset != before.len()
            || (
                before.len(),
                before.mtime(),
                before.mtime_nsec(),
                before.ctime(),
                before.ctime_nsec(),
            ) != (
                after.len(),
                after.mtime(),
                after.mtime_nsec(),
                after.ctime(),
                after.ctime_nsec(),
            )
        {
            return Err("owned output changed while capturing its content proof".into());
        }
        let identity = ObjectIdentity {
            kind: ObjectKind::RegularFile,
            dev: before.dev(),
            ino: before.ino(),
            size_bytes: before.len(),
            modified_unix: before.mtime(),
            freshness: Some(ObjectFreshness {
                version: 1,
                modified: before.modified().map_err(|e| e.to_string())?,
                sha256: sha.finalize().into(),
            }),
        };
        self.require_owned()?;
        if (identity.dev, identity.ino) != self.created {
            return Err("staged identity does not belong to this attempt".into());
        }
        Ok(identity)
    }

    pub(super) fn note(&self, parent: &str, detail: serde_json::Value) -> Result<PathBuf, String> {
        for counter in 0..64 {
            let leaf = format!("{}.note-{counter}.json", self.name);
            let mut file = match self.root.create(&leaf) {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            };
            let body = serde_json::json!({"schema": 1, "parent": parent, "staged_path": self.path, "created_dev": self.created.0, "created_ino": self.created.1, "identity": self.identity().ok(), "detail": detail, "retention": "Named artifacts are retained in place; use transaction recovery for Undo originals."});
            file.write_all(
                serde_json::to_string_pretty(&body)
                    .map_err(|e| e.to_string())?
                    .as_bytes(),
            )
            .and_then(|()| file.sync_all())
            .map_err(|e| e.to_string())?;
            event("diagnostic_written", &self.root.path.join(&leaf))?;
            self.root.require_file_binding(&leaf, &file)?;
            return Ok(self.root.path.join(leaf));
        }
        Err("all diagnostic names are occupied; existing notes were left untouched".into())
    }

    pub(super) fn failure(&self, parent: &str, error: String) -> String {
        match self.note(
            parent,
            serde_json::json!({"state": "refused", "reason": error}),
        ) {
            Ok(note) => format!(
                "{error}; no staging cleanup was performed; inspect Recovery and attempt {}; diagnostic recorded in the pinned staging directory as {} (a changed directory binding may make this displayed path unavailable)",
                self.path.display(),
                note.display()
            ),
            Err(e) => format!(
                "{error}; no staging cleanup was performed; diagnostic could not be written: {e}"
            ),
        }
    }
}

pub(super) fn staging_root(path: &Path) -> Result<Arc<Directory>, String> {
    let dir = Directory::open(path)?;
    dir.lock()?;
    Ok(Arc::new(dir))
}

pub(super) struct PublicationGuard {
    pub staging: Arc<Directory>,
    journals: Option<Arc<Directory>>,
    journal_path: PathBuf,
}

impl PublicationGuard {
    pub(super) fn new(staging: &Path, journal: &Path) -> Result<Self, String> {
        let staging = staging_root(staging)?;
        if staging.path == journal {
            return Err("MAME staging and recovery journals require separate directories".into());
        }
        let journals = match Directory::existing(journal)? {
            Some(dir) => {
                dir.lock()?;
                Some(Arc::new(dir))
            }
            None => None,
        };
        let value = Self {
            staging,
            journals,
            journal_path: journal.to_owned(),
        };
        value.check_recovery()?;
        Ok(value)
    }

    pub(super) fn check_recovery(&self) -> Result<(), String> {
        self.staging.require_binding()?;
        let Some(journal_dir) = &self.journals else {
            return Ok(());
        };
        journal_dir.require_binding()?;
        let journals = journal_dir.transactions()?;
        for t in journals {
            let settled_failure = t.unknown.get("workflow").and_then(|v| v.as_str())
                == Some(super::MAME_RECONSTRUCTION_WORKFLOW)
                && !t.entries.is_empty()
                && t.state == TransactionState::ApplyFailed
                && t.entries.iter().all(|e| {
                    matches!(e.state, EntryState::ApplyFailed | EntryState::Skipped)
                        && capture_identity(&e.source_path)
                            .is_ok_and(|i| identity_matches(&e.identity, &i))
                });
            let in_flight_entry = t.entries.iter().any(|e| {
                matches!(
                    e.state,
                    EntryState::Applying | EntryState::RollingBack | EntryState::RollbackFailed
                )
            });
            if in_flight_entry || (t.state.needs_recovery() && !settled_failure) {
                return Err(format!(
                    "transaction {} is {} and requires Recovery before another publication; its journal and Undo files were left untouched",
                    t.transaction_id,
                    t.state.label()
                ));
            }
        }
        journal_dir.require_binding()
    }

    pub(super) fn check_leftovers(&self, parent: &str) -> Result<(), String> {
        use crate::dat::rename_apply::model::TransactionOperation;
        self.staging.require_binding()?;
        let transactions = match &self.journals {
            Some(dir) => dir.transactions()?,
            None => Vec::new(),
        };
        let legacy = self.staging.path.join(format!("{parent}.zip.staged"));
        let anchor = PathBuf::from(format!("/proc/self/fd/{}", self.staging.file.as_raw_fd()));
        for entry in std::fs::read_dir(anchor).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = self.staging.path.join(entry.file_name());
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path != legacy && !(name.starts_with("mame-") && name.ends_with(".zip.staged")) {
                continue;
            }
            if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
                return Err(format!(
                    "leftover {} is not a regular file and was not followed; inspect Recovery before publishing",
                    path.display()
                ));
            }
            let actual = capture_identity(&path).map_err(|e| format!("leftover {} cannot be verified: {e}; inspect Recovery, do not move an Undo backup manually", path.display()))?;
            let recorded = transactions.iter().any(|t| t.entries.iter().any(|e| {
                (e.source_path == path || e.destination_path == path) &&
                (identity_matches(&e.identity, &actual) || matches!(&e.operation, TransactionOperation::ReplaceExisting { original_identity, .. } if identity_matches(original_identity, &actual)))
            }));
            if recorded {
                continue;
            }
            // An unpublished attempt can be retained without a transaction.
            // Its complete diagnostic must bind the exact current object;
            // filenames and content matching alone never authorize adoption.
            let mut noted = false;
            if path != legacy {
                for n in 0..64 {
                    let leaf = format!("{name}.note-{n}.json");
                    let Ok(v) = self.staging.read_json(&leaf) else {
                        continue;
                    };
                    if v.get("staged_path").and_then(|v| v.as_str()) != path.to_str() {
                        continue;
                    }
                    if let Some(i) = v
                        .get("identity")
                        .and_then(|v| serde_json::from_value::<ObjectIdentity>(v.clone()).ok())
                    {
                        if identity_matches(&i, &actual) {
                            noted = true;
                            break;
                        }
                    }
                }
            }
            if !noted {
                return Err(format!(
                    "unrecorded or changed leftover {} was left untouched; inspect Recovery and its diagnostics before publishing; do not move an Undo backup manually",
                    path.display()
                ));
            }
        }
        self.staging.require_binding()
    }

    pub(super) fn reserve(&mut self, transaction: &mut RenameTransaction) -> Result<(), String> {
        // Creating a journal directory is deferred until staging and all
        // publication gates succeed. Existing journals are inspected first;
        // a directory created by another process during staging is inspected
        // under the same publication lock before any journal is reserved.
        if self.journals.is_none() {
            let dir = Directory::open(&self.journal_path)?;
            dir.lock()?;
            self.journals = Some(Arc::new(dir));
        }
        self.check_recovery()?;
        let journals = self
            .journals
            .as_ref()
            .ok_or("journal directory unavailable")?;
        journals.require_binding()?;
        for _ in 0..64 {
            // Never leave a previous, colliding name behind if the token fails.
            transaction.transaction_id.clear();
            transaction.transaction_id = format!("mame-{}", token()?);
            let leaf = format!("{}.json", transaction.transaction_id);
            let mut file = match journals.create(&leaf) {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    // Nothing was created: do not name a journal that does not exist.
                    transaction.transaction_id.clear();
                    return Err(format!("could not exclusively reserve MAME journal: {e}"));
                }
            };
            file.write_all(
                serde_json::to_string_pretty(transaction)
                    .map_err(|e| e.to_string())?
                    .as_bytes(),
            )
            .and_then(|()| file.sync_all())
            .map_err(|e| {
                format!("reserved journal could not be completed: {e}; Recovery must inspect it")
            })?;
            journals.require_file_binding(&leaf, &file)?;
            journals.file.sync_all().map_err(|e| e.to_string())?;
            return journals.require_binding();
        }
        transaction.transaction_id.clear();
        Err(
            "could not exclusively reserve a MAME journal; earlier journals were not overwritten"
                .into(),
        )
    }
}

#[cfg(test)]
thread_local! {
    pub(super) static HOOK: std::cell::RefCell<Option<Box<dyn FnMut(&str, &Path) -> Result<(), String>>>> = const { std::cell::RefCell::new(None) };
    pub(super) static TOKENS: std::cell::RefCell<std::collections::VecDeque<String>> = const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
}

pub(super) fn event(name: &str, path: &Path) -> Result<(), String> {
    #[cfg(test)]
    return HOOK.with(|hook| match hook.borrow_mut().as_mut() {
        Some(h) => h(name, path),
        None => Ok(()),
    });
    #[cfg(not(test))]
    {
        let _ = (name, path);
        Ok(())
    }
}
