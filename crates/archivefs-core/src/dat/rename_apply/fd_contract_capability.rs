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
pub(crate) fn probe(root: &Path) -> io::Result<Eligibility> {
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
    fs::write(
        probe.join("probe-report.json"),
        serde_json::to_vec(&report).map_err(io::Error::other)?,
    )?;
    // Probe artifacts are retained, identified as probes, never game entries or
    // a second recovery authority. No automatic collection of holding data.
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
                    entries.push(serde_json::json!({"transaction":tx.transaction_id,"entry":index,"kind":kind,"path":path,"device":metadata.dev(),"inode":metadata.ino(),"links":metadata.nlink(),"allocated_bytes":metadata.blocks()*512,"shares_known_library_inode":shares_library_inode,"needs_review":true,"evidence":evidence}));
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
