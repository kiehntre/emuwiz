//! Raw TR-DOS disk image (`.trd`).
//!
//! # What the format is
//!
//! A `.trd` file is a headerless, in-order dump of every 256-byte sector of
//! a ZX Spectrum Beta Disk (TR-DOS) floppy. There is no container magic, so
//! recognition rests on TR-DOS's own on-disk bookkeeping:
//!
//! * **Track 0, sectors 0-7** hold the catalogue: up to 128 sixteen-byte
//!   directory entries.
//! * **Track 0, sector 8** (the ninth sector, file offset `0x800`) is the
//!   *system / volume descriptor*. The fields this reads, at their offsets
//!   inside that 256-byte sector:
//!
//! | +offset | Field |
//! |---------|-------|
//! | 0x00    | zero - the catalogue "no more entries" marker |
//! | 0xE1    | first free sector (0..=15) |
//! | 0xE2    | first free track (linear: track-per-side x sides) |
//! | 0xE3    | disk type: `0x16` 80T DS, `0x17` 40T DS, `0x18` 80T SS, `0x19` 40T SS |
//! | 0xE4    | number of files (0..=128) |
//! | 0xE5    | free sectors, little-endian u16 |
//! | 0xE7    | the TR-DOS identifier byte, always `0x10` |
//! | 0xF5    | 8-byte disk label |
//!
//! Verified against the widely mirrored "TR-DOS disk structure" reference
//! (ZX-Modules / VTRD / hobeta documentation), the same layout every
//! emulator's TR-DOS reader uses.
//!
//! # What a valid one proves, and the SAM / MGT guard
//!
//! The `0x10` identifier plus a documented disk-type byte plus geometry
//! that fits the file is written only by the ZX Spectrum-family TR-DOS
//! ecosystem (Beta Disk, Pentagon, Scorpion). No other platform produces
//! this descriptor, so [`DiskFormat::proves_platform`] is `true` for
//! `SpectrumTrDosDisk` - but it carries **no machine subtype**, so callers
//! get `platform = "ZX Spectrum"` and nothing finer.
//!
//! Recognition is deliberately narrow: a SAM Coupe / `+D` / DISCiPLE MGT
//! image uses 512-byte sectors, a different track-0 layout, and has no
//! `0x10` byte at `0x8E7`, so it fails this check rather than being
//! mislabelled. A random file that merely happens to be a valid TR-DOS
//! size is rejected by the descriptor gates.
//!
//! # What is read
//!
//! The catalogue's first sector (offset 0) and the system descriptor sector
//! (offset `0x800`) - two 256-byte reads. No file data is read; free-space
//! and geometry are checked arithmetically against the descriptor and the
//! file length.

use std::sync::atomic::AtomicBool;

use super::{
    BoundedReader, DiskFormat, DiskFormatContext, DiskFormatEvidence, DiskFormatMetadata,
    DiskFormatRefusal, MAX_TRD_BYTES, TRDOS_MAX_FILES, TRDOS_SECTOR_BYTES, TrDosDescriptor,
    confidence_for, le_u16,
};

const SECTORS_PER_TRACK: u64 = 16;
/// The descriptor lives in the ninth sector of track 0.
const DESCRIPTOR_OFFSET: u64 = 8 * TRDOS_SECTOR_BYTES; // 0x800
const DESCRIPTOR_BYTES: usize = 256;

const OFF_END_MARKER: usize = 0x00;
const OFF_FIRST_FREE_SECTOR: usize = 0xE1;
const OFF_FIRST_FREE_TRACK: usize = 0xE2;
const OFF_DISK_TYPE: usize = 0xE3;
const OFF_FILE_COUNT: usize = 0xE4;
const OFF_FREE_SECTORS: usize = 0xE5;
const OFF_TRDOS_ID: usize = 0xE7;
const OFF_LABEL: usize = 0xF5;
const LABEL_BYTES: usize = 8;

const TRDOS_ID: u8 = 0x10;

