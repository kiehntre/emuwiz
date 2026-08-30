//! Bounded structural evidence for Japanese D88 floppy containers.
//!
//! D88 is a container format, not a machine identifier. The same structure is
//! used by PC-88, PC-98, FM Towns and X68000 software, so this adapter proves
//! only that the header, track table and bounded sector records agree.
//!
//! The fixed header layout is 0x2b0 bytes: a 17-byte disk name, write-protect
//! and media-type bytes, then 164 little-endian absolute track offsets. Each
//! present track contains 16-byte sector headers followed by the sector data.

use std::sync::atomic::AtomicBool;

use super::{
    BoundedReader, D88_HEADER_BYTES, D88Layout, DiskFormat, DiskFormatContext, DiskFormatEvidence,
    DiskFormatMetadata, DiskFormatRefusal, MAX_D88_BYTES, MAX_D88_TRACK_ENTRIES, confidence_for,
    le_u32,
};

const DISK_NAME_OFFSET: usize = 0;
const DISK_NAME_BYTES: usize = 17;
const WRITE_PROTECT_OFFSET: usize = 0x1a;
const MEDIA_TYPE_OFFSET: usize = 0x1b;
const TRACK_TABLE_OFFSET: usize = 0x1c;
const SECTOR_HEADER_BYTES: usize = 16;
const DELETED_FLAG_OFFSET: usize = 7;
const STATUS_OFFSET: usize = 8;
const SECTOR_SIZE_CODE_OFFSET: usize = 3;
const SECTOR_DATA_LENGTH_OFFSET: usize = 14;
const TRACKS_TO_WALK: usize = 4;
const MAX_SECTORS_PER_TRACK: u16 = 64;

