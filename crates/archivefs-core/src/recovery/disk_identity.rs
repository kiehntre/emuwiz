//! Which disk a folder for prepared copies is meant to be on.
//!
//! A folder being there proves nothing about which disk is: when a disk is
//! not mounted, its mount point is an ordinary folder on the disk underneath,
//! and a folder of the same name can exist there. Looking at the folder and
//! remembering what filesystem it is on has the same flaw the first time: it
//! records whatever happens to be at that path.
//!
//! So the identity is never learned from the path. The person says which
//! disk they mean by its filesystem UUID (what `lsblk -f` prints, a property
//! of the disk itself, the same wherever it is mounted), and the folder is
//! accepted only when the filesystem it is on reports the identity that
//! belongs to that UUID.
//!
//! Only ext4 (and ext2/ext3, which share its on-disk format) is supported:
//! there the id the kernel reports for a mounted filesystem (`f_fsid`) is
//! computed from the UUID alone, so the two can be compared without reading
//! the device and without any privilege. Every other filesystem is refused.
//!
//! What this does not prove (see `docs/GAME_RECOVERY_ENGINE.md`): the kernel
//! id is 64 bits folded from the 128-bit UUID; a block-level copy of a disk
//! has the same UUID and cannot be told from the original when only one of
//! them is connected; and a person who gives the UUID of the wrong disk has
//! named the wrong disk.

use std::fs::File;
use std::path::{Path, PathBuf};

use super::prepared_media::{filesystem_id, filesystem_of, unescape_mount_path};

/// How the person finds the UUID, appended to refusals that ask for one.
pub const HOW_TO_FIND_UUID: &str =
    "`lsblk -f` lists each connected disk's filesystem with its UUID";

/// The ext4 filesystem a folder is expected to be on, as the person named it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpectedFilesystem {
    uuid: String,
    id: u64,
}

impl ExpectedFilesystem {
    /// Parses a filesystem UUID: 32 hexadecimal digits, with or without the
    /// usual four dashes. Anything else is refused, never guessed at.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let refused = || {
            format!(
                "{text:?} is not a filesystem UUID (expected the form 01234567-89ab-cdef-0123-456789abcdef; {HOW_TO_FIND_UUID})"
            )
        };
        let bytes = text.as_bytes();
        let dashed = bytes.len() == 36
            && bytes
                .iter()
                .enumerate()
                .all(|(index, byte)| (*byte == b'-') == matches!(index, 8 | 13 | 18 | 23));
        if !(dashed || bytes.len() == 32) {
            return Err(refused());
        }
        let digits: Vec<u8> = bytes.iter().copied().filter(|byte| *byte != b'-').collect();
        if digits.len() != 32 || !digits.iter().all(u8::is_ascii_hexdigit) {
            return Err(refused());
        }
        let mut raw = [0_u8; 16];
        for (index, pair) in digits.chunks(2).enumerate() {
            let pair = std::str::from_utf8(pair).map_err(|_| refused())?;
            raw[index] = u8::from_str_radix(pair, 16).map_err(|_| refused())?;
        }
        // ext4's `statfs`: the two little-endian halves of the UUID, XORed.
        let (first, second) = raw.split_at(8);
        let id = u64::from_le_bytes(first.try_into().map_err(|_| refused())?)
            ^ u64::from_le_bytes(second.try_into().map_err(|_| refused())?);
        if id == 0 {
            // The all-zero UUID, or one whose halves cancel: the kernel
            // would report "no identity" for it.
            return Err(format!(
                "{text:?} does not identify a filesystem (its identity is empty)"
            ));
        }
        let hex: String = raw.iter().map(|byte| format!("{byte:02x}")).collect();
        Ok(Self {
            uuid: format!(
                "{}-{}-{}-{}-{}",
                &hex[0..8],
                &hex[8..12],
                &hex[12..16],
                &hex[16..20],
                &hex[20..32]
            ),
            id,
        })
    }

    /// The UUID in its usual lower-case, dashed form.
    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    /// The id the kernel reports for a mounted ext4 filesystem with this UUID.
    pub fn id(&self) -> u64 {
        self.id
    }
}

