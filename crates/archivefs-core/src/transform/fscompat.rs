//! Which filesystems the engine's durability claims hold on. Fail closed.
//!
//! The protocol (and its model checker) relies on D1–D5 (see
//! `model_check`): explicit directory-fsync durability, atomic rename,
//! per-name linear crash history, file-data fsync and SQLite durability.
//! This release supports exactly one filesystem configuration where those
//! are documented properties:
//!
//! - **ext4**, read-write, with the jbd2 journal **in use** (no `norecovery`
//!   or `noload`), write **barriers on** (`barrier`, so `fsync` reaches the
//!   device), `data=ordered` or `data=journal`, and **`errors=remount-ro`**
//!   (ext4 stops writing as soon as it detects damage to its own metadata,
//!   instead of continuing over it).
//!
//! The option is a prerequisite, not a substitute: the engine still fsyncs
//! every file and directory explicitly, journals every step and re-verifies
//! content. Under [`FsPolicy::TestScratch`] (selected only by a test-only
//! `ScratchAuthority`, never by anything in a folder) `errors=continue` is
//! accepted, and every preview says so; results obtained that way are
//! process-death and fault-injection evidence, not power-loss evidence.
//!
//! Everything else is refused, including: an ext4 mount whose journal
//! options cannot be read; `data=writeback`; `nobarrier`; ext2 and ext3 (the
//! same superblock magic, so the type is read from the mount table, never
//! from `statfs`); tmpfs (nothing survives power loss); FUSE (ratarmount,
//! mergerfs, rclone: durability is whatever the daemon does); NTFS, exFAT,
//! vfat; network filesystems; overlayfs. The type is never assumed from a
//! path. Every path the engine touches must be on its library root's own
//! filesystem, never on a mount nested inside it.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Pseudo-option recorded when the kernel shows no jbd2 journal for the
/// device (an ext4 made or mounted without one).
const NO_JOURNAL: &str = "emuwiz:no-jbd2-journal";

/// Which filesystem requirements apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FsPolicy {
    /// Real libraries: every requirement, including `errors=remount-ro`.
    Production,
    /// Test builds only, on scratch roots a `ScratchAuthority` created: as
    /// Production, except that `errors=continue` is accepted (this host's
    /// test filesystems use it). Not power-loss evidence.
    TestScratch,
    /// Test builds only, on scratch roots a strict `ScratchAuthority`
    /// created on disposable ext4 fixtures: exactly Production's
    /// requirements, `errors=remount-ro` included. It exists so the full
    /// production eligibility policy runs end to end under test while real
    /// libraries (Production plans) stay unexecutable.
    TestStrict,
}

/// What the mount table and the ext4 driver report for one path.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MountFacts {
    pub mount_point: PathBuf,
    pub device: String,
    pub fstype: String,
    pub mount_options: Vec<String>,
    /// `/proc/fs/ext4/<dev>/options` for ext4; None when unreadable or not ext4.
    pub journal_options: Option<Vec<String>>,
}

