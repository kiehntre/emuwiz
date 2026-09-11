//! Shared, persisted object/content binding for apply, rollback and recovery.
//!
//! Capture reads the whole regular file through one descriptor, verifies its
//! metadata before/after reading and rechecks the pathname. A symlink binds
//! its own target text, never the referred file. Comparison requires the same
//! object, SHA256 and full-precision mtime; legacy proofs fail closed.
//!
//! This detects stale evidence when a binding is carried across stages. It
//! cannot recover evidence a caller discarded before capture, serialize jobs,
//! or eliminate the check/use window in a subsequent pathname syscall.

use std::fs::{Metadata, OpenOptions};
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

use super::model::{ObjectFreshness, ObjectIdentity, ObjectKind};

const FRESHNESS_VERSION: u32 = 1;

/// Captures `path` without following a leaf symlink. Regular files cost one
/// complete read with fixed memory and a byte bound fixed by the initial
/// metadata. Unreadable or changing files return an error, never a weaker
/// metadata-only proof. Special files are classified without opening them.
pub fn capture_identity(path: &Path) -> std::io::Result<ObjectIdentity> {
    let metadata = std::fs::symlink_metadata(path)?;
    let kind = classify_metadata(path, &metadata);
    let freshness = match kind {
        ObjectKind::RegularFile => Some(capture_regular(path, &metadata)?),
        ObjectKind::Symlink | ObjectKind::BrokenSymlink => {
            let target = std::fs::read_link(path)?;
            #[cfg(unix)]
            let bytes = std::os::unix::ffi::OsStrExt::as_bytes(target.as_os_str());
            #[cfg(not(unix))]
            let bytes = target.as_os_str().as_encoded_bytes();
            let proof = ObjectFreshness {
                version: FRESHNESS_VERSION,
                modified: metadata.modified()?,
                sha256: Sha256::digest(bytes).into(),
            };
            require_same_snapshot(&metadata, &std::fs::symlink_metadata(path)?)?;
            Some(proof)
        }
        ObjectKind::Other => None,
    };
    let modified_unix = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0);
    let identity = ObjectIdentity {
        size_bytes: metadata.len(),
        modified_unix,
        kind,
        #[cfg(unix)]
        ino: std::os::unix::fs::MetadataExt::ino(&metadata),
        #[cfg(unix)]
        dev: std::os::unix::fs::MetadataExt::dev(&metadata),
        freshness,
    };
    Ok(identity)
}

/// Requires matching supported freshness proofs on BOTH sides. Legacy
/// metadata-only journals remain inspectable, but cannot authorize apply,
/// reverse mutation or recovery confirmation. Re-capturing such a journal's
/// baseline would bind the very replacement this check exists to reject.
pub fn identity_matches(expected: &ObjectIdentity, current: &ObjectIdentity) -> bool {
    if expected.kind != current.kind
        || expected.size_bytes != current.size_bytes
        || expected.modified_unix != current.modified_unix
    {
        return false;
    }
    #[cfg(unix)]
    {
        if expected.ino != current.ino || expected.dev != current.dev {
            return false;
        }
    }
    matches!((&expected.freshness, &current.freshness), (Some(a), Some(b))
        if a.version == FRESHNESS_VERSION && b.version == FRESHNESS_VERSION && a == b)
}

fn capture_regular(path: &Path, before: &Metadata) -> io::Result<ObjectFreshness> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // NONBLOCK prevents a file swapped for a FIFO after lstat from
        // blocking the worker before fstat can reject the replacement.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let mut file = options.open(path)?;
    require_same_snapshot(before, &file.metadata()?)?;
    let mut digest = Sha256::new();
    let mut bytes_read = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    let mut bounded = (&mut file).take(before.len().saturating_add(1));
    loop {
        let count = match bounded.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            break;
        }
        bytes_read += count as u64;
        digest.update(&buffer[..count]);
    }
    if bytes_read != before.len() {
        return Err(changed());
    }
    require_same_snapshot(before, &file.metadata()?)?;
    require_same_snapshot(before, &std::fs::symlink_metadata(path)?)?;
    Ok(ObjectFreshness {
        version: FRESHNESS_VERSION,
        modified: before.modified()?,
        sha256: digest.finalize().into(),
    })
}

fn changed() -> io::Error {
    io::Error::other("object changed while capturing its content binding")
}

/// ctime detects edits with restored mtime *during* capture on Unix. It is
/// intentionally not compared across a completed rename or rollback.
fn require_same_snapshot(before: &Metadata, after: &Metadata) -> io::Result<()> {
    if before.file_type() != after.file_type()
        || before.len() != after.len()
        || before.modified()? != after.modified()?
    {
        return Err(changed());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (
            before.dev(),
            before.ino(),
            before.ctime(),
            before.ctime_nsec(),
        ) != (after.dev(), after.ino(), after.ctime(), after.ctime_nsec())
        {
            return Err(changed());
        }
    }
    Ok(())
}

