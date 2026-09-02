//! The one read-only disk-image format evidence layer.
//!
//! Platform detection and disc identity both need to answer "what kind of image
//! is this, really?" without extracting, mounting or decoding it. Before this
//! module, each answered it privately: the platform registry compared magic
//! bytes at fixed offsets, and `game_identity` had its own per-format readers.
//! Fixed-offset comparison cannot express a *structural* format - one that is
//! recognised by a set of interdependent header fields rather than a constant -
//! and duplicating structural rules per caller is how two callers come to
//! disagree about the same file.
//!
//! So structural formats live here, once, and both callers consume the same
//! [`DiskFormatEvidence`].
//!
//! # What this module does not do
//!
//! It reads a bounded prefix of a file and validates header structure. It never
//! creates, writes, extracts, mounts or decodes an image, never spawns a
//! process, and never touches a network. It never reconstructs a disk: the
//! deepest it goes is walking a fixed-size table of record headers to confirm
//! the table is internally consistent.
//!
//! # Reading is delegated, not reinvented
//!
//! Every byte comes through [`crate::safe_read::open_bounded_read`], which is
//! the single place this build decides what may be opened. A symlink is followed
//! only under that module's trusted-root policy; this module adds no policy of
//! its own and cannot bypass it.
//!
//! # Honesty about what a structure proves
//!
//! A format's structure being valid is not the same as a platform being
//! certain. An Atari ST `.st` image is a raw FAT12 floppy dump whose boot sector
//! is laid out identically to a PC DOS floppy's, so a valid one proves "this is
//! a plausible ST-geometry FAT12 floppy", not "this is an Atari ST disk". A
//! `.stx` image, by contrast, is the Pasti preservation format, which exists
//! only for Atari ST media, so a valid one does prove the platform.
//!
//! [`DiskFormatEvidence::conclusive`] is how that difference is carried to the
//! caller instead of being flattened away.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use crate::platform::DetectionConfidence;
use crate::safe_read::{SafeFile, TrustedRoots, open_bounded_read};

pub mod atari_st;
pub mod atari_stx;
pub mod crt;
pub mod d64;
pub mod d88;
pub mod dc42;
pub mod dfs;
pub mod dsk;
pub mod fds;
pub mod hdi;
pub mod scl;
pub mod trd;
pub mod x68000;

#[cfg(test)]
mod tests;

// --- Limits ---------------------------------------------------------------
//
// Every limit is explicit, checked before the read it governs, and asserted by
// `limits_are_internally_consistent`.

/// The most this module will ever read from one file, across every step of one
/// inspection. Chosen so a full inspection is a handful of small reads: a boot
/// sector, or one header plus a bounded table of record headers.
pub const MAX_DISK_FORMAT_BYTES_READ: u64 = 4 * 1024 * 1024;

/// The largest single read.
pub const MAX_DISK_FORMAT_READ_CHUNK: usize = 1024;

/// The furthest offset any ordinary structural read starts at. D64 opts into
/// its own larger, still bounded sector offset because its BAM is in the
/// middle of the image.
pub const MAX_DISK_FORMAT_OFFSET: u64 = 32 * 1024;

/// The largest file this module will treat as a raw floppy image. An Atari ST
/// floppy tops out at 1.44 MB; the slack covers unusual track counts without
/// admitting whole hard-disk images.
pub const MAX_RAW_FLOPPY_BYTES: u64 = 4 * 1024 * 1024;

/// The largest file this module will treat as a Pasti image. Flux-level
/// preservation dumps are larger than the disk they describe.
pub const MAX_PASTI_BYTES: u64 = 32 * 1024 * 1024;

/// The fixed size of one side in the common raw FDS representation.
pub const FDS_SIDE_BYTES: u64 = 65_500;

/// The largest raw FDS image this bounded evidence layer will inspect.
pub const MAX_FDS_BYTES: u64 = 4 * 1024 * 1024;

/// A raw FDS side cannot contain more file pairs than fit before its padding.
pub const MAX_FDS_FILES_PER_SIDE: u8 = 64;

/// Sector size this module accepts for a raw floppy. Atari ST TOS floppies are
/// always 512-byte sectors; accepting others would weaken the check for no real
/// coverage.
pub const FLOPPY_SECTOR_BYTES: u32 = 512;

/// A TR-DOS sector is always 256 bytes; a TR-DOS track is always 16 of them.
pub const TRDOS_SECTOR_BYTES: u64 = 256;
pub const TRDOS_TRACK_BYTES: u64 = 16 * TRDOS_SECTOR_BYTES;

/// The largest `.trd` this module will treat as a TR-DOS disk. The biggest
/// standard geometry is 80 tracks x 2 sides x 16 sectors x 256 = 655360; the
/// slack covers over-formatted disks (a few extra tracks) without admitting
/// anything hard-disk sized.
pub const MAX_TRD_BYTES: u64 = 1024 * 1024;

