use super::*;
use std::os::unix::fs::PermissionsExt;

#[derive(Debug, Serialize)]
pub(crate) struct Eligibility {
    pub(crate) filesystem_magic: libc::c_long,
    pub(crate) filesystem_type: String,
    pub(crate) root_identity: (u64, u64),
    pub(crate) generation_available: bool,
    pub(crate) reflink_available: bool,
}
fn mount_id(file: &File) -> io::Result<u64> {
    fs::read_to_string(format!("/proc/self/fdinfo/{}", file.as_raw_fd()))?
        .lines()
        .find_map(|line| {
            line.strip_prefix("mnt_id:")
                .and_then(|v| v.trim().parse().ok())
        })
        .ok_or_else(|| io::Error::other("mount identity unavailable"))
}
pub(super) fn same_mount(a: &File, b: &File) -> io::Result<()> {
    if mount_id(a)? != mount_id(b)? {
        return Err(io::Error::other(
            "different mount identities refused before capture",
        ));
    }
    Ok(())
}
/// Disposable same-root capability probe, never a production activation grant.
/// Only the locally exercised ext4 path is eligible for this candidate. Other
/// filesystems require their own evidence, even if individual syscalls work.
pub(crate) fn root_profile(root: &Path) -> io::Result<(File, libc::statfs, String)> {
    if fs::canonicalize(root)? != root {
        return Err(io::Error::other("root must be canonical"));
    }
    let dir = File::open(root)?;
    if crate::dat::rename_apply::is_private_recovery_path(root) {
        return Err(io::Error::other(
            "a recovery namespace cannot be a collection root",
        ));
    }
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::fstatfs(dir.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };
    // ext2/ext3/ext4 share this statfs magic. Bind the mountinfo type to the
    // already-open root FD; the magic alone must not admit untested roots.
    let mount = mount_id(&dir)?;
    let mountinfo = fs::read_to_string("/proc/self/mountinfo")?;
    let filesystem_type = mountinfo
        .lines()
        .find_map(|line| {
            let (fields, filesystem) = line.split_once(" - ")?;
            if fields.split_whitespace().next()?.parse::<u64>().ok()? == mount {
                filesystem.split_whitespace().next()
            } else {
                None
            }
        })
        .ok_or_else(|| io::Error::other("root filesystem type unavailable"))?;
    if stat.f_type != 0xef53 || filesystem_type != "ext4" || stat.f_bsize != 4096 {
        return Err(io::Error::other(
            "filesystem has not passed this candidate's eligibility tests",
        ));
    }
    Ok((dir, stat, filesystem_type.to_owned()))
}
pub(crate) fn probe(root: &Path) -> io::Result<Eligibility> {
    let (dir, stat, filesystem_type) = root_profile(root)?;
    private_dir(&root.join(".emuwiz-r2-holding"), true)?;
    let probe = tempfile::Builder::new()
        .prefix("probe-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(root.join(".emuwiz-r2-holding"))?
        .keep();
    let a = probe.join("a");
    let published = probe.join("published");
    let moved = probe.join("moved");
    fs::write(&a, b"probe A")?;
    File::open(&a)?.sync_all()?;
    let expected = capture_identity(&a)?;
    let pinned = Publisher::open(root, &a, &published, &expected)?;
    let dirs = RenameDirs::open_in(&a, &moved, &TrustedRoots::from_paths([root]))?;
    dirs.rename_noreplace(false).map_err(io::Error::other)?;
    dirs.sync_and_verify(false)?;
    if !matches!(pinned.publish(|| {})?, Publication::LinkedVerified) {
        return Err(io::Error::other("descriptor publication probe failed"));
    }
    match pinned.publish(|| {})? {
        Publication::LinkError(e) if e.raw_os_error() == Some(libc::EEXIST) => (),
        _ => return Err(io::Error::other("link no-overwrite probe failed")),
    }
    let no_replace = RenameDirs::open_in(&moved, &published, &TrustedRoots::from_paths([root]))?;
    if no_replace.rename_noreplace(false).is_ok() {
        return Err(io::Error::other("rename no-overwrite probe failed"));
    }
    if !identity_matches(&expected, &capture_identity(&moved)?)
        || !identity_matches(&expected, &capture_identity(&published)?)
    {
        return Err(io::Error::other("capability probe identity changed"));
    }
    same_mount(&dir, &File::open(&probe)?)?;
    let mut generation: libc::c_long = 0;
    let generation_available = unsafe {
        libc::ioctl(
            pinned.file.as_raw_fd(),
            libc::FS_IOC_GETVERSION,
            &mut generation,
        )
    } == 0;
    let clone = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(probe.join("clone"))?;
    let reflink_available =
        unsafe { libc::ioctl(clone.as_raw_fd(), libc::FICLONE, pinned.file.as_raw_fd()) } == 0;
    let report = Eligibility {
        filesystem_magic: stat.f_type,
        filesystem_type: filesystem_type.into(),
        root_identity: (dir.metadata()?.dev(), dir.metadata()?.ino()),
        generation_available,
        reflink_available,
    };
    let mut report_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(probe.join("probe-report.json"))?;
    report_file.write_all(&serde_json::to_vec(&report).map_err(io::Error::other)?)?;
    report_file.sync_all()?;
    // Keep creation FDs alive, and bind cleanup to those objects rather than
    // adopting whatever might later occupy a probe pathname.
    let owned_snapshot = |name: &str, file: &File| -> io::Result<ObjectIdentity> {
        let before = file.metadata()?;
        let observed = capture_identity(&probe.join(name))?;
        super::super::require_same_snapshot(&before, &file.metadata()?)?;
        if (observed.dev, observed.ino) != (before.dev(), before.ino()) {
            return Err(io::Error::other(
                "probe creation object was replaced; retain evidence",
            ));
        }
        Ok(observed)
    };
    let directory = File::open(&probe)?;
    let expected_files = vec![
        ("moved".to_owned(), expected.clone()),
        ("published".to_owned(), expected.clone()),
        ("clone".to_owned(), owned_snapshot("clone", &clone)?),
        (
            "probe-report.json".to_owned(),
            owned_snapshot("probe-report.json", &report_file)?,
        ),
    ];
    cleanup_owned_probe(
        &File::open(root.join(".emuwiz-r2-holding"))?,
        &directory,
        &probe,
        &expected_files,
    )?;
    Ok(report)
}

pub(crate) fn inventory(root: &Path, tx: &RenameTransaction) -> io::Result<serde_json::Value> {
    let mut entries = Vec::new();
    let mut unique = std::collections::BTreeMap::new();
    for (index, entry) in tx.entries.iter().enumerate() {
        let evidence = evidence(tx, index)?;
        let (held, salvage) = paths(root, tx, index)?;
        for (kind, path) in [
            ("held", held),
            ("salvage", salvage),
            ("anchor", anchor::path(root, tx, index)?),
        ] {
            match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    unique.insert((metadata.dev(), metadata.ino()), metadata.blocks() * 512);
                    let shares_library_inode = [&entry.source_path, &entry.destination_path]
                        .iter()
                        .any(|p| {
                            fs::symlink_metadata(p).is_ok_and(|m| {
                                (m.dev(), m.ino()) == (metadata.dev(), metadata.ino())
                            })
                        });
                    entries.push(serde_json::json!({"transaction":tx.transaction_id,"entry":index,"kind":kind,"path":path,"device":metadata.dev(),"inode":metadata.ino(),"links":metadata.nlink(),"allocated_bytes":metadata.blocks()*512,"shares_known_library_inode":shares_library_inode,"needs_review":evidence.as_ref().is_none_or(|v|v.phase!=Phase::RetainedForeign),"retained_by_signed_decision":evidence.as_ref().is_some_and(|v|v.phase==Phase::RetainedForeign),"evidence":evidence}));
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                Err(e) => return Err(e),
            }
        }
    }
    Ok(
        serde_json::json!({"scope":"single transaction; excludes probe and unowned entries","retained_entries":entries.len(),"unique_inodes":unique.len(),"allocated_bytes_unique_inodes_upper_bound":unique.values().sum::<u64>(),"incremental_data_allocation_for_hardlink":0,"reclaimable_bytes":null,"entries":entries}),
    )
}