#[cfg(test)]
pub(crate) type KindOverride = Box<dyn Fn(&Path) -> Option<bool>>;
#[cfg(test)]
pub(crate) type SameIdentityOverride = Box<dyn Fn(u64) -> Option<Result<usize, String>>>;

#[cfg(test)]
thread_local! {
    /// Test-only: whether the folder at a path is on an ext4 filesystem (a
    /// test cannot format or mount a disk).
    pub(crate) static EXT4_OVERRIDE: std::cell::RefCell<Option<KindOverride>> =
        const { std::cell::RefCell::new(None) };
    /// Test-only: how many connected disks report an identity.
    pub(crate) static SAME_IDENTITY_OVERRIDE: std::cell::RefCell<Option<SameIdentityOverride>> =
        const { std::cell::RefCell::new(None) };
}

const EXT_FAMILY: [&str; 3] = ["ext4", "ext3", "ext2"];

/// Whether an open folder is on an ext2/ext3/ext4 filesystem, asked of the
/// handle. `path` is how it was reached and is not opened again.
fn on_ext4(directory: &File, path: &Path) -> std::io::Result<bool> {
    use std::os::fd::AsRawFd;
    #[cfg(test)]
    if let Some(pretended) =
        EXT4_OVERRIDE.with(|hook| hook.borrow().as_ref().and_then(|hook| hook(path)))
    {
        return Ok(pretended);
    }
    let _ = path;
    // SAFETY: zeroed is a valid initial value for this plain C struct.
    let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: the descriptor is open and `stats` is writable and sized.
    if unsafe { libc::fstatfs(directory.as_raw_fd(), &mut stats) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // ext2, ext3 and ext4 share one magic number.
    Ok(stats.f_type == libc::EXT4_SUPER_MAGIC)
}

/// The mounts of a `/proc/self/mountinfo` table that are not covered by
/// another mount stacked directly on top of them: device ("major:minor"),
/// mount point and filesystem type. A covered mount cannot be reached by its
/// path (the path answers for what is on top), and asking would mean asking
/// the filesystem on top, which may be a network or FUSE mount that hangs.
pub(crate) fn mount_lines(table: &str) -> Vec<(String, PathBuf, String)> {
    let parsed: Vec<(String, String, String, PathBuf, String)> = table
        .lines()
        .filter_map(|line| {
            let (before, after) = line.split_once(" - ")?;
            let mut fields = before.split(' ');
            let id = fields.next()?.to_string();
            let parent = fields.next()?.to_string();
            let device = fields.next()?.to_string();
            let mount_point = unescape_mount_path(fields.nth(1)?);
            let kind = after.split(' ').next()?.to_string();
            Some((id, parent, device, mount_point, kind))
        })
        .collect();
    parsed
        .iter()
        .filter(|(id, _, _, mount_point, _)| {
            !parsed
                .iter()
                .any(|(_, parent, _, on_top, _)| parent == id && on_top == mount_point)
        })
        .map(|(_, _, device, mount_point, kind)| {
            (device.clone(), mount_point.clone(), kind.clone())
        })
        .collect()
}

/// The distinct devices among the ext4 mounts of `table` whose filesystem
/// reports `id` (asked through `id_at`).
pub(crate) fn devices_reporting(
    table: &str,
    id: u64,
    id_at: &dyn Fn(&str, &Path) -> Option<u64>,
) -> std::collections::BTreeSet<String> {
    mount_lines(table)
        .into_iter()
        .filter(|(_, _, kind)| EXT_FAMILY.contains(&kind.as_str()))
        .filter(|(device, mount_point, _)| id_at(device, mount_point) == Some(id))
        .map(|(device, _, _)| device)
        .collect()
}

/// A device number the way `/proc/self/mountinfo` writes it.
pub(crate) fn device_name(device: u64) -> String {
    format!("{}:{}", libc::major(device), libc::minor(device))
}

/// The identity of the filesystem of `device` mounted at `mount_point`, only
/// if that mount is what the path leads to now. A mount hidden under another
/// one cannot be asked by its path (the path answers for the mount on top),
/// so it reports nothing rather than the identity of what covers it.
pub(crate) fn visible_identity(device: &str, mount_point: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    let reached = std::fs::metadata(mount_point).ok()?.dev();
    (device_name(reached) == device)
        .then(|| filesystem_id(mount_point))
        .flatten()
}

/// How many connected (mounted) ext4 disks report this identity. More than
/// one means the identity does not say which disk is meant.
///
/// `own` is the device of the folder being verified, which reports the
/// identity itself: it counts even if no visible mount shows it, so a second
/// device with the same identity is noticed either way. A copy mounted where
/// it is hidden under another mount is not seen (see `visible_identity`).
fn disks_reporting(id: u64, own: u64) -> Result<usize, String> {
    #[cfg(test)]
    if let Some(pretended) =
        SAME_IDENTITY_OVERRIDE.with(|hook| hook.borrow().as_ref().and_then(|hook| hook(id)))
    {
        return pretended;
    }
    let table = std::fs::read_to_string("/proc/self/mountinfo")
        .map_err(|error| format!("the list of mounted disks could not be read ({error})"))?;
    let mut devices = devices_reporting(&table, id, &visible_identity);
    let listed = mount_lines(&table).iter().any(|(device, _, kind)| {
        *device == device_name(own) && EXT_FAMILY.contains(&kind.as_str())
    });
    if !listed {
        // The folder's own disk is not among the visible ext4 mounts: its
        // identity cannot be confirmed against the list at all.
        return Ok(0);
    }
    devices.insert(device_name(own));
    Ok(devices.len())
}

/// Why an open folder is not verifiably on the filesystem with an identity.
#[derive(Debug)]
pub(crate) enum NotVerified {
    NotExt4,
    Unknown(std::io::Error),
    OtherDisk,
    NotMounted,
    Shared,
    Table(String),
}

/// Proves that an open folder is on the one connected ext4 filesystem that
/// reports `id`, or says why not. Nothing is written and nothing is created.
pub(crate) fn verify_id_on(directory: &File, path: &Path, id: u64) -> Result<(), NotVerified> {
    use std::os::unix::fs::MetadataExt;
    if !on_ext4(directory, path).map_err(NotVerified::Unknown)? {
        return Err(NotVerified::NotExt4);
    }
    let actual = filesystem_of(directory, path)
        .map_err(NotVerified::Unknown)?
        .id;
    if actual != Some(id) {
        return Err(NotVerified::OtherDisk);
    }
    let own = directory.metadata().map_err(NotVerified::Unknown)?.dev();
    match disks_reporting(id, own).map_err(NotVerified::Table)? {
        1 => Ok(()),
        0 => Err(NotVerified::NotMounted),
        _ => Err(NotVerified::Shared),
    }
}

/// Proves that an open folder is on the filesystem the person named, or says
/// why not. Nothing is written and nothing is created. Every refusal means
/// "do not write here": the folder is on another filesystem (its disk is not
/// mounted, or another disk is in its place), on a filesystem whose identity
/// cannot be verified, or the identity is shared by two connected disks.
pub(crate) fn verify_on(
    directory: &File,
    path: &Path,
    expected: &ExpectedFilesystem,
) -> Result<(), String> {
    verify_id_on(directory, path, expected.id()).map_err(|refusal| match refusal {
        NotVerified::NotExt4 => format!(
            "{} is not on an ext4 filesystem. EmuWiz can verify which disk a folder is on only for ext4, so only ext4 disks can hold prepared copies outside EmuWiz's own data folder (if the disk with UUID {} is ext4, it is not mounted here)",
            path.display(),
            expected.uuid()
        ),
        NotVerified::Unknown(error) => format!(
            "which disk {} is on could not be checked ({error})",
            path.display()
        ),
        NotVerified::OtherDisk => format!(
            "{} is not on the disk with filesystem UUID {} (is that disk connected and mounted?)",
            path.display(),
            expected.uuid()
        ),
        NotVerified::NotMounted => format!(
            "the disk with filesystem UUID {} is not among the mounted ext4 disks, so {} cannot be verified",
            expected.uuid(),
            path.display()
        ),
        NotVerified::Shared => format!(
            "more than one connected disk reports the identity of filesystem UUID {} (a copied disk?), so EmuWiz cannot tell which one {} is on; disconnect the other disk",
            expected.uuid(),
            path.display()
        ),
        NotVerified::Table(detail) => detail,
    })
}

/// A connected ext4 disk, for the person to choose from by what it is
/// rather than by where a folder happens to be.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MountedDisk {
    pub uuid: String,
    pub label: Option<String>,
    pub mount_point: PathBuf,
    pub available_bytes: Option<u64>,
}