/// The largest `.scl` this module will read. An `.scl` can hold up to a full
/// double-sided 80-track disk of files plus its own table and checksum; the
/// slack keeps unusual packers inside the bound.
pub const MAX_SCL_BYTES: u64 = 4 * 1024 * 1024;

/// The most files a TR-DOS catalogue (and therefore an `.scl` archive) can
/// hold: the directory area is 8 sectors of 16-byte entries.
pub const TRDOS_MAX_FILES: u8 = 128;

/// The most track records a Pasti table may declare. Two sides of 84 tracks is
/// already beyond any real Atari ST disk.
pub const MAX_PASTI_TRACK_RECORDS: usize = 168;

/// The largest file this module will treat as a CPCEMU `.dsk` image. A
/// double-sided 80-track extended disk with oversized tracks is comfortably
/// under this; anything larger is not a floppy image.
pub const MAX_DSK_BYTES: u64 = 8 * 1024 * 1024;

/// The most `track x side` entries a `.dsk` header may declare. 85 tracks on
/// two sides is already past any real drive.
pub const MAX_DSK_TRACK_ENTRIES: usize = 170;
pub const MAX_DC42_BYTES: u64 = 4 * 1024 * 1024;

/// The largest file this module will treat as a D88 floppy container.
pub const MAX_D88_BYTES: u64 = 8 * 1024 * 1024;

/// The fixed D88 header and track-table size.
pub const D88_HEADER_BYTES: usize = 0x2b0;

/// The maximum number of D88 track-table entries (82 tracks x 2 heads).
pub const MAX_D88_TRACK_ENTRIES: usize = 164;
/// The largest HDI/NHD image this bounded evidence layer will inspect.
pub const MAX_HARD_DISK_IMAGE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// The fixed size of a `.dsk` disk-information block and of each track-
/// information block.
pub const DSK_INFO_BLOCK_BYTES: usize = 256;

/// Which structural format was recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskFormat {
    /// A sector-order Commodore 1541 disk image (`.d64`). The media is shared
    /// by C64, C128 and VIC-20 software, so this is deliberately family-level
    /// evidence rather than a machine claim.
    Commodore1541D64,
    /// A raw sector dump of an Atari ST floppy: no container header at all,
    /// recognised from its FAT12 boot-sector geometry and its exact length.
    AtariStRawFloppy,
    /// The Pasti (`.stx`) preservation container.
    AtariStPasti,
    /// A structurally valid raw Famicom Disk System side image.
    FamicomDiskSystem,
    /// A CPCEMU `.dsk` container (standard or extended), recognised from its
    /// `MV - CPC`/`EXTENDED CPC DSK` disk-information block and a track table
    /// that is internally consistent with the file's length. This container
    /// is shared by the Amstrad CPC, ZX Spectrum +3, Amstrad PCW and other
    /// systems, so on its own it does **not** settle a platform.
    CpcEmuDsk,
    /// A CPCEMU `.dsk` container whose track 0 additionally carries a valid
    /// `+3DOS`/`PCW` disk-specification block (disk type, geometry and
    /// reserved fields all consistent, and agreeing with the container's own
    /// track descriptors). That structure is specific to the Spectrum +3 /
    /// PCW disk family; Amstrad CPC AMSDOS disks do not carry it.
    SpectrumPlus3Disk,
    /// A raw TR-DOS disk image (`.trd`), recognised from the Beta Disk
    /// system/volume descriptor in track 0's ninth sector (the `0x10`
    /// TR-DOS identifier, a documented disk-type byte, and geometry that
    /// agrees with the file's length). TR-DOS is a ZX Spectrum-family
    /// filesystem; no other platform writes this descriptor.
    SpectrumTrDosDisk,
    /// The `.scl` ("SINCLAIR") archive of TR-DOS files: an 8-byte signature,
    /// a one-byte file count, a bounded 14-byte-per-entry directory, and a
    /// payload whose size the entries account for exactly. Specific to the
    /// TR-DOS / ZX Spectrum ecosystem.
    SpectrumSclArchive,
    /// A structurally valid D88 disk container shared by Japanese computer
    /// families; it does not identify a platform by itself.
    D88Container,
    /// A structurally valid Anex86 HDI hard-disk container.
    HdiContainer,
    /// A structurally valid T98-Next NHD hard-disk container.
    NhdContainer,
    /// A valid raw Acorn DFS catalogue in an `.ssd` or `.dsd` sector dump.
    /// DFS is shared by BBC-family machines and never settles one machine.
    AcornDfsDisk,
    /// A structurally valid VICE/CCS64 C64 CRT cartridge container. The
    /// cartridge may be usable across more than one Commodore 8-bit machine.
    CommodoreCrt,
    /// A structurally validated raw X68000 XDF floppy image.
    X68000Xdf,
    /// A structurally validated X68000 DIM container.
    X68000Dim,
    /// A structurally valid Macintosh Disk Copy 4.2 image.
    MacintoshDiskCopy42,
}

