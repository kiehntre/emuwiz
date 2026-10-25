//! Bounded structural inspection of the common raw Famicom Disk System image.
//!
//! A `.fds` file may have a 16-byte `FDS\x1a` wrapper header, followed by one
//! or more fixed 65,500-byte sides. Each side contains the ordered disk-info,
//! file-count, and file-header/file-data blocks, with zero padding to the side
//! boundary. This observer validates those relationships only; it never reads
//! file payloads and never treats the embedded three-byte game code as a game
//! identity.

use std::sync::atomic::AtomicBool;

use super::{
    BoundedReader, DiskFormat, DiskFormatContext, DiskFormatEvidence, DiskFormatMetadata,
    DiskFormatRefusal, FDS_SIDE_BYTES, MAX_FDS_BYTES, MAX_FDS_FILES_PER_SIDE, confidence_for,
    le_u16,
};

const FDS_MAGIC: &[u8; 4] = b"FDS\x1a";
const FDS_HEADER_BYTES: u64 = 16;
const DISK_INFO_BYTES: u64 = 56;
const FILE_COUNT_BYTES: u64 = 2;
const FILE_HEADER_BYTES: u64 = 17;
const MAX_SIDES: u8 = (MAX_FDS_BYTES / FDS_SIDE_BYTES) as u8;

pub(super) fn inspect(
    reader: &mut BoundedReader<'_>,
    context: DiskFormatContext<'_>,
    cancel: Option<&AtomicBool>,
) -> DiskFormatEvidence {
    match validate(reader, cancel) {
        Ok(layout) => {
            let format = DiskFormat::FamicomDiskSystem;
            let (confidence, conclusive) = confidence_for(format, context);
            DiskFormatEvidence {
                format: Some(format),
                platform: Some(format.platform()),
                confidence,
                conclusive,
                evidence: vec![
                    format!(
                        "Raw FDS structure: {} side(s), {} file(s) declared on the first side",
                        layout.sides, layout.files_per_side
                    ),
                    format!(
                        "Each side is exactly {FDS_SIDE_BYTES} bytes and begins with the +                         `*NINTENDO-HVC*` disk marker"
                    ),
                    "Ordered disk-info, file-count, and file-header/data boundaries +                     validated; file payload bytes were not read"
                        .to_string(),
                    "The structure proves Famicom Disk System media, not a particular game +                     release; exact identity still requires DAT/hash evidence"
                        .to_string(),
                ],
                bytes_inspected: reader.bytes_read(),
                refusal: None,
                metadata: Some(DiskFormatMetadata::Fds(layout)),
                read_via_symlink: false,
            }
        }
        Err(refusal) => {
            let mut evidence = DiskFormatEvidence::refused(refusal);
            evidence.bytes_inspected = reader.bytes_read();
            evidence
        }
    }
}

fn validate(
    reader: &mut BoundedReader<'_>,
    cancel: Option<&AtomicBool>,
) -> Result<super::FdsLayout, DiskFormatRefusal> {
    let length = reader.len();
    if length > MAX_FDS_BYTES {
        return Err(DiskFormatRefusal::TooLarge {
            length,
            maximum: MAX_FDS_BYTES,
        });
    }

    let (header_bytes, sides) =
        if length >= FDS_HEADER_BYTES && reader.read_exact_at_fds(0, 4)?.as_slice() == FDS_MAGIC {
            let header = reader.read_exact_at_fds(0, FDS_HEADER_BYTES as usize)?;
            if header[4] == 0 || header[4] > MAX_SIDES {
                return Err(malformed(format!(
                    "FDS wrapper declares {} side(s), outside 1..={MAX_SIDES}",
                    header[4]
                )));
            }
            if header[5..].iter().any(|byte| *byte != 0) {
                return Err(malformed(
                    "the 16-byte FDS wrapper has non-zero reserved bytes".to_string(),
                ));
            }
            let expected = FDS_HEADER_BYTES
                .checked_add(
                    u64::from(header[4])
                        .checked_mul(FDS_SIDE_BYTES)
                        .ok_or_else(|| {
                            malformed("FDS side count multiplication overflowed".to_string())
                        })?,
                )
                .ok_or_else(|| malformed("FDS image length calculation overflowed".to_string()))?;
            if length != expected {
                return Err(DiskFormatRefusal::GeometryMismatch {
                    declared_bytes: expected,
                    actual_bytes: length,
                });
            }
            (16, header[4])
        } else {
            if length == 0 || !length.is_multiple_of(FDS_SIDE_BYTES) {
                return Err(malformed(format!(
                    "headerless FDS data must contain whole {FDS_SIDE_BYTES}-byte side(s)"
                )));
            }
            let sides = length / FDS_SIDE_BYTES;
            if sides == 0 || sides > u64::from(MAX_SIDES) {
                return Err(malformed(format!(
                    "headerless FDS image has {sides} side(s), outside 1..={MAX_SIDES}"
                )));
            }
            (0, sides as u8)
        };

    let mut first_side_files = 0;
    for side in 0..u64::from(sides) {
        if super::cancelled(cancel) {
            return Err(DiskFormatRefusal::Cancelled);
        }
        let side_start = u64::from(header_bytes)
            .checked_add(side.checked_mul(FDS_SIDE_BYTES).ok_or_else(|| {
                malformed("FDS side offset multiplication overflowed".to_string())
            })?)
            .ok_or_else(|| malformed("FDS side offset overflowed".to_string()))?;
        let files = validate_side(reader, side_start, cancel)?;
        if side == 0 {
            first_side_files = files;
        }
    }

    Ok(super::FdsLayout {
        header_bytes,
        sides,
        files_per_side: first_side_files,
    })
}

