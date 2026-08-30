//! CPCEMU `.dsk` disk image container (standard and extended).
//!
//! # What the format is
//!
//! A `.dsk` file is a 256-byte *disk-information block* followed by one
//! *track-information block* (also 256 bytes) per physical track, each
//! followed by that track's raw sector data. Two container variants exist:
//!
//! * **standard** - identifier `MV - CPCEMU Disk-File\r\nDisk-Info\r\n`;
//!   every track is the same size, given once at offset 0x32 as a
//!   little-endian word that *includes* the 256-byte track header.
//! * **extended** - identifier `EXTENDED CPC DSK File\r\nDisk-Info\r\n`; a
//!   per-`track x side` size table starts at offset 0x34, one byte each,
//!   holding the high byte of that track's total size (so `value * 256`);
//!   a `0` means the track is not present.
//!
//! Verified against the CPCWiki "Format:DSK disk image file format" page,
//! the reference every CPC/+3 emulator's `.dsk` loader is written against.
//!
//! # What a valid one proves, and what it does not
//!
//! It proves the file is a coherent CPCEMU container whose declared track
//! table accounts for its length. It does **not** prove a platform: this
//! exact container is used by the Amstrad CPC, the ZX Spectrum +3 and the
//! Amstrad PCW. So a bare match is [`DiskFormat::CpcEmuDsk`], which
//! [`DiskFormat::proves_platform`] reports `false` for.
//!
//! # The one platform-specific signal read here
//!
//! Track 0's first sector is read (512 bytes) and checked for a `+3DOS` /
//! PCW *disk-specification block*: a 16-byte structure (disk type, sidedness,
//! tracks, sectors-per-track, `log2(sector size) - 7`, reserved fields and a
//! checksum) that a Spectrum +3 / PCW disk carries in that sector and an
//! Amstrad CPC AMSDOS disk does not. It is accepted only when every field is
//! internally consistent *and* agrees with the container's own track
//! descriptors, and only then (disk type 0 or 3) is
//! [`DiskFormat::SpectrumPlus3Disk`] claimed. If any check fails the result
//! degrades to the bare `CpcEmuDsk` container - never to a false Spectrum
//! claim.
//!
//! Reads: the disk-information block, up to four track-information blocks, and
//! track 0's first sector. No sector body beyond that first one is touched;
//! no filesystem is walked.

use std::sync::atomic::AtomicBool;

use super::{
    BoundedReader, DSK_INFO_BLOCK_BYTES, DiskFormat, DiskFormatContext, DiskFormatEvidence,
    DiskFormatMetadata, DiskFormatRefusal, DskLayout, MAX_DSK_BYTES, MAX_DSK_TRACK_ENTRIES,
    confidence_for, le_u16,
};

const STANDARD_MAGIC: &[u8] = b"MV - CPC";
const EXTENDED_MAGIC: &[u8] = b"EXTENDED CPC DSK";
const TRACK_INFO_MAGIC: &[u8] = b"Track-Info";

const TRACKS_OFFSET: usize = 0x30;
const SIDES_OFFSET: usize = 0x31;
const STANDARD_TRACK_SIZE_OFFSET: usize = 0x32;
const EXTENDED_SIZE_TABLE_OFFSET: usize = 0x34;

/// The most sectors that fit in a 256-byte track-information block:
/// `(256 - 24) / 8`.
const MAX_SECTORS_PER_TRACK: u8 = 29;
/// How many track-information blocks to actually read and cross-check. The
/// rest of the table is validated arithmetically against the file length.
const TRACK_BLOCKS_TO_READ: usize = 4;

const FIRST_SECTOR_OFFSET: u64 = (DSK_INFO_BLOCK_BYTES * 2) as u64; // 0x200
const FIRST_SECTOR_BYTES: usize = 512;

