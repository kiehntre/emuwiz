//! Pieces shared by the Linux native adapters (Atari800, b-em, Caprice32,
//! NP2kai, Oricutron) so there is exactly one executable policy and one
//! seed-verification rule, not five copies.
//!
//! # Executable policy
//!
//! The same rule as `emulator_inventory` and `emulator_lifecycle`: the path is
//! absolute and normalised, and its LEAF is a regular file (not a symlink) with
//! an execute bit. Parent directories may be symlinks (`/bin` is one on a
//! merged-/usr system); what is launched and bound is the file itself. The
//! identity `(device, inode, size, mtime)` of that file is captured at planning
//! and compared again immediately before spawn.
//!
//! A symlink on `PATH` (for example `~/.local/bin/caprice32 -> ~/Applications/
//! .../cap32`) is therefore NOT an eligible executable. Discovery does not
//! silently follow it: it reports the link and, when the target is itself an
//! eligible regular executable, that target, so a person can select the real
//! file explicitly. Trust is not widened to make a particular install pass.

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use super::process_spawn::CapturedFileIdentity;
use super::safe_launch_sandbox::{
    MAX_CONFIG_BYTES, MediaKind, MediaMember, MediaRole, SourceProvenance,
};

const MAX_EXPLICIT: usize = 16;
const MAX_PATH_ENTRIES: usize = 64;
const MAX_PATH_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutableRefusal {
    /// Nothing exists at the path.
    Missing,
    /// Relative, non-normalised, not a regular file, a symlink, or not
    /// executable.
    Unsafe,
}

/// Validates the executable policy above and captures the file's identity.
pub fn executable_identity(path: &Path) -> Result<CapturedFileIdentity, ExecutableRefusal> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(ExecutableRefusal::Unsafe);
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| ExecutableRefusal::Missing)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o111 == 0
    {
        return Err(ExecutableRefusal::Unsafe);
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

/// Largest executable that is also content-hashed. Larger files (AppImages)
/// are bound by metadata identity only, and `sha256` is `None`.
pub const MAX_HASHED_EXECUTABLE_BYTES: u64 = 128 * 1024 * 1024;

/// Planning-time binding of an executable: metadata identity AND content hash.
/// `(device, inode, size, mtime)` alone cannot see a same-size replacement
/// that reuses the inode within one filesystem timestamp tick (observed in
/// testing), so the hash is the stronger half.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutableBinding {
    pub identity: CapturedFileIdentity,
    pub sha256: Option<[u8; 32]>,
}

impl ExecutableBinding {
    pub fn capture(path: &Path) -> Result<Self, ExecutableRefusal> {
        let identity = executable_identity(path)?;
        let sha256 = if identity.size <= MAX_HASHED_EXECUTABLE_BYTES {
            Some(hash_executable(path, identity).map_err(|_| ExecutableRefusal::Unsafe)?)
        } else {
            None
        };
        Ok(Self { identity, sha256 })
    }

    /// True only if the path is still an eligible executable with the same
    /// identity and, when bound, the same bytes.
    pub fn is_unchanged(&self, path: &Path) -> bool {
        Self::capture(path).is_ok_and(|now| now == *self)
    }
}

fn hash_executable(path: &Path, identity: CapturedFileIdentity) -> io::Result<[u8; 32]> {
    use sha2::{Digest, Sha256};
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if CapturedFileIdentity::capture(&file.metadata()?) != identity {
        return Err(io::Error::other("executable changed while binding"));
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut remaining = identity.size;
    while remaining > 0 {
        let count = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..count])?;
        hash.update(&buffer[..count]);
        remaining -= count as u64;
    }
    Ok(hash.finalize().into())
}

/// A `PATH` or explicit entry that is a symlink, which the policy refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymlinkedExecutable {
    pub link: PathBuf,
    /// The fully resolved target when it is itself an eligible executable:
    /// the path a person may select explicitly. `None` when it is dangling or
    /// ineligible.
    pub eligible_target: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutableDiscovery {
    /// Eligible executables, sorted and de-duplicated. Never probed or run.
    pub executables: Vec<PathBuf>,
    pub symlink_refusals: Vec<SymlinkedExecutable>,
}