/// Narrow cleanup of a NEW successful probe under Contract 1-I trusted private
/// storage. No recursive removal and no adoption of pre-existing probe names.
fn cleanup_owned_probe(
    parent: &File,
    dir: &File,
    path: &Path,
    expected: &[(String, ObjectIdentity)],
) -> io::Result<()> {
    let name = basename(path)?;
    let bound = PathBuf::from(format!(
        "/proc/self/fd/{}/{}",
        parent.as_raw_fd(),
        path.file_name().unwrap().to_string_lossy()
    ));
    let metadata = fs::symlink_metadata(&bound)?;
    let pinned = dir.metadata()?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || (metadata.dev(), metadata.ino()) != (pinned.dev(), pinned.ino())
        || metadata.dev() != parent.metadata()?.dev()
    {
        return Err(io::Error::other(
            "probe identity/ownership changed; preserve for review",
        ));
    }
    same_mount(parent, dir)?;
    let root = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()));
    let mut names = fs::read_dir(&root)?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<io::Result<Vec<_>>>()?;
    names.sort();
    let mut approved = expected.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    approved.sort();
    if names != approved {
        return Err(io::Error::other(
            "unexpected probe contents retained for review",
        ));
    }
    for (name, id) in expected {
        if !identity_matches(id, &capture_identity(&root.join(name))?) {
            return Err(io::Error::other("probe file changed; retain all evidence"));
        }
    }
    // The private directory is trusted by contract. These are NOT inode-conditional unlinks.
    for (name, id) in expected {
        if !identity_matches(id, &capture_identity(&root.join(name))?) {
            return Err(io::Error::other(
                "probe changed before cleanup; retain remaining evidence",
            ));
        }
        let name = CString::new(name.as_str()).map_err(io::Error::other)?;
        if unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    dir.sync_all()?;
    let m = fs::symlink_metadata(&bound)?;
    if (m.dev(), m.ino()) != (pinned.dev(), pinned.ino()) || !m.is_dir() {
        return Err(io::Error::other("probe directory changed; retain evidence"));
    }
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
        return Err(io::Error::last_os_error());
    }
    parent.sync_all()
}