const UDEV_DATA: &str = "/run/udev/data";
const MAX_UDEV_RECORD_BYTES: u64 = 256 * 1024;

/// The UUID and label the system's device database (udev) read from the
/// device itself, for the block device `major:minor`.
fn device_facts(device: &str) -> Option<(String, Option<String>)> {
    use std::io::Read;
    if !device
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b':')
    {
        return None;
    }
    let path = Path::new(UDEV_DATA).join(format!("b{device}"));
    let file = crate::safe_read::open_regular_file_nonblocking(&path, false).ok()?;
    let mut text = String::new();
    file.take(MAX_UDEV_RECORD_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    device_facts_in(&text)
}

/// The filesystem UUID and label in one udev database record.
pub(crate) fn device_facts_in(record: &str) -> Option<(String, Option<String>)> {
    let value = |key: &str| {
        record
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .map(str::to_string)
    };
    Some((
        value("E:ID_FS_UUID=")?,
        value("E:ID_FS_LABEL=").filter(|label| !label.is_empty()),
    ))
}

/// The connected ext4 disks whose identity is consistent: the UUID the
/// device database read from the device belongs to the identity the mounted
/// filesystem reports, and no other connected disk reports it. A list for a
/// person to choose from; choosing proves nothing by itself, the folder is
/// still verified against the chosen UUID (see [`verify_on`]).
pub fn mounted_ext4_disks() -> Vec<MountedDisk> {
    let Ok(table) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return Vec::new();
    };
    mounted_ext4_disks_in(&table, &device_facts, &visible_identity)
}