impl MountFacts {
    pub fn summary(&self) -> String {
        let journal = self
            .journal_options
            .as_ref()
            .map(|o| {
                o.iter()
                    .filter(|x| {
                        x.starts_with("data=") || x.contains("barrier") || x.starts_with("errors=")
                    })
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_else(|| "journal options unknown".into());
        format!(
            "{} on {} ({}, {}; {journal})",
            self.mount_point.display(),
            self.device,
            self.fstype,
            self.mount_options.join(",")
        )
    }
}

/// Decodes the octal escapes `/proc/self/mountinfo` uses (`\040` etc.).
fn unescape(field: &str) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    let b = field.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            let v = b[i + 1..i + 4]
                .iter()
                .fold(0u32, |v, c| v * 8 + u32::from(c - b'0'));
            out.push(u8::try_from(v).unwrap_or(b'?'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    PathBuf::from(std::ffi::OsString::from_vec(out))
}

/// The mount holding `path` (its own filesystem: matched by device number,
/// longest mount point that is a prefix of the real path).
pub fn inspect(path: &Path) -> Result<MountFacts, String> {
    let facts = inspect_mount_table(path)?;
    #[cfg(test)]
    if let Some(f) = test_override::apply(path, &facts) {
        return f;
    }
    Ok(facts)
}

fn inspect_mount_table(path: &Path) -> Result<MountFacts, String> {
    let real = std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let dev = std::fs::metadata(&real)
        .map_err(|e| format!("{}: {e}", real.display()))?
        .dev();
    let (major, minor) = (libc::major(dev), libc::minor(dev));
    let table =
        std::fs::read_to_string("/proc/self/mountinfo").map_err(|e| format!("mount table: {e}"))?;
    parse_mount_table(&table, &real, major, minor, |major, minor| {
        let name = std::fs::read_link(format!("/sys/dev/block/{major}:{minor}")).ok()?;
        let name = name.file_name()?.to_str()?.to_string();
        let text = std::fs::read_to_string(format!("/proc/fs/ext4/{name}/options")).ok()?;
        let mut options: Vec<String> = text
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        // A mounted ext4 journal has a jbd2 entry (`<dev>-8`); its absence
        // means the filesystem runs without a journal.
        let journalled = std::fs::read_dir("/proc/fs/jbd2").ok()?.flatten().any(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_prefix(name.as_str()))
                .is_some_and(|rest| rest.starts_with('-'))
        });
        if !journalled {
            options.push(NO_JOURNAL.into());
        }
        Some(options)
    })
}

/// The mount for `real` (whose device is `major:minor`): among the entries
/// for that device, the longest mount point that is a prefix of `real`.
/// Matching on the device makes stacked mounts safe: a filesystem mounted
/// over the path has a different device and is the one `real` resolves to,
/// and a mount nested deeper is never chosen for a path outside it.
/// Duplicate entries for one mount (propagation into another peer group, a
/// stacked mount) describe the same filesystem.
fn parse_mount_table(
    table: &str,
    real: &Path,
    major: u32,
    minor: u32,
    ext4_options: impl Fn(u32, u32) -> Option<Vec<String>>,
) -> Result<MountFacts, String> {
    let want = format!("{major}:{minor}");
    let mut best: Option<MountFacts> = None;
    for line in table.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        let Some(sep) = fields.iter().position(|f| *f == "-") else {
            continue;
        };
        if fields.len() < sep + 4 || fields.len() < 6 || fields[2] != want {
            continue;
        }
        let mount_point = unescape(fields[4]);
        if !real.starts_with(&mount_point) {
            continue;
        }
        if best
            .as_ref()
            .is_some_and(|b| b.mount_point.as_os_str().len() > mount_point.as_os_str().len())
        {
            continue;
        }
        let fstype = fields[sep + 1].to_string();
        let mut mount_options: Vec<String> = fields[5].split(',').map(str::to_string).collect();
        mount_options.extend(fields[sep + 3].split(',').map(str::to_string));
        mount_options.sort();
        mount_options.dedup();
        if let Some(b) = best.as_ref().filter(|b| b.mount_point == mount_point) {
            // Several entries for one device at one mount point: propagated
            // duplicates are fine, but stacked bind mounts that disagree
            // (one read-only, say) leave the effective options uncertain.
            if b.mount_options != mount_options || b.fstype != fstype {
                return Err(format!(
                    "{} has several mounts of the same filesystem with different options; which one applies is \
                     uncertain, so it is treated as unsupported",
                    mount_point.display()
                ));
            }
            continue;
        }
        let journal_options = if fstype == "ext4" {
            ext4_options(major, minor)
        } else {
            None
        };
        best = Some(MountFacts {
            mount_point,
            device: fields[sep + 2].to_string(),
            fstype,
            mount_options,
            journal_options,
        });
    }
    best.ok_or_else(|| {
        format!(
            "{} is on no mount in the mount table (device {want})",
            real.display()
        )
    })
}