#[cfg(test)]
mod owned_probe_tests {
    use super::*;
    #[test]
    fn changed_unknown_symlink_or_rebound_probe_is_preserved() {
        for attack in ["changed", "unknown", "symlink", "rebound"] {
            let home = tempfile::tempdir().unwrap();
            let path = home.path().join("probe-owned");
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            let file = path.join("owned");
            fs::write(&file, b"owned probe").unwrap();
            let expected = vec![("owned".into(), capture_identity(&file).unwrap())];
            let parent = File::open(home.path()).unwrap();
            let pinned = File::open(&path).unwrap();
            match attack {
                "changed" => fs::write(&file, b"changed").unwrap(),
                "unknown" => fs::write(path.join("foreign"), b"foreign").unwrap(),
                "symlink" => {
                    fs::remove_file(&file).unwrap();
                    fs::write(home.path().join("outside"), b"outside").unwrap();
                    std::os::unix::fs::symlink(home.path().join("outside"), &file).unwrap();
                }
                _ => {
                    fs::rename(&path, home.path().join("saved")).unwrap();
                    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
                    fs::write(path.join("foreign"), b"foreign").unwrap();
                }
            }
            assert!(
                cleanup_owned_probe(&parent, &pinned, &path, &expected).is_err(),
                "{attack}"
            );
            assert!(path.exists());
            if attack == "symlink" {
                assert_eq!(fs::read(home.path().join("outside")).unwrap(), b"outside");
            }
            if attack == "rebound" {
                assert_eq!(
                    fs::read(home.path().join("saved/owned")).unwrap(),
                    b"owned probe"
                );
            }
        }
    }
    #[test]
    fn new_probe_cleanup_never_adopts_existing_probe_directories() {
        let root = tempfile::tempdir().unwrap();
        private_dir(&root.path().join(".emuwiz-r2-holding"), true).unwrap();
        let old = root.path().join(".emuwiz-r2-holding/probe-existing");
        fs::DirBuilder::new().mode(0o700).create(&old).unwrap();
        fs::write(old.join("evidence"), b"keep old probe").unwrap();
        probe(root.path()).unwrap();
        assert_eq!(fs::read(old.join("evidence")).unwrap(), b"keep old probe");
        assert_eq!(
            fs::read_dir(root.path().join(".emuwiz-r2-holding"))
                .unwrap()
                .count(),
            1
        );
    }
}
