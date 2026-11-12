//! Nested FUSE mounts (for example ratarmount archive views) inside one
//! configured source. Each one is its own filesystem ownership boundary:
//! it is opened as a separately pinned root, and everything beneath it is
//! resolved from that root with the owned rules (beneath, no symlinks, no
//! mount crossing). The parent source never resolves across it.
//!
//! The kernel's mount identity (device, mount ID) changes every time such a
//! mount restarts, so it only proves "same mount instance" during one scan.
//! Across restarts the area is recognised by a stable identity: the kernel
//! mount-table type and source, the root inode, and a digest of every
//! entry's path, type, size and modification time read inside the mount.
use super::{BoundRoot, NestedState, SourceRootBinding};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::{Path, PathBuf};

/// `FUSE_SUPER_MAGIC`, as reported by `fstatfs`.
pub(crate) const FUSE_SUPER_MAGIC: u64 = 0x6573_5546;
/// Bound on identity work; a larger mount is left unproven, never guessed.
const MAX_IDENTITY_ENTRIES: u64 = 2_000_000;

/// Survives a remount of the same backing archive; changes when its
/// content, layout or mount configuration changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StableAreaIdentity {
    pub filesystem_type: i64,
    pub mount_fstype: String,
    pub mount_source: String,
    pub root_inode: u64,
    pub entries: u64,
    pub content_digest: String,
}

/// What `source_nested_boundaries.binding_json` holds. Older records are a
/// bare [`SourceRootBinding`] and decode with no stable identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NestedBoundaryRecord {
    #[serde(flatten)]
    pub(crate) binding: SourceRootBinding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) stable: Option<StableAreaIdentity>,
    /// Kernel mount ID of the instance the latest scan proved. Device numbers
    /// of FUSE mounts are reused after an unmount, so only this tells one
    /// instance from the next; it is never part of the stable identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mount_id: Option<u64>,
}

impl NestedBoundaryRecord {
    pub(crate) fn decode(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }
}

pub(crate) fn is_supported_area_filesystem(filesystem_type: i64) -> bool {
    filesystem_type as u64 & 0xffff_ffff == FUSE_SUPER_MAGIC
}

/// One nested mount opened as its own pinned root.
pub(crate) struct LiveArea {
    pub(crate) root: BoundRoot,
    pub(crate) mount_id: u64,
}

impl LiveArea {
    pub(crate) fn binding(&self) -> &SourceRootBinding {
        &self.root.binding
    }
    pub(crate) fn path(&self) -> &Path {
        self.root.root_path()
    }
    /// Still the same mount instance, still reached from the source the same
    /// way (no symlink, still a mount root, same kernel mount ID).
    pub(crate) fn current(&self, source: &BoundRoot) -> bool {
        self.root.current()
            && source.nested_state(self.path()).is_ok_and(|state| {
                state.is_mount_root()
                    && state.mount_id == self.mount_id
                    && state.binding == self.root.binding
            })
    }
}

/// Opens `area` (a directory beneath `source`) as a pinned root, only when it
/// is currently a mount root of a supported filesystem type.
pub(crate) fn open_area(source: &BoundRoot, area: &Path) -> Result<LiveArea, &'static str> {
    let before: NestedState = source
        .nested_state(area)
        .map_err(|_| "mount point is missing or cannot be inspected")?;
    if !before.is_mount_root() {
        return Err("no filesystem is mounted there now");
    }
    if !is_supported_area_filesystem(before.binding.filesystem_type) {
        return Err("only FUSE archive mounts are scanned inside a source");
    }
    let root = BoundRoot::open(area).ok_or("the mounted folder could not be opened safely")?;
    let after = source
        .nested_state(area)
        .map_err(|_| "the mount changed while it was opened")?;
    if root.binding != before.binding || after != before {
        return Err("the mount changed while it was opened");
    }
    Ok(LiveArea {
        root,
        mount_id: before.mount_id,
    })
}

/// Computes the stable identity of an opened area. Every entry is resolved
/// through the area root; a nested mount or symlink inside is recorded by
/// kind only and never followed.
pub(crate) fn stable_identity(
    source: &BoundRoot,
    area: &LiveArea,
) -> Result<StableAreaIdentity, &'static str> {
    let (mount_fstype, mount_source) = mount_table_entry(area.binding().device, area.path())?;
    let mut hasher = Sha256::new();
    let mut entries = 0_u64;
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        let names = area
            .root
            .list_dir(&area.path().join(&relative))
            .map_err(|_| "a folder inside the mount could not be listed")?;
        for name in names {
            entries += 1;
            if entries > MAX_IDENTITY_ENTRIES {
                return Err("the mount holds too many entries to identify");
            }
            let child = relative.join(&name);
            let bytes = child.as_os_str().as_bytes();
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
            match area.root.metadata(&area.path().join(&child)) {
                Ok(metadata) => {
                    let directory = metadata.is_dir();
                    hasher.update([if directory {
                        b'd'
                    } else if metadata.is_file() {
                        b'f'
                    } else {
                        b'o'
                    }]);
                    hasher.update(if directory { 0 } else { metadata.size() }.to_le_bytes());
                    hasher.update(metadata.mtime().to_le_bytes());
                    hasher.update(metadata.mtime_nsec().to_le_bytes());
                    if directory {
                        pending.push(child);
                    }
                }
                Err(error) => match error.raw_os_error() {
                    Some(libc::ELOOP) => hasher.update(b"l"),
                    Some(libc::EXDEV) => hasher.update(b"m"),
                    _ => return Err("an entry inside the mount could not be read"),
                },
            }
        }
    }
    if !area.current(source) {
        return Err("the mount changed while it was identified");
    }
    let digest = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(StableAreaIdentity {
        filesystem_type: area.binding().filesystem_type,
        mount_fstype,
        mount_source,
        root_inode: area.binding().inode,
        entries,
        content_digest: digest,
    })
}

