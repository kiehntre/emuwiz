//! Linux-only XRoar consumption boundary. Original paths are never reopened by
//! the child: executable, tape and ROM bytes are bounded, checked and sealed.
//! The CAS alias keeps XRoar's required extension. This is not an OS sandbox:
//! host interpreters/libraries and same-UID private-directory tampering remain
//! outside this guarantee. Native XRoar is the supported watched child.

use super::process_spawn::{
    CapturedFileIdentity, PreparedProcessCommand, WatchedProcess, open_regular_file_nonblocking,
    spawn_watched_process_with_assets,
};
use super::xroar_command::XRoarCommand;
use super::xroar_execution::XRoarLaunchRequest;
use crate::identity_source::hashing::Crc32;
use md5::Md5;
use sha1::{Digest, Sha1};
use std::ffi::{CString, OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_EXECUTABLE: u64 = 64 * 1024 * 1024;
const MAX_CASSETTE: u64 = 8 * 1024 * 1024;

fn invalid(detail: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, detail)
}

fn fd_path(file: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

fn above_stdio(file: File) -> io::Result<File> {
    if file.as_raw_fd() >= 3 {
        return Ok(file);
    }
    // SAFETY: file owns the live descriptor; the new CLOEXEC descriptor has
    // independent ownership and cannot be replaced by Command's stdio setup.
    let fd = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[derive(Debug, PartialEq, Eq)]
struct Stamp {
    identity: CapturedFileIdentity,
    mode: u32,
    uid: u32,
    gid: u32,
    ctime: (i64, i64),
}

fn stamp(meta: &fs::Metadata) -> Stamp {
    Stamp {
        identity: CapturedFileIdentity::capture(meta),
        mode: meta.mode(),
        uid: meta.uid(),
        gid: meta.gid(),
        ctime: (meta.ctime(), meta.ctime_nsec()),
    }
}

fn execution_access(source: &File) -> io::Result<()> {
    let meta = source.metadata()?;
    if !meta.is_file() || meta.mode() & 0o111 == 0 || meta.mode() & 0o6000 != 0 {
        return Err(invalid(
            "XRoar must be a regular executable without setuid/setgid bits",
        ));
    }
    // SAFETY: the owned source descriptor and empty C string remain valid.
    // Effective-credential X_OK preserves execute ACL and noexec mount policy;
    // a missing syscall/capability fails closed rather than upgrading access.
    let result = unsafe {
        libc::syscall(
            libc::SYS_faccessat2,
            source.as_raw_fd(),
            c"".as_ptr(),
            libc::X_OK,
            libc::AT_EMPTY_PATH | libc::AT_EACCESS,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn read_consistent(
    path: &Path,
    source: &mut File,
    before: &Stamp,
    limit: u64,
) -> io::Result<Vec<u8>> {
    if before.identity.size > limit {
        return Err(invalid("XRoar launch asset exceeds its copy limit"));
    }
    let mut bytes = Vec::new();
    (&mut *source).take(limit + 1).read_to_end(&mut bytes)?;
    let after = fs::symlink_metadata(path)?;
    if after.file_type().is_symlink()
        || !after.is_file()
        || stamp(&after) != *before
        || stamp(&source.metadata()?) != *before
        || bytes.len() as u64 != before.identity.size
    {
        return Err(invalid(
            "XRoar launch asset changed while its bytes were captured",
        ));
    }
    Ok(bytes)
}

fn snapshot(
    path: &Path,
    limit: u64,
    expected: Option<CapturedFileIdentity>,
    executable: bool,
    validate: impl FnOnce(&[u8]) -> io::Result<()>,
) -> io::Result<(File, File)> {
    let mut source = above_stdio(open_regular_file_nonblocking(path)?)?;
    let before = stamp(&source.metadata()?);
    if expected.is_some_and(|e| e != before.identity) {
        return Err(invalid("cassette identity changed before capture"));
    }
    if executable {
        execution_access(&source)?;
    }
    let bytes = read_consistent(path, &mut source, &before, limit)?;
    validate(&bytes)?;
    if executable {
        execution_access(&source)?;
    }
    // SAFETY: constant name and flags; successful fd is uniquely owned below.
    let fd = unsafe {
        libc::memfd_create(
            c"emuwiz-xroar".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut image = above_stdio(unsafe { File::from_raw_fd(fd) })?;
    image.write_all(&bytes)?;
    // SAFETY: these operate only on our owned anonymous file. Never chmod a
    // source asset. A kernel refusing executable memfds is not bypassed.
    if unsafe { libc::fchmod(image.as_raw_fd(), if executable { 0o500 } else { 0o400 }) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let seals = libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL;
    if unsafe { libc::fcntl(image.as_raw_fd(), libc::F_ADD_SEALS, seals) } < 0
        || unsafe { libc::fcntl(image.as_raw_fd(), libc::F_GET_SEALS) } & seals != seals
    {
        return Err(invalid("XRoar launch asset could not be sealed"));
    }
    Ok((image, source))
}

/// Cleanup is nonrecursive and identity checked. Unknown replacements/content
/// are retained, never recursively treated as our workspace.
#[derive(Debug)]
struct AliasDirectory {
    directory: File,
    parent: File,
    leaf: CString,
    identity: (u64, u64),
    alias_identity: Option<(u64, u64)>,
}

impl AliasDirectory {
    fn create(tape: &File) -> io::Result<Self> {
        // Disable TempDir's path-based recursive cleanup immediately.
        let path = tempfile::Builder::new()
            .prefix("emuwiz-xroar-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()?
            .keep();
        let directory = above_stdio(
            File::options()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&path)?,
        )?;
        let parent = above_stdio(
            File::options()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(
                    path.parent()
                        .ok_or_else(|| invalid("launch directory has no parent"))?,
                )?,
        )?;
        let meta = directory.metadata()?;
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            return Err(invalid("XRoar launch directory is not private"));
        }
        let leaf = CString::new(path.file_name().unwrap().as_bytes())
            .map_err(|_| invalid("invalid launch directory name"))?;
        let mut owned = Self {
            directory,
            parent,
            leaf,
            identity: (meta.dev(), meta.ino()),
            alias_identity: None,
        };
        let target = CString::new(fd_path(tape).as_os_str().as_bytes()).unwrap();
        // SAFETY: directory and strings are owned, fixed local alias only.
        if unsafe {
            libc::symlinkat(
                target.as_ptr(),
                owned.directory.as_raw_fd(),
                c"input.cas".as_ptr(),
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let alias = fs::symlink_metadata(owned.tape_path())?;
        owned.alias_identity = Some((alias.dev(), alias.ino()));
        Ok(owned)
    }

    fn tape_path(&self) -> PathBuf {
        fd_path(&self.directory).join("input.cas")
    }
}

use std::os::unix::fs::OpenOptionsExt;

impl Drop for AliasDirectory {
    fn drop(&mut self) {
        if let Ok(alias) = fs::symlink_metadata(self.tape_path())
            && alias.file_type().is_symlink()
            && self.alias_identity == Some((alias.dev(), alias.ino()))
        {
            // SAFETY: unlink only our known symlink, relative to the owned fd.
            let _ = unsafe { libc::unlinkat(self.directory.as_raw_fd(), c"input.cas".as_ptr(), 0) };
        }
        let parent_path = fd_path(&self.parent).join(OsStr::from_bytes(self.leaf.as_bytes()));
        if let Ok(meta) = fs::symlink_metadata(parent_path)
            && meta.is_dir()
            && (meta.dev(), meta.ino()) == self.identity
        {
            // SAFETY: rmdir is nonrecursive; unexpected content is preserved.
            let _ = unsafe {
                libc::unlinkat(
                    self.parent.as_raw_fd(),
                    self.leaf.as_ptr(),
                    libc::AT_REMOVEDIR,
                )
            };
        }
    }
}

#[derive(Debug)]
struct Assets {
    alias: AliasDirectory,
    executable_source: File,
    images: Vec<File>,
}

#[derive(Debug)]
pub(super) struct BoundXRoarLaunch {
    command: PreparedProcessCommand,
    assets: Arc<Assets>,
}

impl BoundXRoarLaunch {
    pub(super) fn prepare(
        command: &XRoarCommand,
        request: &XRoarLaunchRequest,
        sha1: &str,
    ) -> io::Result<Self> {
        let (executable, executable_source) =
            snapshot(&command.executable, MAX_EXECUTABLE, None, true, |_| Ok(()))?;
        let (tape, _) = snapshot(
            &command.content_path,
            MAX_CASSETTE,
            Some(request.expected_content_identity),
            false,
            |bytes| {
                let hex = |b: &[u8]| b.iter().map(|v| format!("{v:02x}")).collect::<String>();
                if hex(&Sha1::digest(bytes)) != sha1
                    || hex(&Md5::digest(bytes)) != request.expected_content_md5
                    || Crc32::of(bytes) != request.expected_content_crc32
                {
                    return Err(invalid(
                        "captured cassette no longer matches its exact catalogue hashes",
                    ));
                }
                Ok(())
            },
        )?;
        let alias = AliasDirectory::create(&tape)?;
        let mut arguments = command.arguments.clone();
        fn replace(arguments: &mut [OsString], option: &str, path: PathBuf) -> io::Result<()> {
            let index = arguments
                .iter()
                .position(|v| v == option)
                .ok_or_else(|| invalid("bound XRoar command is missing a required asset option"))?;
            let value = arguments
                .get_mut(index + 1)
                .ok_or_else(|| invalid("missing asset path"))?;
            *value = path.into_os_string();
            Ok(())
        }
        replace(&mut arguments, "-load-tape", alias.tape_path())?;
        let executable_path = fd_path(&executable);
        let working_directory = Some(fd_path(&alias.directory));
        let mut images = vec![executable, tape];
        for (option, _, size) in command.machine.cassette_roms() {
            let index = command
                .arguments
                .iter()
                .position(|v| v == *option)
                .ok_or_else(|| invalid("missing machine ROM binding"))?;
            let source = command
                .arguments
                .get(index + 1)
                .ok_or_else(|| invalid("missing ROM path"))?;
            let (image, _) = snapshot(Path::new(source), *size, None, false, |b| {
                if b.len() as u64 != *size {
                    Err(invalid("machine ROM has the wrong size"))
                } else {
                    Ok(())
                }
            })?;
            replace(&mut arguments, option, fd_path(&image))?;
            images.push(image);
        }
        Ok(Self {
            command: PreparedProcessCommand {
                executable: executable_path,
                arguments,
                working_directory,
            },
            assets: Arc::new(Assets {
                alias,
                executable_source,
                images,
            }),
        })
    }

    pub(super) fn spawn(&self) -> io::Result<WatchedProcess> {
        execution_access(&self.assets.executable_source)?;
        let retained = Arc::clone(&self.assets);
        let mut files: Vec<&File> = self.assets.images.iter().collect();
        files.push(&self.assets.alias.directory);
        spawn_watched_process_with_assets(&self.command, &files, move |_| drop(retained))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn snapshots_are_sealed_and_parent_descriptors_stay_cloexec() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("input.cas");
        fs::write(&path, b"original").unwrap();
        let (mut image, source) = snapshot(&path, 32, None, false, |_| Ok(())).unwrap();
        assert!(image.write_all(b"overwrite").is_err());
        assert!(image.set_len(0).is_err());
        assert!(image.set_len(100).is_err());
        for file in [&image, &source] {
            assert!(file.as_raw_fd() > 2);
            assert_ne!(
                unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
                0
            );
        }
        assert_eq!(fs::read(path).unwrap(), b"original");
    }

    #[test]
    fn symlinks_oversized_and_changed_open_files_refuse() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("source");
        let link = root.path().join("link");
        fs::write(&path, b"original").unwrap();
        symlink(&path, &link).unwrap();
        assert!(snapshot(&link, 32, None, false, |_| Ok(())).is_err());
        assert!(snapshot(&path, 2, None, false, |_| Ok(())).is_err());
        let mut file = open_regular_file_nonblocking(&path).unwrap();
        let before = stamp(&file.metadata().unwrap());
        // Do not depend on two writes receiving different timestamps: some
        // filesystems share one coarse timestamp tick. Size changes reliably.
        fs::write(&path, b"modified!").unwrap();
        assert!(read_consistent(&path, &mut file, &before, 32).is_err());
    }

    #[test]
    fn executable_snapshot_cannot_bypass_a_noexec_mount() {
        let root = tempfile::Builder::new()
            .prefix("emuwiz-xroar-noexec-")
            .tempdir_in("/dev/shm")
            .unwrap();
        let path = root.path().join("xroar");
        fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let name = CString::new(root.path().as_os_str().as_bytes()).unwrap();
        let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: a valid C path and output storage for statvfs.
        if unsafe { libc::statvfs(name.as_ptr(), stats.as_mut_ptr()) } != 0 {
            panic!("statvfs failed: {}", io::Error::last_os_error());
        }
        let stats = unsafe { stats.assume_init() };
        if stats.f_flag & libc::ST_NOEXEC == 0 {
            eprintln!("noexec check not exercised: /dev/shm is executable on this host");
            return;
        }
        let error = snapshot(&path, 1024, None, true, |_| Ok(())).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn alias_cleanup_preserves_replaced_directory_and_unknown_content() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("source.cas");
        fs::write(&path, b"original").unwrap();
        let (image, _) = snapshot(&path, 32, None, false, |_| Ok(())).unwrap();
        let alias = AliasDirectory::create(&image).unwrap();
        let original = fs::read_link(fd_path(&alias.directory)).unwrap();
        let moved = root.path().join("moved");
        fs::rename(&original, &moved).unwrap();
        fs::create_dir(&original).unwrap();
        fs::write(original.join("unrelated.txt"), b"keep").unwrap();
        fs::write(moved.join("unexpected.txt"), b"also keep").unwrap();
        drop(alias);
        assert_eq!(fs::read(original.join("unrelated.txt")).unwrap(), b"keep");
        assert_eq!(
            fs::read(moved.join("unexpected.txt")).unwrap(),
            b"also keep"
        );
        assert!(!moved.join("input.cas").exists());
        // The replacement here is entirely our disposable fixture.
        fs::remove_file(original.join("unrelated.txt")).unwrap();
        fs::remove_dir(original).unwrap();
    }
}