/// Ok(notes) when the engine's guarantees hold on this mount; Err(reason)
/// otherwise. Unknown means unsupported.
pub fn supported(facts: &MountFacts, policy: FsPolicy) -> Result<Vec<String>, String> {
    let at = facts.mount_point.display();
    if facts.fstype != "ext4" {
        return Err(format!(
            "{at} is {}; only ext4 is supported in this release (its crash guarantees are not established for {})",
            facts.fstype, facts.fstype
        ));
    }
    if facts.mount_options.iter().any(|o| o == "ro") {
        return Err(format!("{at} is mounted read-only"));
    }
    let Some(journal) = &facts.journal_options else {
        return Err(format!(
            "{at}: the ext4 journal options cannot be read, so durability is unknown"
        ));
    };
    let has =
        |o: &str| journal.iter().any(|x| x == o) || facts.mount_options.iter().any(|x| x == o);
    if journal.iter().any(|x| x == NO_JOURNAL) {
        return Err(format!("{at}: the ext4 filesystem has no journal"));
    }
    if has("norecovery") || has("noload") {
        return Err(format!(
            "{at}: the ext4 journal is not in use (norecovery/noload)"
        ));
    }
    if has("nobarrier") || has("barrier=0") || !has("barrier") {
        return Err(format!(
            "{at}: write barriers are not confirmed on, so fsync may not reach the device"
        ));
    }
    match journal.iter().find_map(|x| x.strip_prefix("data=")) {
        Some("ordered" | "journal") => {}
        Some(other) => {
            return Err(format!(
                "{at}: data={other} is not supported (needs data=ordered or data=journal)"
            ));
        }
        None => return Err(format!("{at}: the ext4 data mode is unknown")),
    }
    let mut notes = Vec::new();
    let errors = journal
        .iter()
        .chain(&facts.mount_options)
        .find_map(|x| x.strip_prefix("errors="));
    match (errors, policy) {
        (Some("remount-ro"), _) => {}
        (Some("continue"), FsPolicy::TestScratch) => notes.push(format!(
            "{at} uses errors=continue: accepted for test scratch folders only (process-death evidence, not \
             power-loss evidence); moving real games requires errors=remount-ro"
        )),
        (Some(other), _) => {
            return Err(format!(
                "{at} uses errors={other}: after detecting damage ext4 keeps writing instead of stopping. Moving \
                 games requires errors=remount-ro on both drives (see 'Preparing a drive' in \
                 docs/design/ouroboros-transactions.md); planning and preview still work"
            ));
        }
        (None, _) => return Err(format!("{at}: the ext4 error behaviour is unknown")),
    }
    Ok(notes)
}

/// Checks a folder the engine depends on that is not a library root (the
/// catalogue's folder, the transaction journal): its filesystem must meet
/// the same policy as the roots.
pub fn check_path(path: &Path, policy: FsPolicy) -> Result<(MountFacts, Vec<String>), String> {
    let facts = inspect(path)?;
    let notes = supported(&facts, policy)?;
    Ok((facts, notes))
}

/// Checks a library root and the paths in it the engine will touch: the
/// root's filesystem is supported, and each path is on that same
/// filesystem (not on a mount nested inside the root).
pub fn check_root(
    root: &Path,
    paths: &[&Path],
    policy: FsPolicy,
) -> Result<(MountFacts, Vec<String>), String> {
    let facts = inspect(root)?;
    let notes = supported(&facts, policy)?;
    let root_dev = std::fs::metadata(root)
        .map_err(|e| format!("{}: {e}", root.display()))?
        .dev();
    for p in paths {
        let dev = std::fs::metadata(p)
            .map_err(|e| format!("{}: {e}", p.display()))?
            .dev();
        // A bind mount of the same filesystem nested inside the root keeps
        // the device number, so the mount table is consulted too: the path
        // must be on the root's own mount, not one stacked inside it.
        let nested = dev != root_dev || inspect(p)?.mount_point != facts.mount_point;
        if nested {
            return Err(format!(
                "{} is on a different mount from its library root {} (a nested or bind mount); it is not moved",
                p.display(),
                root.display()
            ));
        }
    }
    Ok((facts, notes))
}

/// Test seam: tests replace what the mount table reports (a remount
/// read-only, changed options, a vanished mount) without touching any real
/// mount. Thread-local, so parallel tests do not see each other's.
#[cfg(test)]
pub(crate) mod test_override {
    use super::MountFacts;
    use std::cell::RefCell;
    use std::path::Path;

    type Hook = Box<dyn Fn(&Path, &MountFacts) -> Option<Result<MountFacts, String>>>;
    thread_local!(static HOOK: RefCell<Option<Hook>> = RefCell::new(None));

    pub(crate) fn set(
        hook: impl Fn(&Path, &MountFacts) -> Option<Result<MountFacts, String>> + 'static,
    ) {
        HOOK.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
    }

    pub(crate) fn clear() {
        HOOK.with(|h| *h.borrow_mut() = None);
    }

