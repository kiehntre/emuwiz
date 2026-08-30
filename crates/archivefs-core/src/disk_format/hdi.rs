//! Bounded structural evidence for Anex86 HDI and T98-Next NHD images.
//!
//! HDI and NHD are Japanese-computer hard-disk containers, but a valid header
//! proves only the container and its geometry.  It does not identify PC-98,
//! DOS, Windows, or any other machine; callers must supply that identity from
//! independent folder, DAT, or hash evidence.
//!
//! The layouts here were checked against the PC98.org HDI/NHD format notes,
//! DOSBox-X's loader structures, and the independent 98imgtools readers.  No
//! filesystem or boot-sector bytes are inspected.

use std::sync::atomic::AtomicBool;

use super::{
    BoundedReader, DiskFormat, DiskFormatContext, DiskFormatEvidence, DiskFormatMetadata,
    DiskFormatRefusal, HardDiskLayout, MAX_HARD_DISK_IMAGE_BYTES, confidence_for, le_u16, le_u32,
};

const HDI_HEADER_BYTES: usize = 0x20;
const HDI_MAX_HEADER_BYTES: u64 = 64 * 1024;
const NHD_HEADER_BYTES: usize = 0x200;
const NHD_MAX_HEADER_BYTES: u64 = 64 * 1024;
const NHD_SIGNATURE: &[u8; 15] = b"T98HDDIMAGE.R0\0";

pub(super) fn inspect_hdi(
    reader: &mut BoundedReader<'_>,
    context: DiskFormatContext<'_>,
    cancel: Option<&AtomicBool>,
) -> DiskFormatEvidence {
    inspect(reader, context, cancel, DiskFormat::HdiContainer)
}

pub(super) fn inspect_nhd(
    reader: &mut BoundedReader<'_>,
    context: DiskFormatContext<'_>,
    cancel: Option<&AtomicBool>,
) -> DiskFormatEvidence {
    inspect(reader, context, cancel, DiskFormat::NhdContainer)
}

