//! Bounded structural evidence for classic Macintosh Disk Copy 4.2 images.
//!
//! The fixed 84-byte header and data-then-tag layout are independently
//! described by the DiscFerret/68kMLA specification and implemented by libdsk.
//! Header integers are big-endian. No Macintosh filesystem is traversed.

use super::{
    BoundedReader, Dc42Layout, DiskFormat, DiskFormatContext, DiskFormatEvidence,
    DiskFormatMetadata, DiskFormatRefusal, MAX_DC42_BYTES, MacintoshFilesystem, be_u16, be_u32,
    confidence_for,
};
use std::sync::atomic::AtomicBool;

const HEADER: u64 = 84;
const MAGIC: u16 = 0x0100;

pub(super) fn inspect(
    reader: &mut BoundedReader<'_>,
    context: DiskFormatContext<'_>,
    cancel: Option<&AtomicBool>,
) -> DiskFormatEvidence {
    match validate(reader, cancel) {
        Ok((layout, name, checksum_note)) => {
            let format = DiskFormat::MacintoshDiskCopy42;
            let (confidence, conclusive) = confidence_for(format, context);
            let mut evidence = vec![
                format!("84-byte DC42 header; data payload at offset 0x54 ({} bytes) followed by {} tag bytes", layout.data_size, layout.tag_size),
                format!("DC42 encoding 0x{:02X}, format byte 0x{:02X}; data and tag offsets fit exactly within EOF", layout.encoding, layout.format_byte),
                "Disk Copy 4.2 is Macintosh-family container evidence; the internal disk name is provenance only and never release identity".to_string(),
                checksum_note,
            ];
            if let Some(name) = name {
                evidence.push(format!("Internal disk name (provenance only): {name:?}"));
            }
            evidence.push(match layout.filesystem {
                Some(MacintoshFilesystem::Hfs) => "HFS signature 0x4244 observed at payload-relative offset 1024 (classic HFS MDB location); this does not identify an exact release".to_string(),
                Some(MacintoshFilesystem::Mfs) => "MFS signature 0xD2D7 observed at payload-relative offset 1024 (MFS MDB location); this does not identify an exact release".to_string(),
                None => "No HFS/MFS signature was observed at the safe payload-relative offset".to_string(),
            });
            if let Some(folder) = context.folder_platform
                && folder != format.platform()
            {
                evidence.push(format!("The containing folder names {folder} instead, so the structure and folder disagree"));
            }
            DiskFormatEvidence {
                format: Some(format),
                platform: Some(format.platform()),
                confidence,
                conclusive,
                evidence,
                bytes_inspected: reader.bytes_read(),
                refusal: None,
                metadata: Some(DiskFormatMetadata::Dc42(layout)),
                read_via_symlink: false,
            }
        }
        Err(refusal) => {
            let mut result = DiskFormatEvidence::refused(refusal);
            result.bytes_inspected = reader.bytes_read();
            result
        }
    }
}

