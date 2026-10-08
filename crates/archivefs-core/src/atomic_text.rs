//! Owned temporary staging for the existing atomic text replacement API.
//!
//! Creation is exclusive; writes, permissions and file synchronization use the
//! retained descriptor. An observed name/identity mismatch refuses publication
//! and cleanup. Entry operations use one retained parent directory descriptor;
//! observed parent rebinding refuses or reports publication into that original
//! directory explicitly. This is not a writer lock or protection against every
//! hostile same-UID leaf check/use race. Directory sync remains best effort.

mod parent_dir;

use std::fs::{self, Metadata, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use crate::{ArchiveFsError, Result};
use parent_dir::{Leaf, ParentDir};

struct OwnedStage<'a> {
    temporary: tempfile::NamedTempFile,
    identity: (u64, u64),
    parent: &'a ParentDir,
    name: Leaf,
}

impl<'a> OwnedStage<'a> {
    fn create(
        parent: &'a ParentDir,
        path: &Path,
        existing_permissions: Option<&Permissions>,
    ) -> io::Result<Self> {
        let creation_permissions = existing_permissions
            .cloned()
            .unwrap_or_else(|| Permissions::from_mode(0o666));
        let mut builder = tempfile::Builder::new();
        builder
            .prefix(".archivefs-config-write-")
            .suffix(".tmp")
            .rand_bytes(12)
            // An existing restricted destination must not start with a broader
            // staging mode. New files retain File::create's 0666 request. The
            // kernel applies umask/default ACLs; never change process umask.
            // A pathname destructor cannot establish ownership after a swap.
            // Cleanup below is explicit; panic/process death retains the stage.
            .disable_cleanup(true);
        #[cfg(test)]
        tests::configure_builder(&mut builder);
        #[cfg(test)]
        tests::event(Event::BeforeStageCreate, path)?;
        // Builder supplies names/retries only. Creation and mode application
        // are kernel operations beneath the pinned directory, with its umask
        // and default ACL behavior; the diagnostic TempPath never cleans up.
        let temporary = builder.make_in(path, |candidate| {
            parent.create_stage(&Leaf::from_path(candidate)?, creation_permissions.mode())
        })?;
        let metadata = temporary.as_file().metadata().map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "cannot identify created temporary {}; retained for inspection: {error}",
                    temporary.path().display()
                ),
            )
        })?;
        Ok(Self {
            identity: (metadata.dev(), metadata.ino()),
            name: Leaf::from_path(temporary.path())?,
            parent,
            temporary,
        })
    }

    fn path(&self) -> &Path {
        self.temporary.path()
    }

    fn matches(&self, metadata: &Metadata) -> bool {
        metadata.is_file()
            && metadata.nlink() == 1
            && (metadata.dev(), metadata.ino()) == self.identity
    }

    fn require_owned_name(&self) -> io::Result<()> {
        let held = self.temporary.as_file().metadata()?;
        let named = self.parent.metadata(&self.name)?;
        if !self.matches(&held)
            || named.st_mode & libc::S_IFMT != libc::S_IFREG
            || named.st_nlink != 1
            || (named.st_dev as u64, named.st_ino as u64) != self.identity
        {
            return Err(io::Error::other(
                "temporary entry no longer identifies the exclusively owned staging file",
            ));
        }
        Ok(())
    }

    fn set_permissions(&self, permissions: Permissions) -> io::Result<()> {
        #[cfg(test)]
        tests::event(Event::BeforePermissions, self.path())?;
        self.temporary.as_file().set_permissions(permissions)
    }

    fn cleanup(&self) -> io::Result<()> {
        #[cfg(test)]
        tests::event(Event::BeforeCleanup, self.path())?;
        // This observation is not an atomic conditional unlink. A trusted
        // namespace is still needed to exclude hostile changes after it.
        self.require_owned_name()?;
        #[cfg(test)]
        tests::event(Event::BeforeUnlink, self.path())?;
        self.parent.remove_stage(&self.name)
    }

    fn failure(&self, primary: ArchiveFsError) -> ArchiveFsError {
        match self.cleanup() {
            Ok(()) => primary,
            Err(cleanup) => {
                let kind = match &primary {
                    ArchiveFsError::Io { source, .. } => source.kind(),
                    _ => io::ErrorKind::Other,
                };
                ArchiveFsError::io(
                    self.path(),
                    io::Error::new(
                        kind,
                        format!(
                            "{primary}; temporary cleanup refused or failed ({cleanup}); inspect the destination and temporary path {}",
                            self.path().display()
                        ),
                    ),
                )
            }
        }
    }
}