fn inspect(
    reader: &mut BoundedReader<'_>,
    context: DiskFormatContext<'_>,
    cancel: Option<&AtomicBool>,
    format: DiskFormat,
) -> DiskFormatEvidence {
    let result = match format {
        DiskFormat::HdiContainer => validate_hdi(reader, cancel).map(|(layout, type_code)| {
            (
                layout,
                vec![
                    format!(
                        "HDI header: {}-byte header, {}-byte data payload",
                        layout.header_bytes, layout.declared_payload_bytes
                    ),
                    format!(
                        "HDI geometry is {} cylinders x {} heads x {} sectors/track x {} bytes/sector",
                        layout.cylinders, layout.heads, layout.sectors_per_track, layout.sector_size
                    ),
                    format!("HDI type field is 0x{type_code:08x}; it is observed only"),
                ],
            )
        }),
        DiskFormat::NhdContainer => validate_nhd(reader, cancel).map(|(layout, comment)| {
            let mut evidence = vec![
                format!(
                    "NHD signature T98HDDIMAGE.R0, {}-byte header, {}-byte data payload",
                    layout.header_bytes, layout.declared_payload_bytes
                ),
                format!(
                    "NHD geometry is {} cylinders x {} heads x {} sectors/track x {} bytes/sector",
                    layout.cylinders, layout.heads, layout.sectors_per_track, layout.sector_size
                ),
                "NHD version is R0; comment field and reserved bytes are structurally valid"
                    .to_string(),
            ];
            if let Some(comment) = comment {
                evidence.push(format!("NHD comment: {comment}"));
            }
            (layout, evidence)
        }),
        _ => unreachable!("HDI adapter called for a different format"),
    };

    match result {
        Ok((layout, mut evidence)) => {
            evidence.push(
                "HDI/NHD structure proves only a hard-disk container; it does not prove PC-98, DOS, Windows, or X68000"
                    .to_string(),
            );
            let (confidence, conclusive) = if context
                .folder_platform
                .is_some_and(|folder| matches!(folder, "PC-98" | "NEC PC-9801"))
            {
                // Both historical identifiers remain stored separately, but
                // the platform registry declares them equivalent for evidence.
                (crate::platform::DetectionConfidence::Confirmed, true)
            } else {
                confidence_for(format, context)
            };
            DiskFormatEvidence {
                format: Some(format),
                platform: Some(format.platform()),
                confidence,
                conclusive,
                evidence,
                bytes_inspected: reader.bytes_read(),
                refusal: None,
                metadata: Some(match format {
                    DiskFormat::HdiContainer => DiskFormatMetadata::Hdi(layout),
                    DiskFormat::NhdContainer => DiskFormatMetadata::Nhd(layout),
                    _ => unreachable!(),
                }),
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

fn validate_hdi(
    reader: &mut BoundedReader<'_>,
    cancel: Option<&AtomicBool>,
) -> Result<(HardDiskLayout, u32), DiskFormatRefusal> {
    let length = checked_image_length(reader)?;
    if length < HDI_HEADER_BYTES as u64 {
        return Err(DiskFormatRefusal::TooSmall {
            length,
            minimum: HDI_HEADER_BYTES as u64,
        });
    }
    if super::cancelled(cancel) {
        return Err(DiskFormatRefusal::Cancelled);
    }
    let header = reader.read_exact_at(0, HDI_HEADER_BYTES)?;
    let malformed = |detail: String| DiskFormatRefusal::Malformed { detail };
    if le_u32(&header, 0) != Some(0) {
        return Err(malformed(
            "HDI reserved DWORD at 0x00 is not zero".to_string(),
        ));
    }
    let header_bytes =
        u64::from(le_u32(&header, 8).ok_or_else(|| malformed("truncated HDI header".to_string()))?);
    let declared_payload = u64::from(
        le_u32(&header, 0x0c)
            .ok_or_else(|| malformed("truncated HDI data-size field".to_string()))?,
    );
    let sector_size = u64::from(le_u32(&header, 0x10).unwrap());
    let sectors = u64::from(le_u32(&header, 0x14).unwrap());
    let heads = u64::from(le_u32(&header, 0x18).unwrap());
    let cylinders = u64::from(le_u32(&header, 0x1c).unwrap());
    validate_common(
        length,
        header_bytes,
        declared_payload,
        sector_size,
        sectors,
        heads,
        cylinders,
        HDI_MAX_HEADER_BYTES,
        "HDI",
    )?;
    let layout = HardDiskLayout {
        header_bytes,
        data_offset: header_bytes,
        sector_size,
        sectors_per_track: sectors,
        heads,
        cylinders,
        declared_payload_bytes: declared_payload,
        file_bytes: length,
        version: None,
    };
    Ok((layout, le_u32(&header, 4).unwrap()))
}

fn validate_nhd(
    reader: &mut BoundedReader<'_>,
    cancel: Option<&AtomicBool>,
) -> Result<(HardDiskLayout, Option<String>), DiskFormatRefusal> {
    let length = checked_image_length(reader)?;
    if length < NHD_HEADER_BYTES as u64 {
        return Err(DiskFormatRefusal::TooSmall {
            length,
            minimum: NHD_HEADER_BYTES as u64,
        });
    }
    if super::cancelled(cancel) {
        return Err(DiskFormatRefusal::Cancelled);
    }
    let header = reader.read_exact_at(0, NHD_HEADER_BYTES)?;
    let malformed = |detail: String| DiskFormatRefusal::Malformed { detail };
    if header.get(..15) != Some(NHD_SIGNATURE.as_slice()) || header[15] != 0 {
        return Err(malformed(
            "NHD signature is not T98HDDIMAGE.R0\\0".to_string(),
        ));
    }
    let header_bytes = u64::from(le_u32(&header, 0x110).unwrap());
    let cylinders = u64::from(le_u32(&header, 0x114).unwrap());
    let heads = u64::from(le_u16(&header, 0x118).unwrap());
    let sectors = u64::from(le_u16(&header, 0x11a).unwrap());
    let sector_size = u64::from(le_u16(&header, 0x11c).unwrap());
    let declared_payload = length.checked_sub(header_bytes).ok_or_else(|| {
        malformed(format!(
            "NHD data offset {header_bytes} is beyond the file length {length}"
        ))
    })?;
    validate_common(
        length,
        header_bytes,
        declared_payload,
        sector_size,
        sectors,
        heads,
        cylinders,
        NHD_MAX_HEADER_BYTES,
        "NHD",
    )?;
    if header[0x11e..].iter().any(|byte| *byte != 0) {
        return Err(malformed(
            "NHD reserved bytes after 0x11e are not zero".to_string(),
        ));
    }
    let comment = printable_c_string(&header[0x10..0x110]);
    Ok((
        HardDiskLayout {
            header_bytes,
            data_offset: header_bytes,
            sector_size,
            sectors_per_track: sectors,
            heads,
            cylinders,
            declared_payload_bytes: declared_payload,
            file_bytes: length,
            version: Some(0),
        },
        comment,
    ))
}

fn validate_common(
    file_bytes: u64,
    header_bytes: u64,
    declared_payload: u64,
    sector_size: u64,
    sectors: u64,
    heads: u64,
    cylinders: u64,
    max_header: u64,
    label: &str,
) -> Result<(), DiskFormatRefusal> {
    let malformed = |detail: String| DiskFormatRefusal::Malformed { detail };
    if !(if label == "HDI" {
        header_bytes >= HDI_HEADER_BYTES as u64
    } else {
        header_bytes >= NHD_HEADER_BYTES as u64
    }) || header_bytes > max_header
    {
        return Err(malformed(format!(
            "{label} header size {header_bytes} is outside the supported range"
        )));
    }
    let end = header_bytes
        .checked_add(declared_payload)
        .ok_or_else(|| malformed(format!("{label} header plus payload overflows")))?;
    if end != file_bytes {
        return Err(malformed(format!(
            "{label} header plus declared payload is {end} bytes, but file is {file_bytes}"
        )));
    }
    if sector_size == 0 || !sector_size.is_power_of_two() || !(256..=1024).contains(&sector_size) {
        return Err(malformed(format!(
            "{label} sector size {sector_size} is not a supported power-of-two size in 256..=1024"
        )));
    }
    if sectors == 0 || heads == 0 || cylinders == 0 {
        return Err(malformed(format!(
            "{label} geometry contains a zero dimension"
        )));
    }
    let computed = cylinders
        .checked_mul(heads)
        .and_then(|value| value.checked_mul(sectors))
        .and_then(|value| value.checked_mul(sector_size))
        .ok_or_else(|| malformed(format!("{label} geometry payload multiplication overflows")))?;
    if computed != declared_payload {
        return Err(malformed(format!(
            "{label} geometry declares {computed} payload bytes, but header/file declares {declared_payload}"
        )));
    }
    if end > MAX_HARD_DISK_IMAGE_BYTES {
        return Err(DiskFormatRefusal::TooLarge {
            length: end,
            maximum: MAX_HARD_DISK_IMAGE_BYTES,
        });
    }
    Ok(())
}

fn checked_image_length(reader: &BoundedReader<'_>) -> Result<u64, DiskFormatRefusal> {
    let length = reader.len();
    if length > MAX_HARD_DISK_IMAGE_BYTES {
        return Err(DiskFormatRefusal::TooLarge {
            length,
            maximum: MAX_HARD_DISK_IMAGE_BYTES,
        });
    }
    Ok(length)
}

fn printable_c_string(bytes: &[u8]) -> Option<String> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let text = &bytes[..end];
    if text.is_empty() || text.iter().any(|byte| !(0x20..=0x7e).contains(byte)) {
        return None;
    }
    Some(String::from_utf8_lossy(text).into_owned())
}