fn validate(
    reader: &mut BoundedReader<'_>,
    cancel: Option<&AtomicBool>,
) -> Result<(Dc42Layout, Option<String>, String), DiskFormatRefusal> {
    let length = reader.len();
    if length < HEADER {
        return Err(DiskFormatRefusal::TooSmall {
            length,
            minimum: HEADER,
        });
    }
    if length > MAX_DC42_BYTES {
        return Err(DiskFormatRefusal::TooLarge {
            length,
            maximum: MAX_DC42_BYTES,
        });
    }
    if super::cancelled(cancel) {
        return Err(DiskFormatRefusal::Cancelled);
    }
    let h = reader.read_exact_at_dc42(0, HEADER as usize)?;
    let bad = |detail: &str| DiskFormatRefusal::Malformed {
        detail: detail.to_string(),
    };
    let name_length = h[0];
    if name_length > 63 {
        return Err(bad(
            "DC42 disk-name length exceeds its 63-byte bounded field",
        ));
    }
    if be_u16(&h, 0x52) != Some(MAGIC) {
        return Err(bad("DC42 private word/magic is not 0x0100"));
    }
    let data_size = be_u32(&h, 0x40).unwrap();
    let tag_size = be_u32(&h, 0x44).unwrap();
    let data_checksum = be_u32(&h, 0x48).unwrap();
    let tag_checksum = be_u32(&h, 0x4c).unwrap();
    let encoding = h[0x50];
    let format_byte = h[0x51];
    let expected = match encoding {
        0 => 409600,
        1 => 819200,
        2 => 737280,
        3 => 1474560,
        _ => return Err(bad("DC42 encoding is unsupported (known values are 0..=3)")),
    };
    if data_size != expected {
        return Err(bad(
            "DC42 data size does not match the selected documented disk encoding",
        ));
    }
    if !match encoding {
        0 | 1 => matches!(format_byte, 0x02 | 0x12 | 0x22 | 0x24 | 0x96),
        2 | 3 => format_byte == 0x22,
        _ => false,
    } {
        return Err(bad(
            "DC42 format byte is not valid for the selected encoding",
        ));
    }
    let expected_tags = (data_size as u64 / 512) * 12;
    if tag_size != 0 && tag_size as u64 != expected_tags {
        return Err(bad(
            "DC42 tag length is not zero or 12 bytes per 512-byte sector",
        ));
    }
    let data_end = HEADER
        .checked_add(data_size as u64)
        .ok_or_else(|| bad("DC42 data offset arithmetic overflowed"))?;
    let tag_end = data_end
        .checked_add(tag_size as u64)
        .ok_or_else(|| bad("DC42 tag offset arithmetic overflowed"))?;
    if tag_end != length {
        return Err(DiskFormatRefusal::GeometryMismatch {
            declared_bytes: tag_end,
            actual_bytes: length,
        });
    }
    let name = (name_length > 0)
        .then(|| String::from_utf8_lossy(&h[1..1 + name_length as usize]).into_owned());
    let filesystem = if data_size >= 1026 {
        let sig = reader.read_exact_at_dc42(HEADER + 1024, 2)?;
        match be_u16(&sig, 0) {
            Some(0x4244) => Some(MacintoshFilesystem::Hfs),
            Some(0xD2D7) => Some(MacintoshFilesystem::Mfs),
            _ => None,
        }
    } else {
        None
    };
    let data = read_region(reader, HEADER, data_size as u64, cancel)?;
    let tags = read_region(reader, data_end, tag_size as u64, cancel)?;
    let actual_data = checksum(&data, 0);
    let actual_tags = checksum(&tags, if tags.is_empty() { 0 } else { 12 });
    if actual_data != data_checksum {
        return Err(bad("DC42 data checksum mismatch"));
    }
    if actual_tags != tag_checksum {
        return Err(bad("DC42 tag checksum mismatch"));
    }
    Ok((
        Dc42Layout {
            name_length,
            data_size,
            tag_size,
            data_checksum,
            tag_checksum,
            encoding,
            format_byte,
            payload_offset: HEADER,
            filesystem,
            checksums_verified: true,
        },
        name,
        "DC42 data and tag checksums verified with the documented rotate-right-1 algorithm"
            .to_string(),
    ))
}

fn read_region(
    reader: &mut BoundedReader<'_>,
    offset: u64,
    length: u64,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<u8>, DiskFormatRefusal> {
    let mut result = Vec::with_capacity(length as usize);
    let end = offset
        .checked_add(length)
        .ok_or_else(|| DiskFormatRefusal::Malformed {
            detail: "DC42 checksum range overflowed".to_string(),
        })?;
    let mut at = offset;
    while at < end {
        if super::cancelled(cancel) {
            return Err(DiskFormatRefusal::Cancelled);
        }
        let count = (end - at).min(super::MAX_DISK_FORMAT_READ_CHUNK as u64) as usize;
        result.extend(reader.read_exact_at_dc42(at, count)?);
        at += count as u64;
    }
    Ok(result)
}

fn checksum(bytes: &[u8], skip: usize) -> u32 {
    let mut sum = 0u32;
    let mut at = skip.min(bytes.len());
    while at < bytes.len() {
        let word = ((bytes[at] as u32) << 8) | bytes.get(at + 1).copied().unwrap_or(0) as u32;
        sum = sum.wrapping_add(word).rotate_right(1);
        at += 2;
    }
    sum
}