impl DiskFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Commodore1541D64 => "Commodore 1541 D64 disk image",
            Self::AtariStRawFloppy => "Atari ST raw floppy image",
            Self::AtariStPasti => "Atari ST Pasti (STX) image",
            Self::FamicomDiskSystem => "Famicom Disk System image",
            Self::CpcEmuDsk => "CPCEMU DSK disk image",
            Self::SpectrumPlus3Disk => "ZX Spectrum +3 (+3DOS) disk image",
            Self::SpectrumTrDosDisk => "ZX Spectrum TR-DOS disk image",
            Self::SpectrumSclArchive => "ZX Spectrum SCL (SINCLAIR) archive",
            Self::D88Container => "D88 disk container",
            Self::HdiContainer => "HDI hard-disk container",
            Self::NhdContainer => "NHD hard-disk container",
            Self::AcornDfsDisk => "Acorn DFS disk image",
            Self::CommodoreCrt => "Commodore CRT cartridge",
            Self::X68000Xdf => "Sharp X68000 XDF floppy image",
            Self::X68000Dim => "Sharp X68000 DIM floppy container",
            Self::MacintoshDiskCopy42 => "Macintosh Disk Copy 4.2 image",
        }
    }

    /// The canonical platform identifier this format belongs to.
    pub fn platform(self) -> &'static str {
        match self {
            Self::Commodore1541D64 => "Commodore disk media",
            Self::AtariStRawFloppy | Self::AtariStPasti => "AtariST",
            Self::FamicomDiskSystem => "NES",
            // The bare CPCEMU container narrows towards Amstrad CPC (its
            // authoring system and dominant use) without settling it - see
            // `proves_platform`, which is `false` here.
            Self::CpcEmuDsk => "Amstrad CPC",
            Self::SpectrumPlus3Disk => "ZX Spectrum",
            Self::SpectrumTrDosDisk | Self::SpectrumSclArchive => "ZX Spectrum",
            Self::D88Container => "NEC PC-8801",
            Self::HdiContainer | Self::NhdContainer => "PC-98",
            Self::AcornDfsDisk => "BBC Micro",
            Self::CommodoreCrt => "Commodore 64",
            Self::X68000Xdf | Self::X68000Dim => "Sharp X68000",
            Self::MacintoshDiskCopy42 => "Macintosh",
        }
    }

    /// Whether the format's structure, on its own, proves the platform.
    ///
    /// `false` for a raw floppy dump: its boot sector is a FAT12 BPB, the same
    /// structure a PC DOS floppy of the same geometry carries, so the structure
    /// narrows the answer without settling it. `true` for Pasti, which exists
    /// only for Atari ST media.
    ///
    /// `false` for a bare CPCEMU `.dsk`: the container is shared across the
    /// CPC, Spectrum +3 and PCW families, so a valid one narrows without
    /// settling. `true` for a `.dsk` carrying a valid +3DOS/PCW disk
    /// specification - that structure is specific to that disk family and
    /// AMSDOS/CPC disks do not have it.
    pub fn proves_platform(self) -> bool {
        match self {
            Self::Commodore1541D64 | Self::AtariStRawFloppy | Self::CpcEmuDsk => false,
            Self::AtariStPasti
            | Self::FamicomDiskSystem
            | Self::SpectrumPlus3Disk
            | Self::SpectrumTrDosDisk
            | Self::SpectrumSclArchive
            | Self::MacintoshDiskCopy42 => true,
            Self::D88Container => false,
            Self::HdiContainer | Self::NhdContainer => false,
            Self::AcornDfsDisk => false,
            Self::CommodoreCrt => false,
            Self::X68000Xdf | Self::X68000Dim => true,
        }
    }
}

/// Why no format was claimed. Each variant is a distinct, explainable outcome
/// rather than one opaque failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskFormatRefusal {
    /// The file could not be opened under the bounded-read policy. Carries that
    /// module's own reason - a target outside the trusted roots, a broken or
    /// looping symlink, a directory, a device.
    NotReadable { code: &'static str, detail: String },
    /// This module has no adapter for the extension.
    NoAdapter { extension: String },
    /// The file has no extension to dispatch on.
    NoExtension,
    /// Smaller than the structure it would have to contain.
    TooSmall { length: u64, minimum: u64 },
    /// Larger than this module will treat as this kind of image.
    TooLarge { length: u64, maximum: u64 },
    /// A raw floppy image must be a whole number of sectors.
    NotSectorAligned { length: u64, sector_bytes: u32 },
    /// A required read failed or returned short - a truncated file.
    Truncated { offset: u64, wanted: usize },
    /// The structure was read but is not valid. The detail names the field.
    Malformed { detail: String },
    /// The declared geometry and the file length disagree.
    GeometryMismatch {
        declared_bytes: u64,
        actual_bytes: u64,
    },
    /// Cancelled by the caller between bounded steps.
    Cancelled,
}