/// Filesystem type and mount source the kernel reports for the mount of
/// `device` at `mount_point`. Stacked copies of one mount (a parent mounted
/// twice) appear more than once; they must all agree.
fn mount_table_entry(device: u64, mount_point: &Path) -> Result<(String, String), &'static str> {
    let table = std::fs::read_to_string("/proc/self/mountinfo")
        .map_err(|_| "the kernel mount table could not be read")?;
    mount_table_entry_in(&table, device, mount_point)
}

fn mount_table_entry_in(
    table: &str,
    device: u64,
    mount_point: &Path,
) -> Result<(String, String), &'static str> {
    let wanted_device = format!("{}:{}", libc::major(device), libc::minor(device));
    let mut found: Option<(String, String)> = None;
    for line in table.lines() {
        let Some((before, after)) = line.split_once(" - ") else {
            continue;
        };
        let fields: Vec<&str> = before.split(' ').collect();
        let tail: Vec<&str> = after.split(' ').collect();
        if fields.len() < 5 || tail.len() < 2 {
            continue;
        }
        if fields[2] != wanted_device || unescape(fields[4]) != mount_point.as_os_str().as_bytes() {
            continue;
        }
        let entry = (tail[0].to_string(), tail[1].to_string());
        match &found {
            Some(existing) if *existing != entry => {
                return Err("kernel mount table entries for this folder disagree");
            }
            _ => found = Some(entry),
        }
    }
    found.ok_or("the mount is not in the kernel mount table")
}

/// Reverses the kernel's octal escaping of spaces, tabs, newlines and
/// backslashes in mountinfo paths.
fn unescape(field: &str) -> Vec<u8> {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\'
            && index + 4 <= bytes.len()
            && bytes[index + 1..index + 4]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b))
        {
            let value = bytes[index + 1..index + 4]
                .iter()
                .fold(0_u32, |acc, b| acc * 8 + u32::from(b - b'0'));
            out.push(value as u8);
            index += 4;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "\
36 25 8:49 / /mnt/usbdrive rw,relatime shared:1 - ext4 /dev/sdd1 rw
61 36 0:63 / /mnt/usbdrive/games/gba/ratarmount rw,nosuid shared:2 - fuse FuseMount rw,user_id=1000
95 70 0:63 / /mnt/usbdrive/games/gba/ratarmount rw,nosuid shared:2 - fuse FuseMount rw,user_id=1000
62 36 0:64 / /mnt/usbdrive/games/my\\040games rw - fuse.other src rw
";

    #[test]
    fn mount_table_lookup_matches_device_and_unescaped_path_and_requires_agreement() {
        let device = libc::makedev(0, 63);
        assert_eq!(
            mount_table_entry_in(
                TABLE,
                device,
                Path::new("/mnt/usbdrive/games/gba/ratarmount")
            ),
            Ok(("fuse".into(), "FuseMount".into()))
        );
        assert_eq!(
            mount_table_entry_in(
                TABLE,
                libc::makedev(0, 64),
                Path::new("/mnt/usbdrive/games/my games")
            ),
            Ok(("fuse.other".into(), "src".into()))
        );
        assert!(mount_table_entry_in(TABLE, device, Path::new("/mnt/usbdrive/games")).is_err());
        let disagreeing =
            format!("{TABLE}96 70 0:63 / /mnt/usbdrive/games/gba/ratarmount rw - fuse Other rw\n");
        assert_eq!(
            mount_table_entry_in(
                &disagreeing,
                device,
                Path::new("/mnt/usbdrive/games/gba/ratarmount")
            ),
            Err("kernel mount table entries for this folder disagree")
        );
    }

    #[test]
    fn old_bare_binding_records_still_decode() {
        let binding = SourceRootBinding {
            device: 1,
            inode: 2,
            filesystem_type: 3,
            filesystem_id: vec![4],
        };
        let old = serde_json::to_string(&binding).unwrap();
        let record = NestedBoundaryRecord::decode(&old).unwrap();
        assert_eq!(record.binding, binding);
        assert_eq!(record.stable, None);
        let new = serde_json::to_string(&NestedBoundaryRecord {
            binding: binding.clone(),
            stable: None,
            mount_id: None,
        })
        .unwrap();
        assert_eq!(
            new, old,
            "a record without identity is byte-identical to the old form"
        );
    }
}
