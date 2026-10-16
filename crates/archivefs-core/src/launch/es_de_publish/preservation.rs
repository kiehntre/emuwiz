//! Destination binding and cooperating-writer exclusion for ES-DE snapshots.
//!
//! The shared object-identity implementation supplies descriptor-bound hashes.
//! Directory evidence binds the namespace, not just the final filename. The
//! advisory lock excludes EmuWiz writers; it cannot exclude ES-DE or an editor
//! ignoring that lock. Path-based replacement still has a check/use window.
use super::*;
use crate::dat::rename_apply::identity::{capture_identity, identity_matches};
use crate::dat::rename_apply::model::{ObjectIdentity, ObjectKind};
use std::path::Component;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestinationEvidence {
    directories: Vec<DirectoryIdentity>,
    original: Option<ObjectIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DirectoryIdentity {
    path: PathBuf,
    device: u64,
    inode: u64,
}

pub(super) fn conflict(path: &Path, detail: impl Into<String>) -> EsDePublicationError {
    EsDePublicationError::Io {
        path: path.to_path_buf(),
        detail: format!("publication conflict; review again: {}", detail.into()),
    }
}

fn directory(path: &Path) -> Result<DirectoryIdentity, EsDePublicationError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| conflict(path, e.to_string()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(conflict(path, "ancestor is not a plain directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(DirectoryIdentity {
            path: path.into(),
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        Err(conflict(
            path,
            "destination identity is unsupported on this host",
        ))
    }
}

impl DestinationEvidence {
    pub fn capture(path: &Path) -> Result<Self, EsDePublicationError> {
        if !path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        {
            return Err(conflict(
                path,
                "destination must be absolute without traversal",
            ));
        }
        let mut directories = Vec::new();
        let mut current = PathBuf::new();
        for component in path
            .parent()
            .ok_or_else(|| conflict(path, "missing parent"))?
            .components()
        {
            current.push(component);
            match fs::symlink_metadata(&current) {
                Ok(_) => directories.push(directory(&current)?),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                Err(e) => return Err(conflict(&current, e.to_string())),
            }
        }
        let original = current_identity(path)?;
        Ok(Self {
            directories,
            original,
        })
    }

    pub(super) fn verify_directories(&self, path: &Path) -> Result<(), EsDePublicationError> {
        let now = Self::capture(path)?;
        if self.directories.is_empty()
            || now.directories.len() < self.directories.len()
            || now.directories[..self.directories.len()] != self.directories
        {
            return Err(conflict(
                path,
                "reviewed destination directory changed or is unavailable",
            ));
        }
        Ok(())
    }

    pub(super) fn verify_before(
        &self,
        path: &Path,
        before: &Option<String>,
    ) -> Result<(), EsDePublicationError> {
        self.verify_directories(path)?;
        let now = current_identity(path)?;
        let same = match (&self.original, &now) {
            (None, None) => true,
            (Some(a), Some(b)) => identity_matches(a, b),
            _ => false,
        };
        if !same || read_current(path)? != *before {
            return Err(conflict(path, "gamelist changed since preview"));
        }
        Ok(())
    }
}

fn current_identity(path: &Path) -> Result<Option<ObjectIdentity>, EsDePublicationError> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(conflict(path, e.to_string())),
        Ok(m) => {
            if m.len() > MAX_GAMELIST_BYTES {
                return Err(EsDePublicationError::GamelistTooLarge {
                    path: path.into(),
                    bytes: m.len(),
                });
            }
            if !m.is_file() {
                return Err(conflict(path, "destination is not a bounded regular file"));
            }
            let proof = capture_identity(path).map_err(|e| conflict(path, e.to_string()))?;
            if proof.kind != ObjectKind::RegularFile {
                return Err(conflict(path, "destination changed type"));
            }
            Ok(Some(proof))
        }
    }
}

