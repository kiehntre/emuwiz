//! Pure, read-only FAT12/FAT16 root-directory observation for DOS-family
//! boot evidence.
//!
//! # A valid FAT filesystem is *not* DOS
//!
//! FAT12/FAT16 is a structure shared by MS-DOS, PC DOS, DR-DOS, Windows,
//! OS/2, the Atari ST, NEC PC-98, Sharp X68000, digital cameras and
//! countless embedded devices. A coherent BIOS Parameter Block, a
//! plausible geometry, an OEM string, a volume label, and the file's
//! extension therefore prove nothing about the platform on their own -
//! [`crate::disk_format::atari_st`] already documents exactly this trap
//! for the `.st` raw-floppy case.
//!
//! So this module keys strictly on **documented DOS system-file
//! combinations found in the root directory**, and nothing else:
//!
//! | Pair                          | Family it establishes |
//! |-------------------------------|-----------------------|
//! | `IO.SYS` + `MSDOS.SYS`        | MS-DOS family         |
//! | `IBMBIO.COM` + `IBMDOS.COM`   | PC DOS / DR-DOS family |
//!
//! `COMMAND.COM` is the DOS command interpreter and ships with every one
//! of them; it is recorded as corroboration but never resolves a family
//! on its own. `CONFIG.SYS` / `AUTOEXEC.BAT` are plain text scripts any
//! tool can drop onto a FAT disk and are not consulted at all. One file
//! from a pair, without its partner, is not sufficient.
//!
//! # Format verified, not assumed
//!
//! The BPB field offsets and the root-directory arithmetic below were
//! cross-checked against two independent technical descriptions before
//! being coded:
//!
//! - the OSDev wiki "FAT" article
//!   (<https://wiki.osdev.org/FAT>), and
//! - Wikipedia, "Design of the FAT file system" (the DOS 2.0 / DOS 3.31
//!   BPB tables, themselves cited to Microsoft's `fatgen103` specification)
//!   (<https://en.wikipedia.org/wiki/Design_of_the_FAT_file_system>).
//!
//! They agree, and they agree with the FAT12 BPB parser already in
//! [`crate::disk_format::atari_st`]. The DOS system-file naming was taken
//! from Wikipedia's "IO.SYS" article and corroborating DOS-internals
//! references: MS-DOS uses `IO.SYS` + `MSDOS.SYS`; IBM PC DOS (and DR-DOS
//! 3.31-7.05) use `IBMBIO.COM` + `IBMDOS.COM`. FreeDOS ships a single
//! combined `KERNEL.SYS` and so has no documented *pair* - it is
//! deliberately not resolved here.
//!
//! # Bounded
//!
//! Every read goes through [`crate::safe_read::open_bounded_read`] under
//! the caller's trusted-root policy. The boot sector is one 512-byte
//! read; the root directory is read in <= 1 KiB chunks and only ever when
//! it begins within [`crate::disk_format::MAX_DISK_FORMAT_OFFSET`] of the
//! start of the file and is no larger than [`MAX_ROOT_DIRECTORY_BYTES`].
//! The whole inspection stays inside
//! [`crate::disk_format::MAX_DISK_FORMAT_BYTES_READ`]. No FAT chain is
//! walked, no file data is read, nothing is written.
//!
//! # Evidence scope
//!
//! A found pair yields a `Strong` [`ContentEvidenceKind::BootStructure`]
//! fact ([`DOS_MSDOS_SYSTEM_FILES`] / [`DOS_PCDOS_SYSTEM_FILES`]) which
//! [`crate::platform_evidence_fusion`] maps to the canonical `DOS`
//! platform. That establishes *DOS-family boot media* only. It never
//! establishes a game title, release, revision or publisher - a
//! DAT/hash match remains the sole authority for release identity.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::content_evidence::{ContentEvidence, ContentEvidenceConfidence, ContentEvidenceKind};
use crate::disk_format::{
    DiskFormatRefusal, MAX_DISK_FORMAT_BYTES_READ, MAX_DISK_FORMAT_OFFSET,
    MAX_DISK_FORMAT_READ_CHUNK,
};
use crate::safe_read::{TrustedRoots, open_bounded_read};