/// A device's filesystem UUID and, if it has one, its label.
type DeviceFacts = (String, Option<String>);

pub(crate) fn mounted_ext4_disks_in(
    table: &str,
    facts_of: &dyn Fn(&str) -> Option<DeviceFacts>,
    id_at: &dyn Fn(&str, &Path) -> Option<u64>,
) -> Vec<MountedDisk> {
    let mut disks: Vec<MountedDisk> = Vec::new();
    for (device, mount_point, kind) in mount_lines(table) {
        if !EXT_FAMILY.contains(&kind.as_str()) {
            continue;
        }
        let Some((uuid, label)) = facts_of(&device) else {
            continue;
        };
        let Ok(expected) = ExpectedFilesystem::parse(&uuid) else {
            continue;
        };
        if id_at(&device, &mount_point) != Some(expected.id())
            || devices_reporting(table, expected.id(), id_at).len() != 1
        {
            continue;
        }
        // One entry per disk: the shortest place it is mounted.
        match disks.iter_mut().find(|known| known.uuid == expected.uuid()) {
            Some(known) => {
                if mount_point.as_os_str().len() < known.mount_point.as_os_str().len() {
                    known.mount_point = mount_point;
                }
            }
            None => disks.push(MountedDisk {
                uuid: expected.uuid().to_string(),
                label,
                available_bytes: None,
                mount_point,
            }),
        }
    }
    for disk in &mut disks {
        disk.available_bytes = super::prepared_media::available_bytes(&disk.mount_point).ok();
    }
    disks.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    disks
}