fn target_permissions(parent: &ParentDir, name: &Leaf, path: &Path) -> Result<Option<Permissions>> {
    match parent.metadata(name) {
        Ok(metadata) if metadata.st_mode & libc::S_IFMT == libc::S_IFREG => {
            Ok(Some(Permissions::from_mode(metadata.st_mode as u32)))
        }
        Ok(_) => Err(ArchiveFsError::Config(format!(
            "refusing to overwrite config path that is not a regular file: {}",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ArchiveFsError::io(path, error)),
    }
}

pub(super) fn write(path: &Path, contents: &str) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        ArchiveFsError::Config(format!("config path has no parent: {}", path.display()))
    })?;
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let name = Leaf::from_path(path).map_err(|error| ArchiveFsError::io(path, error))?;
    fs::create_dir_all(parent).map_err(|error| ArchiveFsError::io(parent, error))?;
    let directory = ParentDir::open(parent).map_err(|error| ArchiveFsError::io(parent, error))?;
    directory
        .require_binding()
        .map_err(|error| ArchiveFsError::io(parent, error))?;
    let initial_permissions = target_permissions(&directory, &name, path)?;
    // No OwnedStage exists on failure, so a failed exclusive create never
    // grants authority to remove any pre-existing candidate name.
    let mut stage = OwnedStage::create(&directory, parent, initial_permissions.as_ref())
        .map_err(|error| ArchiveFsError::io(parent, error))?;
    let result = (|| {
        #[cfg(test)]
        tests::event(Event::Created, stage.path())
            .map_err(|error| ArchiveFsError::io(stage.path(), error))?;
        directory
            .require_binding()
            .map_err(|error| ArchiveFsError::io(parent, error))?;
        stage
            .require_owned_name()
            .map_err(|error| ArchiveFsError::io(stage.path(), error))?;
        if let Some(permissions) = initial_permissions {
            // Preserve a restricted existing mode before staging its contents.
            stage
                .set_permissions(permissions)
                .map_err(|error| ArchiveFsError::io(stage.path(), error))?;
        }
        let stage_path = stage.path().to_owned();
        stage
            .temporary
            .as_file_mut()
            .write_all(contents.as_bytes())
            .and_then(|()| stage.temporary.as_file_mut().flush())
            .map_err(|error| ArchiveFsError::io(&stage_path, error))?;
        if let Some(permissions) = target_permissions(&directory, &name, path)? {
            // Also preserve a legitimate regular destination's current mode.
            // Failure is a refusal, rather than publication with default mode.
            stage
                .set_permissions(permissions)
                .map_err(|error| ArchiveFsError::io(stage.path(), error))?;
        }
        #[cfg(test)]
        tests::event(Event::BeforeSync, stage.path())
            .map_err(|error| ArchiveFsError::io(stage.path(), error))?;
        stage
            .temporary
            .as_file()
            .sync_all()
            .map_err(|error| ArchiveFsError::io(stage.path(), error))?;
        #[cfg(test)]
        tests::event(Event::BeforePublish, stage.path())
            .map_err(|error| ArchiveFsError::io(stage.path(), error))?;
        directory
            .require_binding()
            .map_err(|error| ArchiveFsError::io(parent, error))?;
        stage
            .require_owned_name()
            .map_err(|error| ArchiveFsError::io(stage.path(), error))?;
        // Preserve the existing post-sync destination eligibility refusal.
        // Modes are applied before sync; this is still a check/use boundary,
        // not destination identity custody or serialization of other writers.
        let _ = target_permissions(&directory, &name, path)?;
        // Preserve the existing intentional regular-file replacement contract.
        // A failed rename is not universal proof of no publication (e.g. NFS).
        #[cfg(test)]
        tests::event(Event::BeforeRename, stage.path())
            .map_err(|error| ArchiveFsError::io(stage.path(), error))?;
        directory
            .replace(&stage.name, &name)
            .map_err(|error| ArchiveFsError::io(path, error))?;
        Ok(())
    })();
    result.map_err(|error| stage.failure(error))?;
    // The stage name was consumed by rename. A later error must not attempt
    // cleanup of that name, which could now belong to a different operation.
    #[cfg(test)]
    tests::event(Event::AfterPublish, stage.path())
        .map_err(|error| ArchiveFsError::io(path, error))?;
    directory.sync_best_effort();
    directory.require_binding().map_err(|error| ArchiveFsError::io(path,
        io::Error::new(error.kind(), format!(
            "publication completed in the pinned original directory, but its pathname binding was lost; inspect the original directory and destination before recovery or retry: {error}"
        ))))
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    BeforeStageCreate,
    Created,
    BeforePermissions,
    BeforeSync,
    BeforePublish,
    BeforeCleanup,
    BeforeRename,
    BeforeUnlink,
    AfterPublish,
}

#[cfg(test)]
mod tests;