/// The boot sector this module reads, and the only fixed-size read it makes
/// before consulting the geometry.
pub const FAT_BOOT_SECTOR_BYTES: usize = 512;

/// One FAT directory entry.
pub const DIRECTORY_ENTRY_BYTES: usize = 32;

/// Offset of the `0x55 0xAA` boot signature inside the boot sector.
pub const BOOT_SIGNATURE_OFFSET: usize = 510;

/// The largest root directory this module will read: 512 entries x 32
/// bytes. 512 is the fixed FAT16 hard-disk root size and larger than any
/// FAT12 floppy's (112 or 224); a BPB declaring more than this is refused
/// rather than read.
pub const MAX_ROOT_DIRECTORY_BYTES: usize = 512 * DIRECTORY_ENTRY_BYTES;

/// `ContentEvidence::value` for a confirmed MS-DOS system-file pair. Shared
/// verbatim with [`crate::platform_evidence_fusion`] and
/// [`crate::content_evidence_scope`].
pub const DOS_MSDOS_SYSTEM_FILES: &str = "MS-DOS system files (IO.SYS + MSDOS.SYS)";

/// `ContentEvidence::value` for a confirmed PC DOS / DR-DOS system-file pair.
pub const DOS_PCDOS_SYSTEM_FILES: &str = "PC DOS system files (IBMBIO.COM + IBMDOS.COM)";

// FAT directory-entry attribute bits (byte 11), per both cited sources.
const ATTR_VOLUME_ID: u8 = 0x08;
const ATTR_DIRECTORY: u8 = 0x10;
/// The exact byte a VFAT long-file-name entry carries at offset 11.
const ATTR_LONG_NAME: u8 = 0x0F;

// The first-byte markers of a directory entry.
const ENTRY_END: u8 = 0x00;
const ENTRY_DELETED: u8 = 0xE5;
/// A real leading `0xE5` in a short name is stored as `0x05` so it is not
/// mistaken for the "deleted" marker.
const ENTRY_E5_ESCAPE: u8 = 0x05;

/// FAT type, decided the documented way: by the count of data clusters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatType {
    Fat12,
    Fat16,
}

impl FatType {
    pub fn label(self) -> &'static str {
        match self {
            Self::Fat12 => "FAT12",
            Self::Fat16 => "FAT16",
        }
    }
}

/// Which documented DOS system-file pair was found in the root directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DosBootFamily {
    /// `IO.SYS` and `MSDOS.SYS` both present.
    MsDos,
    /// `IBMBIO.COM` and `IBMDOS.COM` both present.
    PcDos,
}

impl DosBootFamily {
    pub fn label(self) -> &'static str {
        match self {
            Self::MsDos => "MS-DOS",
            Self::PcDos => "PC DOS / DR-DOS",
        }
    }

    /// The neutral [`ContentEvidence::value`] this family emits.
    pub fn evidence_value(self) -> &'static str {
        match self {
            Self::MsDos => DOS_MSDOS_SYSTEM_FILES,
            Self::PcDos => DOS_PCDOS_SYSTEM_FILES,
        }
    }
}

/// The bounded facts read from the boot sector's BIOS Parameter Block, plus
/// the root-directory location derived from them. Only the fields this
/// module actually needs to locate the root directory safely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FatVolume {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub fat_count: u8,
    pub root_entry_count: u16,
    pub total_sectors: u32,
    pub sectors_per_fat: u16,
    pub media_descriptor: u8,
    /// Byte offset of the root directory from the start of the image.
    pub root_directory_offset: u64,
    /// Length of the root directory in bytes (`root_entry_count * 32`).
    pub root_directory_bytes: u32,
    /// Count of data clusters, used only to name the FAT type.
    pub cluster_count: u32,
    pub fat_type: FatType,
}