pub(super) fn read_current(path: &Path) -> Result<Option<String>, EsDePublicationError> {
    let before = current_identity(path)?;
    let Some(before) = before else {
        return Ok(None);
    };
    let text = read_bounded_gamelist(&path.to_path_buf())?;
    let after = current_identity(path)?
        .ok_or_else(|| conflict(path, "destination disappeared while reading"))?;
    if !identity_matches(&before, &after) {
        return Err(conflict(path, "destination changed while reading"));
    }
    Ok(Some(text))
}

/// Persistent lock inode: never unlink on release (unlink permits split locks).
/// Kernel releases the advisory lock on crash. No automatic stale-lock deletion.
pub(super) struct PublicationLock {
    _file: fs::File,
}

impl PublicationLock {
    pub(super) fn acquire(path: &Path) -> Result<Self, EsDePublicationError> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
            let lock_path = path.with_file_name(format!(
                "{}.es-de-publish.lock",
                path.file_name()
                    .ok_or_else(|| conflict(path, "missing filename"))?
                    .to_string_lossy()
            ));
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
                .open(&lock_path)
                .map_err(|e| conflict(&lock_path, e.to_string()))?;
            let opened = file
                .metadata()
                .map_err(|e| conflict(&lock_path, e.to_string()))?;
            if !opened.is_file() {
                return Err(conflict(&lock_path, "lock is not a regular file"));
            }
            // flock belongs to the open file description. A process that forks (to
            // spawn any helper) holds a close-on-exec copy of every descriptor until
            // its exec, so a lock that was just released can look held for that
            // short window. Wait a bounded time for it; a genuinely live publisher
            // still ends in a refusal, and the wait admits nobody the kernel has not
            // released.
            let started = std::time::Instant::now();
            loop {
                // SAFETY: valid owned descriptor; flock does not retain a pointer.
                if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                    break;
                }
                if std::io::Error::last_os_error().kind() != std::io::ErrorKind::WouldBlock
                    || started.elapsed() >= std::time::Duration::from_millis(300)
                {
                    return Err(conflict(
                        &lock_path,
                        "another publication owns this destination",
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let current = fs::symlink_metadata(&lock_path)
                .map_err(|e| conflict(&lock_path, e.to_string()))?;
            if current.dev() != opened.dev() || current.ino() != opened.ino() || !current.is_file()
            {
                return Err(conflict(&lock_path, "lock pathname changed"));
            }
            Ok(Self { _file: file })
        }
        #[cfg(not(unix))]
        {
            Err(conflict(
                path,
                "publication exclusion is unsupported on this host",
            ))
        }
    }
}

#[cfg(all(test, unix))]
mod lock_tests {
    use super::*;

    /// flock belongs to the open file description. A process that forks (to spawn
    /// any helper) holds a close-on-exec copy of every descriptor until its exec,
    /// so a just-released lock can look held for that window. Many spawners make
    /// the window easy to hit.
    #[test]
    fn a_released_publication_lock_is_reacquired_while_other_threads_spawn_processes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gamelist.xml");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let spawners: Vec<_> = (0..6)
            .map(|_| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        let _ = std::process::Command::new("true").status();
                    }
                })
            })
            .collect();
        let mut spurious = 0;
        for _ in 0..400 {
            match PublicationLock::acquire(&path) {
                Ok(lock) => drop(lock),
                Err(_) => spurious += 1,
            }
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for spawner in spawners {
            spawner.join().unwrap();
        }
        assert_eq!(spurious, 0, "spurious publication-lock conflicts");
    }

    #[test]
    fn a_genuinely_held_publication_lock_is_still_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gamelist.xml");
        let held = PublicationLock::acquire(&path).unwrap();
        let started = std::time::Instant::now();
        assert!(PublicationLock::acquire(&path).is_err());
        // Bounded: the wait that absorbs the fork window is short.
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        drop(held);
        assert!(PublicationLock::acquire(&path).is_ok());
    }
}