impl DiskFormatRefusal {
    pub fn detail(&self) -> String {
        match self {
            Self::NotReadable { detail, .. } => detail.clone(),
            Self::NoAdapter { extension } => {
                format!("no disk-format adapter handles `.{extension}`")
            }
            Self::NoExtension => "the file has no extension to identify it by".to_string(),
            Self::TooSmall { length, minimum } => format!(
                "the file is {length} bytes, below the {minimum} bytes this structure needs"
            ),
            Self::TooLarge { length, maximum } => format!(
                "the file is {length} bytes, above the {maximum}-byte limit for this format"
            ),
            Self::NotSectorAligned {
                length,
                sector_bytes,
            } => format!("{length} bytes is not a whole number of {sector_bytes}-byte sectors"),
            Self::Truncated { offset, wanted } => {
                format!("the file ends before {wanted} bytes could be read at offset {offset}")
            }
            Self::Malformed { detail } => detail.clone(),
            Self::GeometryMismatch {
                declared_bytes,
                actual_bytes,
            } => format!(
                "the boot sector declares {declared_bytes} bytes of sectors but the file is \
                 {actual_bytes} bytes"
            ),
            Self::Cancelled => "the inspection was cancelled".to_string(),
        }
    }

    /// A stable code, for counting refusals in an audit.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotReadable { code, .. } => code,
            Self::NoAdapter { .. } => "no_adapter",
            Self::NoExtension => "no_extension",
            Self::TooSmall { .. } => "too_small",
            Self::TooLarge { .. } => "too_large",
            Self::NotSectorAligned { .. } => "not_sector_aligned",
            Self::Truncated { .. } => "truncated",
            Self::Malformed { .. } => "malformed",
            Self::GeometryMismatch { .. } => "geometry_mismatch",
            Self::Cancelled => "cancelled",
        }
    }
}

/// A raw floppy image's declared geometry, as read from its boot sector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FloppyGeometry {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub fat_count: u8,
    pub root_directory_entries: u16,
    pub total_sectors: u32,
    pub sectors_per_fat: u16,
    pub sectors_per_track: u16,
    pub sides: u16,
    /// Derived, not stored: `total_sectors / (sectors_per_track * sides)`.
    pub tracks: u16,
}

impl FloppyGeometry {
    /// A person-readable summary, e.g. "80 tracks x 2 sides x 9 sectors".
    pub fn summary(&self) -> String {
        format!(
            "{} tracks x {} side(s) x {} sectors of {} bytes ({} sectors total)",
            self.tracks,
            self.sides,
            self.sectors_per_track,
            self.bytes_per_sector,
            self.total_sectors
        )
    }
}

/// What one Pasti container declared about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PastiLayout {
    pub version: u16,
    pub tool: u16,
    pub revision: u8,
    /// Track records the header declares.
    pub declared_track_records: u8,
    /// Track records actually walked and found internally consistent.
    pub validated_track_records: usize,
    /// Total sectors the validated track records declare between them.
    pub declared_sectors: u32,
}

/// What one CPCEMU `.dsk` declared and what was walked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DskLayout {
    /// `true` for an `EXTENDED CPC DSK` container, `false` for the standard
    /// `MV - CPC` one.
    pub extended: bool,
    pub declared_tracks: u8,
    pub declared_sides: u8,
    /// Track-information blocks actually walked and found internally
    /// consistent with the file's length.
    pub validated_tracks: usize,
    /// Sectors the validated track descriptors declare between them.
    pub declared_sectors: u32,
    /// `true` when track 0 carries a valid `+3DOS`/`PCW` disk-specification
    /// block. Never `true` for a CPC AMSDOS disk.
    pub plus3dos_disk_spec: bool,
    /// The disk-type byte from that specification, when present (0 / 3 =
    /// Spectrum +3 / PCW family, 1 / 2 = Amstrad CPC family).
    pub plus3dos_disk_type: Option<u8>,
    /// `true` when the first sector's whole-sector checksum marks it a
    /// bootable Spectrum +3 disk (sum of the 512 bytes mod 256 == 3).
    pub plus3_bootable: bool,
}

/// What a validated sector-order 1541 image declared. Names and directory
/// filenames are retained as provenance only; they never identify a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct D64Layout {
    pub tracks: u8,
    pub sectors: u16,
    pub has_error_info_tail: bool,
    pub disk_name: [u8; 16],
    pub disk_id: [u8; 2],
    pub dos_type: [u8; 2],
    pub directory: Vec<D64DirectoryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct D64DirectoryEntry {
    pub file_type: u8,
    pub closed: bool,
    pub locked: bool,
    pub start_track: u8,
    pub start_sector: u8,
    pub blocks: u16,
    pub filename: [u8; 16],
}