    pub(super) fn apply(path: &Path, facts: &MountFacts) -> Option<Result<MountFacts, String>> {
        HOOK.with(|h| h.borrow().as_ref().and_then(|f| f(path, facts)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(fstype: &str, mount: &[&str], journal: Option<&[&str]>) -> MountFacts {
        MountFacts {
            mount_point: "/x".into(),
            device: "/dev/x".into(),
            fstype: fstype.into(),
            mount_options: mount.iter().map(|s| s.to_string()).collect(),
            journal_options: journal.map(|j| j.iter().map(|s| s.to_string()).collect()),
        }
    }

    const GOOD: &[&str] = &[
        "journal_checksum",
        "barrier",
        "errors=remount-ro",
        "data=ordered",
    ];
    use FsPolicy::{Production, TestScratch, TestStrict};

    #[test]
    fn only_journalled_ext4_with_barriers_and_remount_ro_is_supported() {
        assert!(supported(&facts("ext4", &["rw"], Some(GOOD)), Production).is_ok());
        assert!(
            supported(
                &facts(
                    "ext4",
                    &["rw"],
                    Some(&["barrier", "errors=remount-ro", "data=journal"])
                ),
                Production
            )
            .is_ok()
        );
        for (fstype, mount, journal, why) in [
            ("tmpfs", &["rw"][..], None, "tmpfs"),
            ("fuse", &["rw"][..], None, "fuse"),
            ("fuse.mergerfs", &["rw"][..], None, "mergerfs"),
            ("exfat", &["rw"][..], None, "exfat"),
            ("ntfs3", &["rw"][..], None, "ntfs3"),
            ("vfat", &["rw"][..], None, "vfat"),
            ("ext3", &["rw"][..], None, "ext3"),
            ("nfs4", &["rw"][..], None, "nfs4"),
            ("overlay", &["rw"][..], None, "overlay"),
            ("ext4", &["rw"][..], None, "cannot be read"),
            ("ext4", &["ro"][..], Some(GOOD), "read-only"),
            (
                "ext4",
                &["rw"][..],
                Some(&["barrier", "data=writeback"][..]),
                "writeback",
            ),
            (
                "ext4",
                &["rw"][..],
                Some(&["nobarrier", "data=ordered"][..]),
                "barriers",
            ),
            ("ext4", &["rw"][..], Some(&["data=ordered"][..]), "barriers"),
            (
                "ext4",
                &["rw", "norecovery"][..],
                Some(GOOD),
                "journal is not in use",
            ),
            (
                "ext4",
                &["rw"][..],
                Some(&["barrier", "errors=remount-ro"][..]),
                "data mode",
            ),
            (
                "ext4",
                &["rw"][..],
                Some(&["barrier", "data=ordered"][..]),
                "error behaviour is unknown",
            ),
            (
                "ext4",
                &["rw"][..],
                Some(&["barrier", "errors=continue", "data=ordered"][..]),
                "errors=remount-ro",
            ),
            (
                "ext4",
                &["rw"][..],
                Some(&["barrier", "errors=panic", "data=ordered"][..]),
                "errors=panic",
            ),
        ] {
            for policy in [Production, TestScratch, TestStrict] {
                if why == "errors=remount-ro" && policy == TestScratch {
                    continue;
                }
                let err = supported(&facts(fstype, mount, journal), policy).expect_err(why);
                assert!(
                    err.contains(why),
                    "{fstype} {mount:?} {journal:?} {policy:?}: {err}"
                );
            }
        }
    }

    #[test]
    fn errors_continue_is_accepted_only_for_test_scratch_and_said_so() {
        let f = facts(
            "ext4",
            &["rw"],
            Some(&["barrier", "errors=continue", "data=ordered"]),
        );
        let err = supported(&f, Production).unwrap_err();
        assert!(err.contains("planning and preview still work"), "{err}");
        assert_eq!(
            supported(&f, TestStrict).unwrap_err(),
            err,
            "the strict test policy is Production's"
        );
        let notes = supported(&f, TestScratch).unwrap();
        assert!(
            notes
                .iter()
                .any(|n| n.contains("test scratch folders only")),
            "{notes:?}"
        );
    }

    const TABLE: &str = "\
30 1 252:0 / / rw,relatime shared:1 - ext4 /dev/mapper/root rw
160 44 8:49 / /mnt/usbdrive rw,relatime shared:111 - ext4 /dev/sdd1 rw
161 30 8:49 / /mnt/usbdrive rw,relatime shared:111 - ext4 /dev/sdd1 rw
187 160 0:59 / /mnt/usbdrive/games/psx/ratarmount rw,nosuid,nodev,relatime shared:1184 - fuse FuseMount rw,user_id=1000
200 160 8:65 / /mnt/usbdrive/games/over rw,relatime shared:2 - ext4 /dev/sde1 rw
300 30 0:99 / /mnt/my\\040games rw - tmpfs tmpfs rw
";

    fn ext4(_: u32, _: u32) -> Option<Vec<String>> {
        Some(GOOD.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn stacked_duplicate_and_nested_mounts_resolve_by_device() {
        let at = |path: &str, maj, min| parse_mount_table(TABLE, Path::new(path), maj, min, ext4);
        // The duplicated (propagated) usbdrive entries resolve to one filesystem.
        let usb = at("/mnt/usbdrive/games/nes/x.nes", 8, 49).unwrap();
        assert_eq!(
            (usb.mount_point.as_path(), usb.fstype.as_str()),
            (Path::new("/mnt/usbdrive"), "ext4")
        );
        // A path inside the nested FUSE mount is the FUSE filesystem: refused.
        let fuse = at("/mnt/usbdrive/games/psx/ratarmount/a.bin", 0, 59).unwrap();
        assert_eq!(fuse.fstype, "fuse");
        assert!(supported(&fuse, TestScratch).is_err());
        // A different filesystem stacked over a folder is identified by its
        // own device, not by the path's longest prefix.
        let over = at("/mnt/usbdrive/games/over/y.nes", 8, 65).unwrap();
        assert_eq!(
            (over.mount_point.as_path(), over.device.as_str()),
            (Path::new("/mnt/usbdrive/games/over"), "/dev/sde1")
        );
        // A device with no entry covering the path is an error, never a guess.
        assert!(at("/mnt/usbdrive/games/nes/x.nes", 8, 65).is_err());
        assert!(at("/elsewhere", 9, 9).is_err());
        // Escaped mount points.
        let spaced = at("/mnt/my games/z", 0, 99).unwrap();
        assert_eq!(spaced.fstype, "tmpfs");
    }

    #[test]
    fn an_ext4_without_a_journal_is_refused_in_every_policy() {
        let mut opts: Vec<&str> = GOOD.to_vec();
        opts.push(NO_JOURNAL);
        for policy in [Production, TestScratch, TestStrict] {
            let err = supported(&facts("ext4", &["rw"], Some(&opts)), policy).unwrap_err();
            assert!(err.contains("no journal"), "{policy:?}: {err}");
        }
    }

    #[test]
    fn same_device_bind_mounts_are_seen_and_disagreeing_stacks_refused() {
        // A bind mount of the same filesystem over a game folder keeps the
        // device number; the mount table still shows it as its own mount.
        let table = format!(
            "{TABLE}400 160 8:49 /elsewhere /mnt/usbdrive/games/nes rw,relatime shared:111 - ext4 /dev/sdd1 rw\n"
        );
        let f = parse_mount_table(
            &table,
            Path::new("/mnt/usbdrive/games/nes/x.nes"),
            8,
            49,
            ext4,
        )
        .unwrap();
        assert_eq!(f.mount_point, Path::new("/mnt/usbdrive/games/nes"));
        let root = parse_mount_table(&table, Path::new("/mnt/usbdrive"), 8, 49, ext4).unwrap();
        assert_ne!(
            f.mount_point, root.mount_point,
            "check_root compares these and refuses"
        );
        // The same filesystem stacked twice at one place with different
        // per-mount options (a read-only bind on top): uncertain, refused.
        let stacked = format!(
            "{TABLE}401 160 8:49 / /mnt/usbdrive ro,relatime shared:111 - ext4 /dev/sdd1 rw\n"
        );
        let err = parse_mount_table(
            &stacked,
            Path::new("/mnt/usbdrive/games/nes/x.nes"),
            8,
            49,
            ext4,
        )
        .unwrap_err();
        assert!(err.contains("different options"), "{err}");
    }

    #[test]
    fn effective_options_come_from_the_ext4_driver_not_fstab() {
        // The ext4 options file is the effective state; when it is
        // unreadable the filesystem is refused, whatever the mount table says.
        let none =
            parse_mount_table(TABLE, Path::new("/mnt/usbdrive/a"), 8, 49, |_, _| None).unwrap();
        assert!(
            supported(&none, TestScratch)
                .unwrap_err()
                .contains("cannot be read")
        );
    }

    #[test]
    fn mount_table_escapes_are_decoded() {
        assert_eq!(
            unescape("/mnt/my\\040games"),
            PathBuf::from("/mnt/my games")
        );
        assert_eq!(unescape("/plain"), PathBuf::from("/plain"));
    }

    #[test]
    fn this_host_is_inspected_from_the_mount_table() {
        let f = inspect(Path::new("/")).unwrap();
        assert_eq!(f.mount_point, PathBuf::from("/"));
        if let Ok(shm) = inspect(Path::new("/dev/shm")) {
            assert_eq!(shm.fstype, "tmpfs");
            assert!(supported(&shm, TestScratch).is_err());
        }
    }
}