pub(super) fn inspect(
    reader: &mut BoundedReader<'_>,
    context: DiskFormatContext<'_>,
    cancel: Option<&AtomicBool>,
) -> DiskFormatEvidence {
    match validate(reader, cancel) {
        Ok(descriptor) => {
            let format = DiskFormat::SpectrumTrDosDisk;
            let (confidence, conclusive) = confidence_for(format, context);
            let mut evidence = vec![
                format!(
                    "TR-DOS system descriptor in track 0 sector 9: identifier byte 0x10, disk \
                     type 0x{:02X} ({} track(s)/side x {} side(s))",
                    descriptor.disk_type, descriptor.tracks_per_side, descriptor.sides
                ),
                format!(
                    "Catalogue: {} file(s); free space starts at track {} sector {} with {} \
                     free sector(s)",
                    descriptor.file_count,
                    descriptor.first_free_track,
                    descriptor.first_free_sector,
                    descriptor.free_sectors
                ),
                "The TR-DOS descriptor is specific to the ZX Spectrum Beta Disk / Pentagon / \
                 Scorpion family; no other platform writes it. It encodes no machine subtype."
                    .to_string(),
            ];
            if let Some(label) = descriptor.label {
                if let Ok(text) = std::str::from_utf8(&label) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        evidence.push(format!("Disk label: {trimmed:?}"));
                    }
                }
            }
            if let Some(folder) = context.folder_platform
                && folder != format.platform()
            {
                evidence.push(format!(
                    "The containing folder names {folder} instead, so the structure and the \
                     folder disagree"
                ));
            }
            DiskFormatEvidence {
                format: Some(format),
                platform: Some(format.platform()),
                confidence,
                conclusive,
                evidence,
                bytes_inspected: reader.bytes_read(),
                refusal: None,
                metadata: Some(DiskFormatMetadata::TrDos(descriptor)),
                read_via_symlink: false,
            }
        }
        Err(refusal) => {
            let mut refused = DiskFormatEvidence::refused(refusal);
            refused.bytes_inspected = reader.bytes_read();
            refused
        }
    }
}

fn geometry_for_disk_type(disk_type: u8) -> Option<(u16, u8)> {
    match disk_type {
        0x16 => Some((80, 2)),
        0x17 => Some((40, 2)),
        0x18 => Some((80, 1)),
        0x19 => Some((40, 1)),
        _ => None,
    }
}