/// What one TR-DOS disk's system/volume descriptor (track 0, sector 9)
/// declared. Every field is read from that one 256-byte sector; nothing is
/// derived from a directory walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TrDosDescriptor {
    /// Documented disk-type byte: `0x16` 80-track DS, `0x17` 40-track DS,
    /// `0x18` 80-track SS, `0x19` 40-track SS.
    pub disk_type: u8,
    pub tracks_per_side: u16,
    pub sides: u8,
    /// Number of catalogued files (0..=128).
    pub file_count: u8,
    /// Free-space cursor, as the descriptor states it.
    pub first_free_sector: u8,
    pub first_free_track: u16,
    pub free_sectors: u16,
    /// The 8-byte disk label, when it is printable ASCII.
    pub label: Option<[u8; 8]>,
}

/// What one `.scl` archive's header and directory declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SclLayout {
    pub file_count: u8,
    /// Total 256-byte sectors the directory entries account for between them.
    pub declared_sectors: u32,
    /// `true` when the file length matches "directory + payload + 4-byte
    /// trailing checksum" rather than "directory + payload" exactly. The
    /// checksum value itself is not verified (that needs a whole-file read,
    /// outside this module's budget).
    pub has_trailing_checksum: bool,
}

/// What one D88 container declared and what its bounded track walk validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct D88Layout {
    /// The fixed-width disk name field, including unused bytes.
    pub disk_name: [u8; 17],
    pub write_protected: bool,
    pub media_type: u8,
    pub declared_track_entries: usize,
    pub validated_track_entries: usize,
    pub declared_sectors: u32,
    pub declared_data_bytes: u64,
}

/// Geometry and header facts declared by an HDI or NHD container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HardDiskLayout {
    pub header_bytes: u64,
    pub data_offset: u64,
    pub sector_size: u64,
    pub sectors_per_track: u64,
    pub heads: u64,
    pub cylinders: u64,
    pub declared_payload_bytes: u64,
    pub file_bytes: u64,
    pub version: Option<u8>,
}

/// A validated standard Acorn DFS catalogue side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DfsSideLayout {
    pub total_sectors: u16,
    pub file_count: u8,
    pub title: String,
    pub boot_option: u8,
    pub files: Vec<DfsFileEntry>,
}

/// A DFS file entry. The data payload is not read; its declared extent is
/// checked against the side geometry before this fact is emitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DfsFileEntry {
    pub directory: String,
    pub filename: String,
    pub locked: bool,
    pub load_address: u32,
    pub execution_address: u32,
    pub length: u32,
    pub start_sector: u16,
}

/// The validated catalogue sides in an `.ssd` or `.dsd` image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DfsLayout {
    pub double_sided: bool,
    pub sides: Vec<DfsSideLayout>,
}

/// The validated fixed header and CHIP packets of a CRT cartridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CrtLayout {
    pub header_length: u32,
    pub version: u16,
    pub hardware_type: u16,
    pub exrom: u8,
    pub game: u8,
    pub cartridge_name: String,
    pub packets: usize,
    pub chip_types: Vec<u16>,
    pub banks: Vec<u16>,
    pub total_image_bytes: u64,
}

/// Validated Sharp X68000 floppy layout. DIM stores only present tracks after
/// its header; XDF is a complete raw 2HD image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct X68000Layout {
    pub format_name: &'static str,
    pub bytes_per_sector: u16,
    pub sectors_per_track: u16,
    pub tracks_per_cylinder: u8,
    pub cylinders: u16,
    pub header_bytes: u64,
    pub stored_tracks: u16,
    pub payload_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MacintoshFilesystem {
    Hfs,
    Mfs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Dc42Layout {
    pub name_length: u8,
    pub data_size: u32,
    pub tag_size: u32,
    pub data_checksum: u32,
    pub tag_checksum: u32,
    pub encoding: u8,
    pub format_byte: u8,
    pub payload_offset: u64,
    pub filesystem: Option<MacintoshFilesystem>,
    pub checksums_verified: bool,
}

/// Optional format-specific metadata. Only ever the shape the recognised format
/// actually has - never a lowest common denominator that invents fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum DiskFormatMetadata {
    D64(D64Layout),
    Floppy(FloppyGeometry),
    Pasti(PastiLayout),
    Fds(FdsLayout),
    Dsk(DskLayout),
    TrDos(TrDosDescriptor),
    Scl(SclLayout),
    D88(D88Layout),
    Hdi(HardDiskLayout),
    Nhd(HardDiskLayout),
    Dfs(DfsLayout),
    Crt(CrtLayout),
    X68000(X68000Layout),
    Dc42(Dc42Layout),
}