/// Bounded, read-only discovery: explicit paths plus `<PATH entry>/<name>` for
/// each reviewed executable name. No candidate is executed.
pub fn discover_executables(
    explicit: &[PathBuf],
    path_env: Option<&OsStr>,
    names: &[&str],
) -> ExecutableDiscovery {
    let mut candidates: Vec<PathBuf> = explicit.iter().take(MAX_EXPLICIT).cloned().collect();
    if let Some(value) = path_env.filter(|v| v.len() <= MAX_PATH_BYTES) {
        for directory in std::env::split_paths(value)
            .take(MAX_PATH_ENTRIES)
            .filter(|p| p.is_absolute())
        {
            candidates.extend(names.iter().map(|name| directory.join(name)));
        }
    }
    let mut found = ExecutableDiscovery::default();
    for path in candidates {
        match executable_identity(&path) {
            Ok(_) => found.executables.push(path),
            Err(ExecutableRefusal::Unsafe)
                if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) =>
            {
                let eligible_target = fs::canonicalize(&path)
                    .ok()
                    .filter(|target| executable_identity(target).is_ok());
                found.symlink_refusals.push(SymlinkedExecutable {
                    link: path,
                    eligible_target,
                });
            }
            Err(_) => {}
        }
    }
    found.executables.sort();
    found.executables.dedup();
    found.symlink_refusals.sort_by(|a, b| a.link.cmp(&b.link));
    found.symlink_refusals.dedup();
    found
}

/// The first `count` bytes of a planned source, read through a no-follow
/// descriptor and only if it is still the file that was captured.
pub fn read_prefix(source: &SourceProvenance, count: usize) -> io::Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&source.original_path)?;
    if CapturedFileIdentity::capture(&file.metadata()?) != source.original_identity {
        return Err(io::Error::other("source changed since it was captured"));
    }
    let mut bytes = Vec::new();
    file.take(count as u64).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Whether a planned config seed is EXACTLY `expected`: same length and same
/// bytes. A length check alone (or a prefix) is not accepted as a profile.
pub fn seed_is_exact(seed: &SourceProvenance, expected: &str) -> io::Result<bool> {
    if seed.original_identity.size != expected.len() as u64
        || seed.original_identity.size > MAX_CONFIG_BYTES
    {
        return Ok(false);
    }
    // One byte more than expected also catches a file that grew meanwhile.
    Ok(read_prefix(seed, expected.len() + 1)? == expected.as_bytes())
}