/// The identity of a *symlink itself* is deliberately never the identity of
/// its target: [`capture_identity`] uses `symlink_metadata`, so a symlink's
/// inode is its own. A source swapped for a symlink therefore never matches a
/// recorded regular-file identity.
///
/// Classifies `path` into [`ObjectKind`], distinguishing a broken symlink.
pub fn classify_at(path: &Path) -> std::io::Result<ObjectKind> {
    let metadata = std::fs::symlink_metadata(path)?;
    Ok(classify_metadata(path, &metadata))
}

fn classify_metadata(path: &Path, metadata: &Metadata) -> ObjectKind {
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        if std::fs::metadata(path).is_ok() {
            ObjectKind::Symlink
        } else {
            ObjectKind::BrokenSymlink
        }
    } else if file_type.is_file() {
        ObjectKind::RegularFile
    } else {
        ObjectKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_uses_symlink_metadata_not_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.bin");
        std::fs::write(&target, b"hello").unwrap();
        let link = dir.path().join("link.bin");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let target_identity = capture_identity(&target).unwrap();
        let link_identity = capture_identity(&link).unwrap();
        // A symlink's own inode is never the target's.
        #[cfg(unix)]
        {
            assert_ne!(target_identity.ino, link_identity.ino);
        }
        assert_eq!(link_identity.kind, ObjectKind::Symlink);
        assert_eq!(classify_at(&link).unwrap(), ObjectKind::Symlink);
        assert_eq!(classify_at(&target).unwrap(), ObjectKind::RegularFile);
    }

    #[test]
    fn a_broken_symlink_is_classified() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("broken.bin");
        std::os::unix::fs::symlink(dir.path().join("nowhere"), &link).unwrap();
        assert_eq!(classify_at(&link).unwrap(), ObjectKind::BrokenSymlink);
    }

    #[test]
    fn identity_matches_itself_and_rejects_a_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.bin");
        std::fs::write(&path, b"one").unwrap();
        let first = capture_identity(&path).unwrap();
        assert!(identity_matches(&first, &first));

        // A different-size rewrite changes the identity.
        std::fs::write(&path, b"a much longer payload").unwrap();
        let resized = capture_identity(&path).unwrap();
        assert!(!identity_matches(&first, &resized), "size changed");

        // A different file with the same size and kind is distinguished by its
        // inode/device where supported.
        let other = dir.path().join("other.bin");
        std::fs::write(&other, b"one").unwrap();
        let other_identity = capture_identity(&other).unwrap();
        assert!(
            !identity_matches(&first, &other_identity),
            "a different object must not match even with identical size"
        );
    }

    #[test]
    fn a_symlink_substitution_never_matches_a_regular_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.bin");
        std::fs::write(&path, b"one").unwrap();
        let regular = capture_identity(&path).unwrap();

        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(dir.path().join("elsewhere"), &path).unwrap();
        let substituted = capture_identity(&path).unwrap();
        assert!(!identity_matches(&regular, &substituted));
    }

    #[test]
    fn replacement_between_stat_and_open_is_refused_even_with_identical_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source");
        std::fs::write(&path, b"same bytes").unwrap();
        let before = std::fs::symlink_metadata(&path).unwrap();
        std::fs::rename(&path, dir.path().join("retained")).unwrap();
        std::fs::write(&path, b"same bytes").unwrap();
        assert!(capture_regular(&path, &before).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn leaf_symlink_between_stat_and_open_is_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source");
        let retained = dir.path().join("retained");
        std::fs::write(&path, b"same object").unwrap();
        let before = std::fs::symlink_metadata(&path).unwrap();
        std::fs::rename(&path, &retained).unwrap();
        std::os::unix::fs::symlink(&retained, &path).unwrap();
        assert!(capture_regular(&path, &before).is_err());
    }

    #[test]
    fn unchanged_content_binding_survives_rename_and_journal_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let before_path = dir.path().join("source");
        let after_path = dir.path().join("destination");
        std::fs::write(&before_path, b"rename preserves content and mtime").unwrap();
        let before = capture_identity(&before_path).unwrap();
        std::fs::rename(&before_path, &after_path).unwrap();
        let persisted: ObjectIdentity =
            serde_json::from_slice(&serde_json::to_vec(&before).unwrap()).unwrap();
        assert!(identity_matches(
            &persisted,
            &capture_identity(&after_path).unwrap()
        ));
    }

    #[test]
    fn special_file_or_directory_never_has_a_mutation_proof() {
        let dir = tempfile::tempdir().unwrap();
        let identity = capture_identity(dir.path()).unwrap();
        assert_eq!(identity.kind, ObjectKind::Other);
        assert!(!identity_matches(&identity, &identity));
    }
}