/// Structural facts from a raw FDS image. The file and disk names are not
/// retained as identity: they are descriptive media fields, not trustworthy
/// release identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FdsLayout {
    pub header_bytes: u8,
    pub sides: u8,
    pub files_per_side: u8,
}

/// The shared result. One shape, whatever the format, so a caller does not need
/// to know which adapter ran.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiskFormatEvidence {
    /// The format recognised, or `None` when none was.
    pub format: Option<DiskFormat>,
    /// The canonical platform the format implies. `None` when no format matched.
    pub platform: Option<&'static str>,
    /// How sure the *platform* is, on the one shared scale. See the module
    /// documentation for why a valid structure is not always Confirmed.
    pub confidence: DetectionConfidence,
    /// Whether the structure alone settles the platform. A caller that ranks
    /// evidence uses this rather than assuming every structural match is
    /// conclusive.
    pub conclusive: bool,
    /// Observed facts, in a person's words. Never a guess or a suggestion.
    pub evidence: Vec<String>,
    /// How many bytes were actually read. Bounded by
    /// [`MAX_DISK_FORMAT_BYTES_READ`], and asserted by the tests.
    pub bytes_inspected: u64,
    /// Why nothing was claimed, when nothing was.
    pub refusal: Option<DiskFormatRefusal>,
    pub metadata: Option<DiskFormatMetadata>,
    /// Whether the bytes came through a symlink the trusted-root policy allowed.
    pub read_via_symlink: bool,
}

impl DiskFormatEvidence {
    /// Nothing claimed, for the stated reason.
    pub fn refused(refusal: DiskFormatRefusal) -> Self {
        Self {
            format: None,
            platform: None,
            confidence: DetectionConfidence::Unknown,
            conclusive: false,
            evidence: Vec::new(),
            bytes_inspected: 0,
            refusal: Some(refusal),
            metadata: None,
            read_via_symlink: false,
        }
    }

    pub fn is_recognised(&self) -> bool {
        self.format.is_some()
    }

    /// A one-line summary for a report.
    pub fn summary(&self) -> String {
        match (&self.format, &self.refusal) {
            (Some(format), _) => format!(
                "{} ({}, {} bytes inspected)",
                format.label(),
                self.confidence.label().to_lowercase(),
                self.bytes_inspected
            ),
            (None, Some(refusal)) => format!("not recognised: {}", refusal.detail()),
            (None, None) => "not recognised".to_string(),
        }
    }
}

/// What the caller already believes about the platform, so the shared result can
/// be honest about agreement and conflict without the adapters knowing anything
/// about folders or filters.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiskFormatContext<'a> {
    /// A canonical platform the surrounding evidence already points at - in
    /// practice, an exact folder alias.
    pub folder_platform: Option<&'a str>,
}