pub(super) fn inspect(
    reader: &mut BoundedReader<'_>,
    context: DiskFormatContext<'_>,
    cancel: Option<&AtomicBool>,
) -> DiskFormatEvidence {
    match validate(reader, cancel) {
        Ok(layout) => {
            let format = if layout.plus3dos_disk_spec
                && matches!(layout.plus3dos_disk_type, Some(0) | Some(3))
            {
                DiskFormat::SpectrumPlus3Disk
            } else {
                DiskFormat::CpcEmuDsk
            };
            let (confidence, conclusive) = confidence_for(format, context);

            let mut evidence = vec![
                format!(
                    "{} disk-information block: {} track(s) x {} side(s)",
                    if layout.extended {
                        "Extended CPC DSK"
                    } else {
                        "Standard CPCEMU"
                    },
                    layout.declared_tracks,
                    layout.declared_sides
                ),
                format!(
                    "Track table accounts for the file's length; {} track header(s) walked and \
                     internally consistent, declaring {} sector(s) between them",
                    layout.validated_tracks, layout.declared_sectors
                ),
            ];
            match format {
                DiskFormat::SpectrumPlus3Disk => {
                    evidence.push(format!(
                        "Track 0, sector 1 carries a valid +3DOS/PCW disk specification \
                         (disk type {}, geometry consistent with the container's own track \
                         descriptors, reserved fields zero)",
                        layout.plus3dos_disk_type.unwrap_or_default()
                    ));
                    if layout.plus3_bootable {
                        evidence.push(
                            "The first sector's whole-sector checksum marks it a bootable \
                             ZX Spectrum +3 disk"
                                .to_string(),
                        );
                    }
                    evidence.push(
                        "This specification is specific to the Spectrum +3 / PCW disk family; \
                         Amstrad CPC AMSDOS disks do not carry it"
                            .to_string(),
                    );
                }
                _ => {
                    let reason = match layout.plus3dos_disk_type {
                        Some(disk_type @ (1 | 2)) => format!(
                            "track 0 carries an AMSDOS/CPC disk specification (disk type {disk_type})"
                        ),
                        _ => "no +3DOS disk specification was found in track 0".to_string(),
                    };
                    evidence.push(format!(
                        "The CPCEMU `.dsk` container is shared by the Amstrad CPC, ZX Spectrum +3 \
                         and Amstrad PCW; {reason}, so no ZX Spectrum claim is made from the \
                         container alone"
                    ));
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
                metadata: Some(DiskFormatMetadata::Dsk(layout)),
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

fn validate(
    reader: &mut BoundedReader<'_>,
    cancel: Option<&AtomicBool>,
) -> Result<DskLayout, DiskFormatRefusal> {
    let length = reader.len();
    let minimum = (DSK_INFO_BLOCK_BYTES * 2) as u64;
    if length < minimum {
        return Err(DiskFormatRefusal::TooSmall { length, minimum });
    }
    if length > MAX_DSK_BYTES {
        return Err(DiskFormatRefusal::TooLarge {
            length,
            maximum: MAX_DSK_BYTES,
        });
    }
    if super::cancelled(cancel) {
        return Err(DiskFormatRefusal::Cancelled);
    }

    let info = reader.read_exact_at(0, DSK_INFO_BLOCK_BYTES)?;
    let malformed = |detail: String| DiskFormatRefusal::Malformed { detail };

    let extended = info.starts_with(EXTENDED_MAGIC);
    if !extended && !info.starts_with(STANDARD_MAGIC) {
        return Err(malformed(
            "the disk-information block has neither the `MV - CPC` nor the `EXTENDED CPC DSK` \
             identifier"
                .to_string(),
        ));
    }

    let declared_tracks = info[TRACKS_OFFSET];
    let declared_sides = info[SIDES_OFFSET];
    if declared_tracks == 0 || declared_tracks > 85 {
        return Err(malformed(format!(
            "{declared_tracks} tracks is outside 1..=85"
        )));
    }
    if declared_sides == 0 || declared_sides > 2 {
        return Err(malformed(format!("{declared_sides} sides is not 1 or 2")));
    }
    let entries = usize::from(declared_tracks)
        .checked_mul(usize::from(declared_sides))
        .ok_or_else(|| malformed("the track table size overflows".to_string()))?;
    if entries > MAX_DSK_TRACK_ENTRIES {
        return Err(malformed(format!(
            "{entries} track x side entries is past the {MAX_DSK_TRACK_ENTRIES}-entry limit"
        )));
    }

    // Per-track sizes, and the offset each track's information block starts at.
    let mut track_offsets: Vec<u64> = Vec::with_capacity(entries);
    let mut cursor = DSK_INFO_BLOCK_BYTES as u64;
    if extended {
        let table_end = EXTENDED_SIZE_TABLE_OFFSET
            .checked_add(entries)
            .filter(|end| *end <= DSK_INFO_BLOCK_BYTES)
            .ok_or_else(|| {
                malformed("the extended track-size table does not fit in the header".to_string())
            })?;
        for byte in &info[EXTENDED_SIZE_TABLE_OFFSET..table_end] {
            let track_size = u64::from(*byte) * 256;
            if track_size == 0 {
                // A track that is simply not present in the image.
                track_offsets.push(cursor);
                continue;
            }
            if track_size < DSK_INFO_BLOCK_BYTES as u64 {
                return Err(malformed(format!(
                    "an extended track size of {track_size} bytes is smaller than its own \
                     256-byte header"
                )));
            }
            track_offsets.push(cursor);
            cursor = cursor
                .checked_add(track_size)
                .ok_or_else(|| malformed("a track offset overflowed".to_string()))?;
        }
    } else {
        let track_size = u64::from(
            le_u16(&info, STANDARD_TRACK_SIZE_OFFSET)
                .ok_or_else(|| malformed("no standard track-size word".to_string()))?,
        );
        if track_size < DSK_INFO_BLOCK_BYTES as u64 || track_size % 256 != 0 {
            return Err(malformed(format!(
                "a standard track size of {track_size} bytes is not a non-zero multiple of 256 \
                 at least as large as its 256-byte header"
            )));
        }
        for _ in 0..entries {
            track_offsets.push(cursor);
            cursor = cursor
                .checked_add(track_size)
                .ok_or_else(|| malformed("a track offset overflowed".to_string()))?;
        }
    }

    // The declared geometry must account for the file. A shorter file is
    // truncated; a much longer one is not the disk its header describes (a
    // little trailing padding is tolerated).
    if cursor > length {
        return Err(DiskFormatRefusal::GeometryMismatch {
            declared_bytes: cursor,
            actual_bytes: length,
        });
    }
    if length - cursor >= DSK_INFO_BLOCK_BYTES as u64 {
        return Err(DiskFormatRefusal::GeometryMismatch {
            declared_bytes: cursor,
            actual_bytes: length,
        });
    }

    // Read and cross-check the first few present track headers.
    let mut validated_tracks = 0usize;
    let mut declared_sectors: u32 = 0;
    let mut track0_sectors: Option<u8> = None;
    for (index, &offset) in track_offsets.iter().enumerate().take(TRACK_BLOCKS_TO_READ) {
        if super::cancelled(cancel) {
            return Err(DiskFormatRefusal::Cancelled);
        }
        let block_end = offset
            .checked_add(DSK_INFO_BLOCK_BYTES as u64)
            .ok_or_else(|| malformed("a track header offset overflowed".to_string()))?;
        if block_end > length || offset > super::MAX_DISK_FORMAT_OFFSET {
            break;
        }
        let block = reader.read_exact_at(offset, DSK_INFO_BLOCK_BYTES)?;
        if !block.starts_with(TRACK_INFO_MAGIC) {
            return Err(malformed(format!(
                "the track header at offset {offset} has no `Track-Info` identifier"
            )));
        }
        let sector_size_code = block[0x14];
        let sector_count = block[0x15];
        if sector_size_code > 6 {
            return Err(malformed(format!(
                "track {index}: sector-size code {sector_size_code} is outside 0..=6"
            )));
        }
        if sector_count > MAX_SECTORS_PER_TRACK {
            return Err(malformed(format!(
                "track {index}: {sector_count} sectors will not fit in a 256-byte track header"
            )));
        }
        if index == 0 {
            track0_sectors = Some(sector_count);
        }
        declared_sectors = declared_sectors.saturating_add(u32::from(sector_count));
        validated_tracks += 1;
    }
    if validated_tracks == 0 {
        return Err(malformed(
            "no track-information block could be read and validated".to_string(),
        ));
    }

    // The one platform-specific probe: track 0's first sector.
    let (plus3dos_disk_spec, plus3dos_disk_type, plus3_bootable) =
        probe_plus3dos_disk_spec(reader, length, declared_tracks, track0_sectors)?;

    Ok(DskLayout {
        extended,
        declared_tracks,
        declared_sides,
        validated_tracks,
        declared_sectors,
        plus3dos_disk_spec,
        plus3dos_disk_type,
        plus3_bootable,
    })
}

/// Reads track 0's first sector and looks for a `+3DOS`/PCW disk
/// specification. Any inconsistency yields `(false, None, false)` rather
/// than an error - a container that is a valid CPCEMU `.dsk` but not a +3
/// disk is a normal, expected outcome, not a malformed file.
fn probe_plus3dos_disk_spec(
    reader: &mut BoundedReader<'_>,
    length: u64,
    declared_tracks: u8,
    track0_sectors: Option<u8>,
) -> Result<(bool, Option<u8>, bool), DiskFormatRefusal> {
    if length < FIRST_SECTOR_OFFSET + FIRST_SECTOR_BYTES as u64 {
        return Ok((false, None, false));
    }
    let sector = reader.read_exact_at(FIRST_SECTOR_OFFSET, FIRST_SECTOR_BYTES)?;

    let disk_type = sector[0];
    let tracks_field = sector[2];
    let sectors_field = sector[3];
    let sector_size_code = sector[4];

    let type_is_known = matches!(disk_type, 0 | 1 | 2 | 3);
    let reserved_zero = sector[10..15].iter().all(|byte| *byte == 0);
    let size_is_512 = sector_size_code == 2;
    let tracks_agree =
        (1..=85).contains(&tracks_field) && tracks_field.abs_diff(declared_tracks) <= 1;
    let sectors_agree = (1..=MAX_SECTORS_PER_TRACK).contains(&sectors_field)
        && track0_sectors.is_some_and(|count| count == sectors_field);

    if !(type_is_known && reserved_zero && size_is_512 && tracks_agree && sectors_agree) {
        return Ok((false, None, false));
    }

    let checksum = sector.iter().fold(0u32, |sum, byte| sum + u32::from(*byte)) % 256;
    Ok((true, Some(disk_type), checksum == 3))
}