fn validate(
    reader: &mut BoundedReader<'_>,
    cancel: Option<&AtomicBool>,
) -> Result<TrDosDescriptor, DiskFormatRefusal> {
    let length = reader.len();
    let minimum = DESCRIPTOR_OFFSET + TRDOS_SECTOR_BYTES; // 0x900
    if length < minimum {
        return Err(DiskFormatRefusal::TooSmall { length, minimum });
    }
    if length > MAX_TRD_BYTES {
        return Err(DiskFormatRefusal::TooLarge {
            length,
            maximum: MAX_TRD_BYTES,
        });
    }
    if !length.is_multiple_of(TRDOS_SECTOR_BYTES) {
        return Err(DiskFormatRefusal::NotSectorAligned {
            length,
            sector_bytes: TRDOS_SECTOR_BYTES as u32,
        });
    }
    if super::cancelled(cancel) {
        return Err(DiskFormatRefusal::Cancelled);
    }

    let catalogue = reader.read_exact_at(0, DESCRIPTOR_BYTES)?;
    let descriptor = reader.read_exact_at(DESCRIPTOR_OFFSET, DESCRIPTOR_BYTES)?;
    let malformed = |detail: String| DiskFormatRefusal::Malformed { detail };

    // The one hard signature: the TR-DOS identifier byte.
    let id = descriptor[OFF_TRDOS_ID];
    if id != TRDOS_ID {
        return Err(malformed(format!(
            "the TR-DOS identifier byte (system sector +0x{OFF_TRDOS_ID:02X}) is 0x{id:02X}, \
             not 0x{TRDOS_ID:02X}"
        )));
    }
    if descriptor[OFF_END_MARKER] != 0 {
        return Err(malformed(
            "the catalogue end marker at the start of the system sector is not zero".to_string(),
        ));
    }

    let disk_type = descriptor[OFF_DISK_TYPE];
    let (tracks_per_side, sides) = geometry_for_disk_type(disk_type).ok_or_else(|| {
        malformed(format!(
            "disk-type byte 0x{disk_type:02X} is not one of the documented TR-DOS values \
             (0x16..=0x19)"
        ))
    })?;
    let total_tracks = u64::from(tracks_per_side) * u64::from(sides);
    let total_sectors = total_tracks * SECTORS_PER_TRACK;
    let expected_bytes = total_sectors * TRDOS_SECTOR_BYTES;
    if length > expected_bytes {
        return Err(DiskFormatRefusal::GeometryMismatch {
            declared_bytes: expected_bytes,
            actual_bytes: length,
        });
    }

    let file_count = descriptor[OFF_FILE_COUNT];
    if file_count > TRDOS_MAX_FILES {
        return Err(malformed(format!(
            "the descriptor claims {file_count} files, more than the 128-entry catalogue holds"
        )));
    }
    if file_count > 0 && catalogue[0] == 0 {
        return Err(malformed(format!(
            "the descriptor claims {file_count} files but the first catalogue slot is empty"
        )));
    }

    let first_free_sector = descriptor[OFF_FIRST_FREE_SECTOR];
    if u64::from(first_free_sector) >= SECTORS_PER_TRACK {
        return Err(malformed(format!(
            "first-free-sector {first_free_sector} is outside 0..=15"
        )));
    }
    let first_free_track = u16::from(descriptor[OFF_FIRST_FREE_TRACK]);
    if u64::from(first_free_track) > total_tracks {
        return Err(malformed(format!(
            "first-free-track {first_free_track} is past the disk's {total_tracks} track(s)"
        )));
    }
    let free_sectors = le_u16(&descriptor, OFF_FREE_SECTORS)
        .ok_or_else(|| malformed("no free-sector-count word in the descriptor".to_string()))?;
    // Track 0 is entirely reserved for the catalogue and descriptor, so the
    // most sectors that can be free is the disk minus that track.
    let max_free = total_sectors.saturating_sub(SECTORS_PER_TRACK);
    if u64::from(free_sectors) > max_free {
        return Err(malformed(format!(
            "the descriptor reports {free_sectors} free sectors, more than the {max_free} \
             data sectors this geometry has"
        )));
    }
    // Free space on a TR-DOS disk is kept contiguous: the free cursor plus the
    // free count must land exactly on the end of the disk. A mismatch is a
    // corrupt or hand-edited descriptor.
    let free_cursor =
        u64::from(first_free_track) * SECTORS_PER_TRACK + u64::from(first_free_sector);
    if free_cursor + u64::from(free_sectors) != total_sectors {
        return Err(malformed(format!(
            "the free-space cursor (track {first_free_track}, sector {first_free_sector}) plus \
             {free_sectors} free sectors does not reach the disk's {total_sectors} sectors"
        )));
    }

    let mut label_bytes = [0u8; LABEL_BYTES];
    label_bytes.copy_from_slice(&descriptor[OFF_LABEL..OFF_LABEL + LABEL_BYTES]);
    // A label only when it is printable ASCII *and* not entirely blank - an
    // all-spaces (or all-zero) field is TR-DOS's "no label set" state.
    let label = (label_bytes.iter().all(|byte| (0x20..=0x7E).contains(byte))
        && label_bytes.iter().any(|byte| *byte != 0x20))
    .then_some(label_bytes);

    Ok(TrDosDescriptor {
        disk_type,
        tracks_per_side,
        sides,
        file_count,
        first_free_sector,
        first_free_track,
        free_sectors,
        label,
    })
}