pub(super) fn inspect(
    reader: &mut BoundedReader<'_>,
    context: DiskFormatContext<'_>,
    cancel: Option<&AtomicBool>,
) -> DiskFormatEvidence {
    match validate(reader, cancel) {
        Ok((layout, name)) => {
            let format = DiskFormat::D88Container;
            let (confidence, conclusive) = confidence_for(format, context);
            let mut evidence = vec![
                format!(
                    "D88 header and track table are structurally valid: {} present track(s), {} sector(s), {} data bytes",
                    layout.validated_track_entries,
                    layout.declared_sectors,
                    layout.declared_data_bytes
                ),
                "D88 is a shared Japanese-computer disk container; its structure does not identify PC-88, PC-98, FM Towns or X68000".to_string(),
                format!(
                    "D88 write-protect flag: {}; media/type byte: 0x{:02x}",
                    if layout.write_protected { "set" } else { "clear" },
                    layout.media_type
                ),
            ];
            if let Some(name) = name {
                evidence.push(format!("D88 disk name: {name}"));
            }
            evidence.push(
                "Sector headers validated for C/H/R/N, sector count, deleted-data flag, status/CRC byte, and declared data length".to_string(),
            );
            DiskFormatEvidence {
                format: Some(format),
                platform: Some(format.platform()),
                confidence,
                conclusive,
                evidence,
                bytes_inspected: reader.bytes_read(),
                refusal: None,
                metadata: Some(DiskFormatMetadata::D88(layout)),
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
) -> Result<(D88Layout, Option<String>), DiskFormatRefusal> {
    let length = reader.len();
    if length < D88_HEADER_BYTES as u64 {
        return Err(DiskFormatRefusal::TooSmall {
            length,
            minimum: D88_HEADER_BYTES as u64,
        });
    }
    if length > MAX_D88_BYTES {
        return Err(DiskFormatRefusal::TooLarge {
            length,
            maximum: MAX_D88_BYTES,
        });
    }
    if super::cancelled(cancel) {
        return Err(DiskFormatRefusal::Cancelled);
    }

    let header = reader.read_exact_at(0, D88_HEADER_BYTES)?;
    let name = disk_name(&header[DISK_NAME_OFFSET..DISK_NAME_OFFSET + DISK_NAME_BYTES]);
    let write_protected = header[WRITE_PROTECT_OFFSET] != 0;
    let media_type = header[MEDIA_TYPE_OFFSET];

    let mut offsets = Vec::with_capacity(MAX_D88_TRACK_ENTRIES);
    let mut previous = None;
    let mut present = 0usize;
    for index in 0..MAX_D88_TRACK_ENTRIES {
        let offset = le_u32(&header, TRACK_TABLE_OFFSET + index * 4)
            .ok_or_else(|| malformed(format!("track offset {index} is truncated")))?
            as u64;
        if offset == 0 {
            offsets.push(None);
            continue;
        }
        if offset < D88_HEADER_BYTES as u64 || offset >= length {
            return Err(malformed(format!(
                "track offset {offset} at table entry {index} is outside the file"
            )));
        }
        if previous.is_some_and(|previous| offset <= previous) {
            return Err(malformed(format!(
                "track offsets are descending or overlapping at table entry {index}"
            )));
        }
        previous = Some(offset);
        present += 1;
        offsets.push(Some(offset));
    }
    if present == 0 {
        return Err(malformed(
            "the D88 track table contains no present tracks".to_string(),
        ));
    }

    let mut validated = 0usize;
    let mut sectors = 0u32;
    let mut data_bytes = 0u64;
    for index in 0..MAX_D88_TRACK_ENTRIES {
        let Some(start) = offsets[index] else {
            continue;
        };
        let end = offsets[index + 1..]
            .iter()
            .flatten()
            .next()
            .copied()
            .unwrap_or(length);
        if end <= start {
            return Err(malformed(format!(
                "track {index} has an empty or reversed extent"
            )));
        }
        if validated >= TRACKS_TO_WALK || start > super::MAX_DISK_FORMAT_OFFSET {
            continue;
        }
        let (track_sectors, track_bytes) = validate_track(reader, start, end, index, cancel)?;
        sectors = sectors
            .checked_add(u32::from(track_sectors))
            .ok_or_else(|| malformed("the sector count overflows".to_string()))?;
        data_bytes = data_bytes
            .checked_add(track_bytes)
            .ok_or_else(|| malformed("the sector data length overflows".to_string()))?;
        validated += 1;
    }
    if validated == 0 {
        return Err(malformed(
            "no present track was reachable within the bounded inspection limit".to_string(),
        ));
    }

    Ok((
        D88Layout {
            disk_name: header[DISK_NAME_OFFSET..DISK_NAME_OFFSET + DISK_NAME_BYTES]
                .try_into()
                .expect("fixed D88 disk-name width"),
            write_protected,
            media_type,
            declared_track_entries: present,
            validated_track_entries: validated,
            declared_sectors: sectors,
            declared_data_bytes: data_bytes,
        },
        name,
    ))
}

fn validate_track(
    reader: &mut BoundedReader<'_>,
    start: u64,
    end: u64,
    index: usize,
    cancel: Option<&AtomicBool>,
) -> Result<(u16, u64), DiskFormatRefusal> {
    let mut cursor = start;
    let mut expected_count = None;
    let mut data_bytes = 0u64;
    let mut count = 0u16;
    while cursor < end {
        if super::cancelled(cancel) {
            return Err(DiskFormatRefusal::Cancelled);
        }
        let header_end = cursor
            .checked_add(SECTOR_HEADER_BYTES as u64)
            .ok_or_else(|| malformed("sector header offset overflowed".to_string()))?;
        if header_end > end {
            return Err(malformed(format!(
                "track {index} ends with a truncated sector header"
            )));
        }
        if cursor > super::MAX_DISK_FORMAT_OFFSET {
            return Err(malformed(format!(
                "sector header in track {index} is past the inspection limit"
            )));
        }
        let sector = reader.read_exact_at(cursor, SECTOR_HEADER_BYTES)?;
        let sector_count = u16::from_le_bytes([sector[4], sector[5]]);
        if !(1..=MAX_SECTORS_PER_TRACK).contains(&sector_count) {
            return Err(malformed(format!(
                "track {index} declares impossible sector count {sector_count}"
            )));
        }
        if expected_count
            .replace(sector_count)
            .is_some_and(|expected| expected != sector_count)
        {
            return Err(malformed(format!(
                "track {index} has inconsistent sectors-per-track metadata"
            )));
        }
        let cylinder = sector[0];
        let head = sector[1];
        let record = sector[2];
        let size_code = sector[SECTOR_SIZE_CODE_OFFSET];
        if cylinder > 81 || head > 1 || record == 0 || size_code > 6 {
            return Err(malformed(format!(
                "track {index} has invalid C/H/R/N sector fields"
            )));
        }
        let expected_bytes = 128u64
            .checked_shl(u32::from(size_code))
            .ok_or_else(|| malformed("sector size code overflowed".to_string()))?;
        let declared_bytes = u64::from(u16::from_le_bytes([
            sector[SECTOR_DATA_LENGTH_OFFSET],
            sector[SECTOR_DATA_LENGTH_OFFSET + 1],
        ]));
        if declared_bytes != expected_bytes {
            return Err(malformed(format!(
                "track {index} sector {record} declares {declared_bytes} data bytes for N={size_code}, expected {expected_bytes}"
            )));
        }
        let data_end = header_end
            .checked_add(declared_bytes)
            .ok_or_else(|| malformed("sector data extent overflowed".to_string()))?;
        if data_end > end {
            return Err(malformed(format!(
                "track {index} sector {record} data extends past the track"
            )));
        }
        // These fields are intentionally observed rather than interpreted as
        // platform evidence: deleted-data and CRC/status values are disk-level
        // facts and may legally vary between preservation dumps.
        let _deleted = sector[DELETED_FLAG_OFFSET];
        let _status = sector[STATUS_OFFSET];
        cursor = data_end;
        count += 1;
        data_bytes = data_bytes
            .checked_add(declared_bytes)
            .ok_or_else(|| malformed("track data length overflowed".to_string()))?;
        if count > sector_count {
            return Err(malformed(format!(
                "track {index} contains more sectors than declared"
            )));
        }
    }
    let expected_count =
        expected_count.ok_or_else(|| malformed(format!("track {index} has no sector records")))?;
    if count != expected_count || cursor != end {
        return Err(malformed(format!(
            "track {index} sector count does not account for its full extent"
        )));
    }
    Ok((count, data_bytes))
}

fn disk_name(bytes: &[u8]) -> Option<String> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let value = bytes[..end]
        .iter()
        .copied()
        .map(char::from)
        .collect::<String>();
    let value = value.trim().to_string();
    (!value.is_empty() && value.chars().all(|character| !character.is_control())).then_some(value)
}

fn malformed(detail: String) -> DiskFormatRefusal {
    DiskFormatRefusal::Malformed { detail }
}
