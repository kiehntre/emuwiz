//! Filesystem ownership of descriptor-pinned Amiga inspection sources.

use crate::launch::process_spawn::CapturedFileIdentity;
use std::fs::{File, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// The inspected object, captured from the opened descriptor. ctime adds
/// detection of same-inode writes whose size and mtime were restored, when
/// the filesystem records a distinguishable change timestamp.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct OpenedSource {
    identity: CapturedFileIdentity,
    changed: (i64, i64),
}

impl OpenedSource {
    pub(crate) fn capture(metadata: &Metadata) -> Self {
        Self {
            identity: CapturedFileIdentity::capture(metadata),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }

    /// Recheck the descriptor, then resolve the original name without opening
    /// or reading another file. Follow symlinks so stable links and anonymous
    /// ADZ staging descriptors work, while a retarget to another inode fails.
    /// This is a final point-in-time check, not a lock or atomic snapshot:
    /// another process can replace the pathname after this check returns.
    pub(crate) fn unchanged(self, file: &File, path: &Path) -> io::Result<bool> {
        if !self.matches(&file.metadata()?) {
            return Ok(false);
        }
        match std::fs::metadata(path) {
            Ok(named) => Ok(self.matches(&named)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn matches(self, metadata: &Metadata) -> bool {
        metadata.is_file() && self == Self::capture(metadata)
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::cell::RefCell;
    use std::fs::{File, FileTimes};
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, symlink};
    use std::path::{Path, PathBuf};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Checkpoint {
        Captured,
        BeforePublish,
    }

    struct Hook {
        path: PathBuf,
        point: Checkpoint,
        action: Box<dyn FnOnce()>,
    }
    thread_local! {
        static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    pub fn checkpoint(path: &Path, point: Checkpoint) {
        let hook = HOOK.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot
                .as_ref()
                .is_some_and(|hook| hook.path == path && hook.point == point)
            {
                slot.take()
            } else {
                None
            }
        });
        if let Some(hook) = hook {
            (hook.action)();
        }
    }

    /// A per-thread, one-shot rendezvous: the inspector cannot continue until
    /// the injected mutation completes. No timing sleeps or global state races.
    pub fn with_hook<R>(
        path: &Path,
        point: Checkpoint,
        action: impl FnOnce() + 'static,
        inspect: impl FnOnce() -> R,
    ) -> R {
        struct Clear;
        impl Drop for Clear {
            fn drop(&mut self) {
                HOOK.with(|slot| {
                    slot.borrow_mut().take();
                });
            }
        }
        HOOK.with(|slot| {
            assert!(slot.borrow().is_none());
            *slot.borrow_mut() = Some(Hook {
                path: path.into(),
                point,
                action: Box::new(action),
            });
        });
        let _clear = Clear;
        let result = inspect();
        assert!(
            HOOK.with(|slot| slot.borrow().is_none()),
            "inspection never reached {point:?}"
        );
        result
    }

    #[derive(Clone, Copy, Debug)]
    pub enum Change {
        Replace,
        SameSizeRestoredMtime,
        RetargetSymlink,
        Remove,
        InPlace,
    }

    pub fn mutate(path: &Path, change: Change) {
        let before = std::fs::metadata(path).unwrap();
        match change {
            Change::Replace | Change::SameSizeRestoredMtime => {
                let replacement = path.with_extension("replacement");
                let bytes = if matches!(change, Change::SameSizeRestoredMtime) {
                    vec![0; before.len() as usize]
                } else {
                    b"not gzip or Amiga media".to_vec()
                };
                std::fs::write(&replacement, bytes).unwrap();
                if matches!(change, Change::SameSizeRestoredMtime) {
                    File::options()
                        .write(true)
                        .open(&replacement)
                        .unwrap()
                        .set_times(FileTimes::new().set_modified(before.modified().unwrap()))
                        .unwrap();
                    let replacement_metadata = std::fs::metadata(&replacement).unwrap();
                    assert_eq!(before.len(), replacement_metadata.len());
                    assert_eq!(
                        before.modified().unwrap(),
                        replacement_metadata.modified().unwrap()
                    );
                    assert_ne!(before.ino(), replacement_metadata.ino());
                }
                std::fs::rename(replacement, path).unwrap();
            }
            Change::RetargetSymlink => {
                let target = path.with_extension("other-target");
                std::fs::write(&target, vec![0; before.len() as usize]).unwrap();
                let link = path.with_extension("replacement-link");
                symlink(target, &link).unwrap();
                std::fs::rename(link, path).unwrap();
                assert_ne!(before.ino(), std::fs::metadata(path).unwrap().ino());
            }
            Change::Remove => std::fs::remove_file(path).unwrap(),
            Change::InPlace => {
                let mut file = File::options().write(true).open(path).unwrap();
                // Force an observable identity change even when consecutive
                // writes share this filesystem's timestamp granularity.
                file.set_len(before.len() + 1).unwrap();
                file.write_all(&[0x55]).unwrap();
                file.set_times(FileTimes::new().set_modified(before.modified().unwrap()))
                    .unwrap();
                let after = file.metadata().unwrap();
                assert_eq!(before.ino(), after.ino());
                assert_eq!(before.len() + 1, after.len());
                assert_eq!(before.modified().unwrap(), after.modified().unwrap());
            }
        }
    }

    pub fn changed<R>(
        bytes: &[u8],
        change: Change,
        point: Checkpoint,
        inspect: impl FnOnce(&Path) -> R,
    ) -> R {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("media");
        if matches!(change, Change::RetargetSymlink) {
            let target = dir.path().join("original-target");
            std::fs::write(&target, bytes).unwrap();
            symlink(target, &path).unwrap();
        } else {
            std::fs::write(&path, bytes).unwrap();
        }
        let source = path.clone();
        with_hook(
            &path,
            point,
            move || mutate(&source, change),
            || inspect(&path),
        )
    }
}