/// Inspects `path` and reports what structural format it is, if any.
///
/// Dispatch is by extension, because that is what says *which structure to
/// look for*; it is never what decides the answer. A `.st` file whose boot
/// sector does not validate is refused, and the caller is left with extension
/// evidence only.
///
/// `cancel` is checked between bounded steps. Every read goes through
/// [`crate::safe_read`], so `trusted` governs symlinks exactly as it does
/// everywhere else.
pub fn inspect_disk_format(
    path: &Path,
    trusted: &TrustedRoots,
    context: DiskFormatContext<'_>,
    cancel: Option<&AtomicBool>,
) -> DiskFormatEvidence {
    let Some(extension) = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
    else {
        return DiskFormatEvidence::refused(DiskFormatRefusal::NoExtension);
    };
    // Refuse before opening anything when no adapter could use the bytes.
    let adapter = match extension.as_str() {
        "d64" => Adapter::Commodore1541D64,
        "st" => Adapter::AtariStRawFloppy,
        "stx" => Adapter::AtariStPasti,
        "fds" => Adapter::FamicomDiskSystem,
        "dsk" => Adapter::CpcEmuDsk,
        "trd" => Adapter::SpectrumTrDos,
        "scl" => Adapter::SpectrumScl,
        "d88" => Adapter::D88,
        "hdi" => Adapter::Hdi,
        "nhd" => Adapter::Nhd,
        "ssd" => Adapter::AcornDfs {
            double_sided: false,
        },
        "dsd" => Adapter::AcornDfs { double_sided: true },
        "crt" => Adapter::Crt,
        "xdf" => Adapter::X68000Xdf,
        "dim" => Adapter::X68000Dim,
        "dc42" => Adapter::MacintoshDiskCopy42,
        _ => return DiskFormatEvidence::refused(DiskFormatRefusal::NoAdapter { extension }),
    };
    if cancelled(cancel) {
        return DiskFormatEvidence::refused(DiskFormatRefusal::Cancelled);
    }

    let mut file = match open_bounded_read(path, trusted) {
        Ok(file) => file,
        Err(refusal) => {
            return DiskFormatEvidence::refused(DiskFormatRefusal::NotReadable {
                code: refusal.code(),
                detail: refusal.detail(),
            });
        }
    };
    let read_via_symlink = file.resolved_via_symlink();

    let mut reader = BoundedReader::new(&mut file);
    let mut evidence = match adapter {
        Adapter::Commodore1541D64 => d64::inspect(&mut reader, context, cancel),
        Adapter::AtariStRawFloppy => atari_st::inspect(&mut reader, context, cancel),
        Adapter::AtariStPasti => atari_stx::inspect(&mut reader, context, cancel),
        Adapter::FamicomDiskSystem => fds::inspect(&mut reader, context, cancel),
        Adapter::CpcEmuDsk => dsk::inspect(&mut reader, context, cancel),
        Adapter::SpectrumTrDos => trd::inspect(&mut reader, context, cancel),
        Adapter::SpectrumScl => scl::inspect(&mut reader, context, cancel),
        Adapter::D88 => d88::inspect(&mut reader, context, cancel),
        Adapter::Hdi => hdi::inspect_hdi(&mut reader, context, cancel),
        Adapter::Nhd => hdi::inspect_nhd(&mut reader, context, cancel),
        Adapter::AcornDfs { double_sided } => {
            dfs::inspect(&mut reader, context, cancel, double_sided)
        }
        Adapter::Crt => crt::inspect(&mut reader, context, cancel),
        Adapter::X68000Xdf => x68000::inspect_xdf(&mut reader, context, cancel),
        Adapter::X68000Dim => x68000::inspect_dim(&mut reader, context, cancel),
        Adapter::MacintoshDiskCopy42 => dc42::inspect(&mut reader, context, cancel),
    };
    evidence.bytes_inspected = reader.bytes_read;
    evidence.read_via_symlink = read_via_symlink;
    if read_via_symlink && evidence.is_recognised() {
        evidence
            .evidence
            .push("Structure read from a validated symlink target".to_string());
    }
    evidence
}

enum Adapter {
    Commodore1541D64,
    AtariStRawFloppy,
    AtariStPasti,
    FamicomDiskSystem,
    CpcEmuDsk,
    SpectrumTrDos,
    SpectrumScl,
    D88,
    Hdi,
    Nhd,
    AcornDfs { double_sided: bool },
    Crt,
    X68000Xdf,
    X68000Dim,
    MacintoshDiskCopy42,
}

fn cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|flag| flag.load(Ordering::Relaxed))
}

/// A read budget over one already-validated handle.
///
/// Adapters read only through this, so no adapter can exceed the module's total
/// byte budget, read past the offset limit, or ask for a chunk larger than the
/// documented maximum - regardless of what a header claims.
pub struct BoundedReader<'a> {
    file: &'a mut SafeFile,
    bytes_read: u64,
    max_offset: u64,
}

impl<'a> BoundedReader<'a> {
    fn new(file: &'a mut SafeFile) -> Self {
        Self {
            file,
            bytes_read: 0,
            max_offset: MAX_DISK_FORMAT_OFFSET,
        }
    }

    /// The file's length, as observed when it was validated.
    pub fn len(&self) -> u64 {
        self.file.len()
    }

    pub fn is_empty(&self) -> bool {
        self.file.is_empty()
    }

    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    pub(crate) fn set_max_offset(&mut self, max_offset: u64) {
        self.max_offset = max_offset;
    }