pub fn member(path: &Path, role: MediaRole, kind: MediaKind, suffix: &str) -> MediaMember {
    MediaMember {
        source: path.to_owned(),
        role,
        kind,
        suffix: suffix.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn make_executable(path: &Path) {
        fs::write(path, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn a_regular_executable_is_bound_by_identity_and_content() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("emu");
        make_executable(&exe);
        let binding = ExecutableBinding::capture(&exe).unwrap();
        assert!(binding.sha256.is_some());
        assert!(binding.is_unchanged(&exe));
        // A different size is seen by the metadata identity.
        fs::write(
            &exe,
            b"#!/bin/sh
exit 1 # changed
",
        )
        .unwrap();
        assert!(!binding.is_unchanged(&exe));
    }

    #[test]
    fn a_same_size_replacement_with_restored_mtime_is_caught_by_the_hash() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("emu");
        make_executable(&exe);
        let binding = ExecutableBinding::capture(&exe).unwrap();
        let modified = fs::metadata(&exe).unwrap().modified().unwrap();
        let mut bytes = fs::read(&exe).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(&exe, bytes).unwrap();
        fs::File::options()
            .write(true)
            .open(&exe)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        // Metadata identity is unchanged by construction; only content moved.
        assert_eq!(executable_identity(&exe).unwrap(), binding.identity);
        assert!(!binding.is_unchanged(&exe));
        // Replaced by a symlink: refused outright.
        let target = dir.path().join("target");
        make_executable(&target);
        fs::remove_file(&exe).unwrap();
        symlink(&target, &exe).unwrap();
        assert!(!binding.is_unchanged(&exe));
    }

    #[test]
    fn symlinked_non_executable_relative_and_traversing_paths_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("emu");
        make_executable(&exe);
        let link = dir.path().join("link");
        symlink(&exe, &link).unwrap();
        assert_eq!(executable_identity(&link), Err(ExecutableRefusal::Unsafe));
        let plain = dir.path().join("plain");
        fs::write(&plain, b"x").unwrap();
        assert_eq!(executable_identity(&plain), Err(ExecutableRefusal::Unsafe));
        assert_eq!(
            executable_identity(Path::new("emu")),
            Err(ExecutableRefusal::Unsafe)
        );
        assert_eq!(
            executable_identity(&dir.path().join("sub/../emu")),
            Err(ExecutableRefusal::Unsafe)
        );
        assert_eq!(
            executable_identity(&dir.path().join("missing")),
            Err(ExecutableRefusal::Missing)
        );
        assert_eq!(
            executable_identity(dir.path()),
            Err(ExecutableRefusal::Unsafe)
        );
    }

    #[test]
    fn a_symlinked_parent_directory_is_accepted_like_the_existing_policy() {
        // /bin -> usr/bin on merged-usr systems: the launched file is regular.
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        make_executable(&real.join("emu"));
        symlink(&real, dir.path().join("alias")).unwrap();
        assert!(executable_identity(&dir.path().join("alias/emu")).is_ok());
    }

    #[test]
    fn discovery_reports_a_path_symlink_with_its_eligible_target_instead_of_following_it() {
        let dir = tempfile::tempdir().unwrap();
        let apps = dir.path().join("apps");
        let bin = dir.path().join("bin");
        let other = dir.path().join("other");
        for d in [&apps, &bin, &other] {
            fs::create_dir(d).unwrap();
        }
        make_executable(&apps.join("cap32"));
        symlink(apps.join("cap32"), bin.join("caprice32")).unwrap();
        make_executable(&other.join("cap32"));
        symlink(dir.path().join("nowhere"), bin.join("cap32")).unwrap();
        let path = std::env::join_paths([&bin, &other]).unwrap();
        let found = discover_executables(&[], Some(&path), &["caprice32", "cap32"]);
        // Only the regular file is eligible; both links are reported.
        assert_eq!(found.executables, vec![other.join("cap32")]);
        assert_eq!(found.symlink_refusals.len(), 2);
        let resolved = found
            .symlink_refusals
            .iter()
            .find(|l| l.link == bin.join("caprice32"))
            .unwrap();
        assert_eq!(
            resolved.eligible_target.as_deref(),
            Some(fs::canonicalize(apps.join("cap32")).unwrap().as_path())
        );
        assert!(
            found
                .symlink_refusals
                .iter()
                .find(|l| l.link == bin.join("cap32"))
                .unwrap()
                .eligible_target
                .is_none()
        );
        // Selecting the resolved target explicitly is accepted.
        let explicit = discover_executables(
            &[resolved.eligible_target.clone().unwrap()],
            None,
            &["caprice32"],
        );
        assert_eq!(explicit.executables.len(), 1);
    }

    #[test]
    fn discovery_is_bounded_and_ignores_relative_path_entries() {
        let dir = tempfile::tempdir().unwrap();
        make_executable(&dir.path().join("emu"));
        let path =
            std::env::join_paths([PathBuf::from("relative"), dir.path().to_owned()]).unwrap();
        let found = discover_executables(&[], Some(&path), &["emu"]);
        assert_eq!(found.executables, vec![dir.path().join("emu")]);
        let huge = "/a:".repeat(MAX_PATH_BYTES);
        assert!(
            discover_executables(&[], Some(OsStr::new(&huge)), &["emu"])
                .executables
                .is_empty()
        );
    }
}