impl FatVolume {
    fn summary(&self) -> String {
        format!(
            "{} boot sector: {}-byte sectors, {} reserved, {} FAT(s) of {} sector(s), \
             {} root entries, media descriptor {:#04x}",
            self.fat_type.label(),
            self.bytes_per_sector,
            self.reserved_sectors,
            self.fat_count,
            self.sectors_per_fat,
            self.root_entry_count,
            self.media_descriptor,
        )
    }
}

/// What one `.img` / `.ima` (or any caller-provided path) was observed to
/// be. Mirrors the shape of [`crate::disk_format::DiskFormatEvidence`]
/// without claiming a platform: the platform decision is
/// [`crate::platform_evidence_fusion`]'s job, from [`observe_dos_boot_evidence`].
#[derive(Debug, Clone, PartialEq)]
pub struct DosBootInspection {
    /// The parsed FAT volume, when the boot sector and geometry validated.
    pub filesystem: Option<FatVolume>,
    /// Every documented DOS system-file pair found in the root directory.
    /// Empty when a valid FAT volume carries no such pair - which is the
    /// common case and is *not* an error.
    pub boot_families: Vec<DosBootFamily>,
    /// Whether `COMMAND.COM` sits in the root directory. Corroboration for
    /// a family that was already established by its pair; never sufficient
    /// on its own.
    pub command_com_present: bool,
    /// Observed facts, in a person's words.
    pub observations: Vec<String>,
    /// Why nothing was concluded, when the boot sector or geometry did not
    /// validate. `None` whenever `filesystem` is `Some`.
    pub refusal: Option<DiskFormatRefusal>,
    pub bytes_inspected: u64,
    pub read_via_symlink: bool,
}

impl DosBootInspection {
    fn refused(refusal: DiskFormatRefusal, bytes_inspected: u64) -> Self {
        Self {
            filesystem: None,
            boot_families: Vec::new(),
            command_com_present: false,
            observations: Vec::new(),
            refusal: Some(refusal),
            bytes_inspected,
            read_via_symlink: false,
        }
    }

    /// Whether a documented DOS system-file pair was found.
    pub fn has_dos_boot_pair(&self) -> bool {
        !self.boot_families.is_empty()
    }
}

/// The known DOS system / interpreter file names, compared case-insensitively
/// against 8.3 root-directory entries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct FoundSystemFiles {
    io_sys: bool,
    msdos_sys: bool,
    ibmbio_com: bool,
    ibmdos_com: bool,
    command_com: bool,
}

impl FoundSystemFiles {
    fn note(&mut self, name: &str) {
        if name.eq_ignore_ascii_case("IO.SYS") {
            self.io_sys = true;
        } else if name.eq_ignore_ascii_case("MSDOS.SYS") {
            self.msdos_sys = true;
        } else if name.eq_ignore_ascii_case("IBMBIO.COM") {
            self.ibmbio_com = true;
        } else if name.eq_ignore_ascii_case("IBMDOS.COM") {
            self.ibmdos_com = true;
        } else if name.eq_ignore_ascii_case("COMMAND.COM") {
            self.command_com = true;
        }
    }

    fn families(&self) -> Vec<DosBootFamily> {
        let mut families = Vec::new();
        if self.io_sys && self.msdos_sys {
            families.push(DosBootFamily::MsDos);
        }
        if self.ibmbio_com && self.ibmdos_com {
            families.push(DosBootFamily::PcDos);
        }
        families
    }
}

fn cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|flag| flag.load(Ordering::Relaxed))
}

fn malformed(detail: impl Into<String>) -> DiskFormatRefusal {
    DiskFormatRefusal::Malformed {
        detail: detail.into(),
    }
}