    /// Reads exactly `length` bytes at `offset`, or refuses.
    ///
    /// Refuses - without reading - when the chunk is too large, the offset is
    /// beyond the module's limit, or the total budget would be exceeded. All
    /// arithmetic is checked, so a hostile header cannot wrap a bound into
    /// looking satisfied.
    pub fn read_exact_at(
        &mut self,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, DiskFormatRefusal> {
        if length == 0 || length > MAX_DISK_FORMAT_READ_CHUNK {
            return Err(DiskFormatRefusal::Malformed {
                detail: format!(
                    "a {length}-byte read is outside the {MAX_DISK_FORMAT_READ_CHUNK}-byte chunk limit"
                ),
            });
        }
        if offset > self.max_offset {
            return Err(DiskFormatRefusal::Malformed {
                detail: format!(
                    "offset {offset} is past the {}-byte inspection limit",
                    self.max_offset
                ),
            });
        }
        let would_read = self.bytes_read.checked_add(length as u64).ok_or_else(|| {
            DiskFormatRefusal::Malformed {
                detail: "the read budget overflowed".to_string(),
            }
        })?;
        if would_read > MAX_DISK_FORMAT_BYTES_READ {
            return Err(DiskFormatRefusal::Malformed {
                detail: format!(
                    "reading {length} more bytes would exceed the \
                     {MAX_DISK_FORMAT_BYTES_READ}-byte inspection budget"
                ),
            });
        }
        let bytes = self
            .file
            .read_exact_at(offset, length, MAX_DISK_FORMAT_READ_CHUNK)
            .ok_or(DiskFormatRefusal::Truncated {
                offset,
                wanted: length,
            })?;
        self.bytes_read = would_read;
        Ok(bytes)
    }

    /// CRT CHIP headers may be chained through a larger, but still bounded,
    /// cartridge image. Keep that exception local to CRT rather than relaxing
    /// the established offset bound for floppy and hard-disk adapters.
    pub(crate) fn read_exact_at_crt(
        &mut self,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, DiskFormatRefusal> {
        self.read_exact_at_with_offset_limit(offset, length, 32 * 1024 * 1024)
    }

    /// DC42 checksum verification streams a standard floppy payload through
    /// the same bounded reader, while retaining the ordinary offset limit for
    /// every other disk adapter.
    pub(crate) fn read_exact_at_dc42(
        &mut self,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, DiskFormatRefusal> {
        self.read_exact_at_with_offset_limit(offset, length, MAX_DC42_BYTES)
    }

    /// FDS sides are larger than the ordinary structural offset window, but
    /// the format remains bounded by its fixed side geometry and file limit.
    pub(crate) fn read_exact_at_fds(
        &mut self,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, DiskFormatRefusal> {
        self.read_exact_at_with_offset_limit(offset, length, MAX_FDS_BYTES)
    }

    pub(crate) fn read_exact_at_with_offset_limit(
        &mut self,
        offset: u64,
        length: usize,
        offset_limit: u64,
    ) -> Result<Vec<u8>, DiskFormatRefusal> {
        if length == 0 || length > MAX_DISK_FORMAT_READ_CHUNK {
            return Err(DiskFormatRefusal::Malformed {
                detail: format!(
                    "a {length}-byte read is outside the {MAX_DISK_FORMAT_READ_CHUNK}-byte chunk limit"
                ),
            });
        }
        if offset > offset_limit {
            return Err(DiskFormatRefusal::Malformed {
                detail: format!("offset {offset} is past the {offset_limit}-byte inspection limit"),
            });
        }
        let would_read = self.bytes_read.checked_add(length as u64).ok_or_else(|| {
            DiskFormatRefusal::Malformed {
                detail: "the read budget overflowed".to_string(),
            }
        })?;
        if would_read > MAX_DISK_FORMAT_BYTES_READ {
            return Err(DiskFormatRefusal::Malformed {
                detail: format!(
                    "reading {length} more bytes would exceed the {MAX_DISK_FORMAT_BYTES_READ}-byte inspection budget"
                ),
            });
        }
        let bytes = self
            .file
            .read_exact_at(offset, length, MAX_DISK_FORMAT_READ_CHUNK)
            .ok_or(DiskFormatRefusal::Truncated {
                offset,
                wanted: length,
            })?;
        self.bytes_read = would_read;
        Ok(bytes)
    }
}

/// Reads a little-endian `u16` from `bytes` at `offset`, or `None` if the slice
/// is too short. Bounds-checked rather than indexed.
pub(crate) fn le_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let end = offset.checked_add(2)?;
    let slice = bytes.get(offset..end)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

/// Reads a little-endian `u32` from `bytes` at `offset`, or `None`.
pub(crate) fn le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let slice = bytes.get(offset..end)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

pub(crate) fn be_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let end = offset.checked_add(2)?;
    let slice = bytes.get(offset..end)?;
    Some(u16::from_be_bytes([slice[0], slice[1]]))
}

pub(crate) fn be_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let slice = bytes.get(offset..end)?;
    Some(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// The confidence a recognised format deserves, given what else is known.
///
/// This is where the milestone's confidence rules live, in one place:
///
/// - a format that proves its platform is Confirmed on its own;
/// - a format that only narrows it - a raw FAT12 floppy dump - is Probable, and
///   reaches Confirmed only when a folder alias names the same platform;
/// - a folder naming a *different* platform is a real conflict, and the shared
///   result reports Ambiguous rather than picking a side. The caller's own
///   precedence still decides the overall verdict; this field describes what
///   the *structure* can honestly claim in that situation.
pub(crate) fn confidence_for(
    format: DiskFormat,
    context: DiskFormatContext<'_>,
) -> (DetectionConfidence, bool) {
    let platform = format.platform();
    match context.folder_platform {
        Some(folder) if folder == platform => (DetectionConfidence::Confirmed, true),
        Some(_) => (DetectionConfidence::Ambiguous, false),
        None if format.proves_platform() => (DetectionConfidence::Confirmed, true),
        None => (DetectionConfidence::Probable, false),
    }
}