fn validate_side(
    reader: &mut BoundedReader<'_>,
    side_start: u64,
    cancel: Option<&AtomicBool>,
) -> Result<u8, DiskFormatRefusal> {
    let info = reader.read_exact_at_fds(side_start, DISK_INFO_BYTES as usize)?;
    if info[0] != 0x01 || &info[1..15] != b"*NINTENDO-HVC*" {
        return Err(malformed(
            "side does not begin with an FDS disk-info block and *NINTENDO-HVC* marker".to_string(),
        ));
    }
    if info[0x13] != b' ' && !matches!(info[0x13], b'E' | b'J' | b'R') {
        return Err(malformed(format!(
            "unsupported FDS game-type byte 0x{:02x}",
            info[0x13]
        )));
    }
    if info[0x18] != 0 {
        return Err(malformed(
            "FDS disk-info reserved byte at 0x18 is not zero".to_string(),
        ));
    }
    if info[0x19] > 0x7f || info[0x35] > 1 || info[0x36] != 0 && !matches!(info[0x36], 0xfe | 0xff)
    {
        return Err(malformed(
            "FDS disk-info side/type metadata is invalid".to_string(),
        ));
    }

    let count_offset = side_start
        .checked_add(DISK_INFO_BYTES)
        .ok_or_else(|| malformed("FDS file-count offset overflowed".to_string()))?;
    let count_block = reader.read_exact_at_fds(count_offset, FILE_COUNT_BYTES as usize)?;
    if count_block[0] != 0x02 || count_block[1] > MAX_FDS_FILES_PER_SIDE {
        return Err(malformed(format!(
            "FDS file-count block is invalid or declares too many files ({})",
            count_block[1]
        )));
    }

    let mut cursor = count_offset
        .checked_add(FILE_COUNT_BYTES)
        .ok_or_else(|| malformed("FDS first file offset overflowed".to_string()))?;
    let side_end = side_start
        .checked_add(FDS_SIDE_BYTES)
        .ok_or_else(|| malformed("FDS side end offset overflowed".to_string()))?;
    for file_number in 0..count_block[1] {
        if super::cancelled(cancel) {
            return Err(DiskFormatRefusal::Cancelled);
        }
        let header_end = cursor
            .checked_add(FILE_HEADER_BYTES)
            .ok_or_else(|| malformed("FDS file-header offset overflowed".to_string()))?;
        if header_end > side_end {
            return Err(DiskFormatRefusal::Truncated {
                offset: cursor,
                wanted: FILE_HEADER_BYTES as usize,
            });
        }
        let file = reader.read_exact_at_fds(cursor, FILE_HEADER_BYTES as usize)?;
        if file[0] != 0x03 || file[1] != file_number || file[0x0f] > 2 {
            return Err(malformed(format!(
                "FDS file header {file_number} has invalid block, number, or file type"
            )));
        }
        if file[3..11]
            .iter()
            .any(|byte| !byte.is_ascii_graphic() && *byte != b' ')
        {
            return Err(malformed(format!(
                "FDS file header {file_number} has malformed non-ASCII file-name metadata"
            )));
        }
        let data_start = header_end;
        let data_end = data_start
            .checked_add(u64::from(le_u16(&file, 0x0d).unwrap_or_default()))
            .ok_or_else(|| malformed("FDS file-data length overflowed".to_string()))?;
        if data_end > side_end {
            return Err(DiskFormatRefusal::Truncated {
                offset: data_start,
                wanted: usize::try_from(data_end - data_start).unwrap_or(usize::MAX),
            });
        }
        cursor = data_end;
    }

    Ok(count_block[1])
}

fn malformed(detail: String) -> DiskFormatRefusal {
    DiskFormatRefusal::Malformed { detail }
}