fn le_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Parses and validates the BIOS Parameter Block of `boot` (a 512-byte boot
/// sector) far enough to locate the root directory of an `image_len`-byte
/// image, or refuses. Never reads anything; `boot` and `image_len` are the
/// only inputs.
///
/// FAT12/FAT16 only: a BPB whose geometry works out to a FAT32 cluster count
/// (>= 65525), or which uses the FAT32-only 32-bit sectors-per-FAT / 0 root
/// entries form, is refused - FAT32 is out of scope for DOS boot media.
pub fn parse_fat_bpb(boot: &[u8], image_len: u64) -> Result<FatVolume, DiskFormatRefusal> {
    if boot.len() < FAT_BOOT_SECTOR_BYTES {
        return Err(DiskFormatRefusal::Truncated {
            offset: 0,
            wanted: FAT_BOOT_SECTOR_BYTES,
        });
    }

    // The IBM PC boot signature. A FAT disk formatted as bootable media
    // always carries it; its absence means this is not DOS boot media.
    let signature = le_u16(boot, BOOT_SIGNATURE_OFFSET)
        .ok_or_else(|| malformed("the boot sector has no signature field"))?;
    if signature != 0xAA55 {
        return Err(malformed(format!(
            "boot-sector signature is {signature:#06x}, not 0x55 0xAA"
        )));
    }

    // -- BPB fields, at the offsets both cited sources agree on --
    let bytes_per_sector =
        le_u16(boot, 0x0B).ok_or_else(|| malformed("no bytes-per-sector field"))?;
    if bytes_per_sector != FAT_BOOT_SECTOR_BYTES as u16 {
        // Every DOS floppy and every DOS-era hard-disk partition uses
        // 512-byte sectors. Restricting to 512 keeps the root-directory
        // arithmetic exact and matches the existing FAT12 parser.
        return Err(malformed(format!(
            "{bytes_per_sector}-byte sectors; DOS boot media is always 512"
        )));
    }

    let sectors_per_cluster = *boot
        .get(0x0D)
        .ok_or_else(|| malformed("no sectors-per-cluster field"))?;
    if sectors_per_cluster == 0
        || !sectors_per_cluster.is_power_of_two()
        || sectors_per_cluster > 128
    {
        return Err(malformed(format!(
            "{sectors_per_cluster} sectors per cluster is not a power of two in 1..=128"
        )));
    }

    let reserved_sectors =
        le_u16(boot, 0x0E).ok_or_else(|| malformed("no reserved-sectors field"))?;
    if reserved_sectors == 0 {
        return Err(malformed(
            "zero reserved sectors: no room for a boot sector",
        ));
    }

    let fat_count = *boot
        .get(0x10)
        .ok_or_else(|| malformed("no FAT-count field"))?;
    if fat_count == 0 || fat_count > 2 {
        return Err(malformed(format!(
            "{fat_count} file allocation tables is not 1 or 2"
        )));
    }

    let root_entry_count =
        le_u16(boot, 0x11).ok_or_else(|| malformed("no root-entry-count field"))?;
    if root_entry_count == 0 {
        // 0 is the FAT32-only form.
        return Err(malformed(
            "zero root-directory entries: a FAT32 BPB, not FAT12/FAT16",
        ));
    }
    if !root_entry_count.is_multiple_of(16) {
        return Err(malformed(format!(
            "{root_entry_count} root entries is not a multiple of 16"
        )));
    }
    if usize::from(root_entry_count) * DIRECTORY_ENTRY_BYTES > MAX_ROOT_DIRECTORY_BYTES {
        return Err(malformed(format!(
            "{root_entry_count} root entries is past the {MAX_ROOT_DIRECTORY_BYTES}-byte \
             root-directory inspection limit"
        )));
    }

    let total_sectors_16 =
        le_u16(boot, 0x13).ok_or_else(|| malformed("no 16-bit total-sectors field"))?;
    let total_sectors_32 =
        le_u32(boot, 0x20).ok_or_else(|| malformed("no 32-bit total-sectors field"))?;
    let total_sectors = if total_sectors_16 != 0 {
        u32::from(total_sectors_16)
    } else {
        total_sectors_32
    };
    if total_sectors == 0 {
        return Err(malformed("both total-sectors fields are zero"));
    }

    let media_descriptor = *boot
        .get(0x15)
        .ok_or_else(|| malformed("no media-descriptor field"))?;
    if media_descriptor < 0xF0 {
        return Err(malformed(format!(
            "media descriptor {media_descriptor:#04x} is not a standard 0xF0..=0xFF value"
        )));
    }

    let sectors_per_fat =
        le_u16(boot, 0x16).ok_or_else(|| malformed("no sectors-per-FAT field"))?;
    if sectors_per_fat == 0 {
        // 0 here is the FAT32-only form (32-bit field at 0x24).
        return Err(malformed(
            "zero 16-bit sectors-per-FAT: a FAT32 BPB, not FAT12/FAT16",
        ));
    }

    // -- Root-directory location, all checked arithmetic --
    let first_root_sector = u64::from(reserved_sectors)
        .checked_add(
            u64::from(fat_count)
                .checked_mul(u64::from(sectors_per_fat))
                .ok_or_else(|| malformed("FAT region size overflows"))?,
        )
        .ok_or_else(|| malformed("metadata region size overflows"))?;
    let root_directory_offset = first_root_sector
        .checked_mul(u64::from(bytes_per_sector))
        .ok_or_else(|| malformed("root-directory offset overflows"))?;
    let root_directory_bytes = u32::from(root_entry_count)
        .checked_mul(DIRECTORY_ENTRY_BYTES as u32)
        .ok_or_else(|| malformed("root-directory size overflows"))?;

    if root_directory_offset > MAX_DISK_FORMAT_OFFSET {
        return Err(malformed(format!(
            "root directory starts at byte {root_directory_offset}, past the \
             {MAX_DISK_FORMAT_OFFSET}-byte inspection limit"
        )));
    }
    let root_directory_end = root_directory_offset
        .checked_add(u64::from(root_directory_bytes))
        .ok_or_else(|| malformed("root-directory end overflows"))?;
    if root_directory_end > image_len {
        return Err(DiskFormatRefusal::Truncated {
            offset: root_directory_offset,
            wanted: root_directory_bytes as usize,
        });
    }

    // The root directory occupies whole sectors; data begins after it.
    let root_dir_sectors = u64::from(root_entry_count) / 16;
    let first_data_sector = first_root_sector
        .checked_add(root_dir_sectors)
        .ok_or_else(|| malformed("first data sector overflows"))?;
    if first_data_sector >= u64::from(total_sectors) {
        return Err(malformed(
            "the reserved, FAT and root-directory regions leave no data region",
        ));
    }
    let declared_bytes = u64::from(total_sectors)
        .checked_mul(u64::from(bytes_per_sector))
        .ok_or_else(|| malformed("declared image size overflows"))?;
    if declared_bytes > image_len.saturating_add(u64::from(bytes_per_sector)) {
        // The BPB describes a bigger disk than the file could possibly hold.
        return Err(DiskFormatRefusal::GeometryMismatch {
            declared_bytes,
            actual_bytes: image_len,
        });
    }

    let data_sectors = u64::from(total_sectors) - first_data_sector;
    let cluster_count = u32::try_from(data_sectors / u64::from(sectors_per_cluster))
        .map_err(|_| malformed("cluster count overflows"))?;
    // The documented FAT-type boundaries (OSDev "FAT"): < 4085 is FAT12,
    // < 65525 is FAT16, anything more is FAT32 - out of scope here.
    let fat_type = if cluster_count < 4085 {
        FatType::Fat12
    } else if cluster_count < 65525 {
        FatType::Fat16
    } else {
        return Err(malformed(format!(
            "{cluster_count} data clusters indicates FAT32, which is out of scope for DOS \
             boot media"
        )));
    };

    Ok(FatVolume {
        bytes_per_sector,
        sectors_per_cluster,
        reserved_sectors,
        fat_count,
        root_entry_count,
        total_sectors,
        sectors_per_fat,
        media_descriptor,
        root_directory_offset,
        root_directory_bytes,
        cluster_count,
        fat_type,
    })
}

/// The 8.3 name of one 32-byte directory entry, upper-cased, or `None` when
/// the entry is the end marker, a deleted entry, a long-file-name fragment,
/// a volume label, a subdirectory, or otherwise not a plain file name.
///
/// Volume labels are deliberately skipped: a volume label reading `MSDOS`
/// or `DOS` is not evidence of anything.
pub fn short_name(entry: &[u8]) -> Option<String> {
    if entry.len() < DIRECTORY_ENTRY_BYTES {
        return None;
    }
    match entry[0] {
        ENTRY_END | ENTRY_DELETED => return None,
        _ => {}
    }
    let attr = entry[11];
    if attr == ATTR_LONG_NAME {
        return None;
    }
    if attr & ATTR_VOLUME_ID != 0 || attr & ATTR_DIRECTORY != 0 {
        return None;
    }

    let mut base = [0_u8; 8];
    base.copy_from_slice(&entry[0..8]);
    if base[0] == ENTRY_E5_ESCAPE {
        base[0] = ENTRY_DELETED;
    }
    let ext = &entry[8..11];

    let clean = |bytes: &[u8]| -> Option<String> {
        let trimmed: Vec<u8> = bytes.iter().copied().take_while(|&b| b != b' ').collect();
        if trimmed.iter().any(|&b| b < 0x20 || b == 0x7F) {
            return None;
        }
        Some(
            trimmed
                .iter()
                .map(|&b| b.to_ascii_uppercase() as char)
                .collect(),
        )
    };

    let base_str = clean(&base)?;
    let ext_str = clean(ext)?;
    if base_str.is_empty() {
        return None;
    }
    if ext_str.is_empty() {
        Some(base_str)
    } else {
        Some(format!("{base_str}.{ext_str}"))
    }
}

/// Scans an already-read root-directory byte region for the known DOS
/// system / interpreter file names. Stops at the first end-of-directory
/// marker. Pure: `region` is the only input.
fn scan_root_directory(region: &[u8]) -> FoundSystemFiles {
    let mut found = FoundSystemFiles::default();
    for entry in region.chunks_exact(DIRECTORY_ENTRY_BYTES) {
        if entry[0] == ENTRY_END {
            break;
        }
        if let Some(name) = short_name(entry) {
            found.note(&name);
        }
    }
    found
}

/// Inspects `path` for DOS-family boot evidence: validates the FAT12/FAT16
/// boot sector, locates the root directory, and reports which documented
/// DOS system-file pairs (if any) it contains.
///
/// Dispatch is the caller's business (see
/// [`crate::ingestion::discovery`]); this function never looks at the
/// path's extension or file name.
pub fn inspect_dos_boot_media(
    path: &Path,
    trusted: &TrustedRoots,
    cancel: Option<&AtomicBool>,
) -> DosBootInspection {
    if cancelled(cancel) {
        return DosBootInspection::refused(DiskFormatRefusal::Cancelled, 0);
    }

    let mut file = match open_bounded_read(path, trusted) {
        Ok(file) => file,
        Err(refusal) => {
            return DosBootInspection::refused(
                DiskFormatRefusal::NotReadable {
                    code: refusal.code(),
                    detail: refusal.detail(),
                },
                0,
            );
        }
    };
    let read_via_symlink = file.resolved_via_symlink();
    let image_len = file.len();
    let mut bytes_inspected = 0_u64;

    let boot = match file.read_exact_at(0, FAT_BOOT_SECTOR_BYTES, MAX_DISK_FORMAT_READ_CHUNK) {
        Some(bytes) => {
            bytes_inspected += FAT_BOOT_SECTOR_BYTES as u64;
            bytes
        }
        None => {
            return DosBootInspection::refused(
                DiskFormatRefusal::Truncated {
                    offset: 0,
                    wanted: FAT_BOOT_SECTOR_BYTES,
                },
                bytes_inspected,
            );
        }
    };

    let volume = match parse_fat_bpb(&boot, image_len) {
        Ok(volume) => volume,
        Err(refusal) => {
            let mut refused = DosBootInspection::refused(refusal, bytes_inspected);
            refused.read_via_symlink = read_via_symlink;
            return refused;
        }
    };

    // Read the root directory in bounded chunks. parse_fat_bpb has already
    // proven it begins within MAX_DISK_FORMAT_OFFSET, is no larger than
    // MAX_ROOT_DIRECTORY_BYTES, and lies wholly inside the file.
    let mut region: Vec<u8> = Vec::with_capacity(volume.root_directory_bytes as usize);
    while (region.len() as u32) < volume.root_directory_bytes {
        if cancelled(cancel) {
            let mut refused =
                DosBootInspection::refused(DiskFormatRefusal::Cancelled, bytes_inspected);
            refused.read_via_symlink = read_via_symlink;
            return refused;
        }
        let remaining = volume.root_directory_bytes - region.len() as u32;
        let want = remaining.min(MAX_DISK_FORMAT_READ_CHUNK as u32) as usize;
        if bytes_inspected + want as u64 > MAX_DISK_FORMAT_BYTES_READ {
            let mut refused = DosBootInspection::refused(
                malformed("reading the root directory would exceed the inspection budget"),
                bytes_inspected,
            );
            refused.read_via_symlink = read_via_symlink;
            return refused;
        }
        let offset = volume.root_directory_offset + region.len() as u64;
        match file.read_exact_at(offset, want, MAX_DISK_FORMAT_READ_CHUNK) {
            Some(chunk) => {
                bytes_inspected += want as u64;
                region.extend_from_slice(&chunk);
            }
            None => {
                let mut refused = DosBootInspection::refused(
                    DiskFormatRefusal::Truncated {
                        offset,
                        wanted: want,
                    },
                    bytes_inspected,
                );
                refused.read_via_symlink = read_via_symlink;
                return refused;
            }
        }
    }

    let found = scan_root_directory(&region);
    let families = found.families();

    let mut observations = vec![
        volume.summary(),
        format!(
            "Root directory at byte offset {}, {} bytes ({} entries)",
            volume.root_directory_offset, volume.root_directory_bytes, volume.root_entry_count
        ),
    ];
    for family in &families {
        observations.push(match family {
            DosBootFamily::MsDos => {
                "MS-DOS system files present in the root directory: IO.SYS and MSDOS.SYS"
                    .to_string()
            }
            DosBootFamily::PcDos => {
                "PC DOS / DR-DOS system files present in the root directory: IBMBIO.COM and \
                 IBMDOS.COM"
                    .to_string()
            }
        });
    }
    if found.command_com {
        observations.push(if families.is_empty() {
            "COMMAND.COM is present, but with no IO.SYS+MSDOS.SYS or IBMBIO.COM+IBMDOS.COM \
             pair it is not sufficient for a DOS boot-media claim"
                .to_string()
        } else {
            "COMMAND.COM is also present (the DOS command interpreter - corroboration, not \
             proof on its own)"
                .to_string()
        });
    }
    if families.is_empty() {
        observations.push(
            "A valid FAT filesystem, but no documented DOS system-file pair in the root \
             directory - FAT structure alone is not DOS evidence"
                .to_string(),
        );
    }

    DosBootInspection {
        filesystem: Some(volume),
        boot_families: families,
        command_com_present: found.command_com,
        observations,
        refusal: None,
        bytes_inspected,
        read_via_symlink,
    }
}

/// Neutral evidence: one `Strong` [`ContentEvidenceKind::BootStructure`]
/// fact per documented DOS system-file pair found - and nothing at all
/// otherwise, no matter how valid the FAT filesystem is or whether
/// `COMMAND.COM` alone was present. Never emits a `ProductCode` and never
/// names a platform; [`crate::platform_evidence_fusion`] is what maps
/// these values to the canonical `DOS` platform.
pub fn observe_dos_boot_evidence(inspection: &DosBootInspection) -> Vec<ContentEvidence> {
    inspection
        .boot_families
        .iter()
        .map(|family| {
            let detail = match family {
                DosBootFamily::MsDos => {
                    "IO.SYS and MSDOS.SYS both present as regular files in the FAT root directory"
                }
                DosBootFamily::PcDos => {
                    "IBMBIO.COM and IBMDOS.COM both present as regular files in the FAT root \
                     directory"
                }
            };
            ContentEvidence::new(
                ContentEvidenceKind::BootStructure,
                family.evidence_value(),
                ContentEvidenceConfidence::Strong,
                detail,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests;
