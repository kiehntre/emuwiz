//! Bounded structural evidence for the VICE/CCS64 CRT cartridge container.
//!
//! Layout verified against two independent implementations/documentations:
//! the VICE manual, section 17.14
//! (https://vice-emu.sourceforge.io/vice_17.html), and the ReplayResources
//! CRT specification (https://rr.c64.org/wiki/CRT_Format). Both define the
//! 64-byte fixed header, big-endian fields, and 16-byte CHIP packet header.
//!
//! This adapter validates the container only. It does not emulate mapper
//! behaviour, and an unknown hardware ID remains structurally valid. A C64 CRT
//! is shared by Commodore 8-bit machines in practice, so its evidence is
//! deliberately family-level and not a bare C64 identity claim.

use std::sync::atomic::AtomicBool;

use super::{
    BoundedReader, CrtLayout, DiskFormat, DiskFormatContext, DiskFormatEvidence,
    DiskFormatMetadata, DiskFormatRefusal, confidence_for,
};

const CRT_HEADER_BYTES: u64 = 0x40;
const CHIP_HEADER_BYTES: u64 = 0x10;
const CRT_SIGNATURE: &[u8; 16] = b"C64 CARTRIDGE   ";
const CHIP_SIGNATURE: &[u8; 4] = b"CHIP";
const MAX_CRT_BYTES: u64 = 32 * 1024 * 1024;

pub(super) fn inspect(
    reader: &mut BoundedReader<'_>,
    context: DiskFormatContext<'_>,
    cancel: Option<&AtomicBool>,
) -> DiskFormatEvidence {
    match validate(reader, cancel) {
        Ok(layout) => {
            let format = DiskFormat::CommodoreCrt;
            let (confidence, conclusive) = confidence_for(format, context);
            let mut evidence = vec![
                format!(
                    "CRT header and {} CHIP packet(s) are structurally valid (version {:04x}, hardware type 0x{:04x})",
                    layout.packets, layout.version, layout.hardware_type
                ),
                "CRT proves a Commodore 8-bit cartridge container shared across C64/C128/VIC-20, not a specific machine".to_string(),
                format!("CRT EXROM line: {}; GAME line: {}", layout.exrom, layout.game),
            ];
            if !layout.cartridge_name.is_empty() {
                evidence.push(format!("CRT cartridge name: {}", layout.cartridge_name));
            }
            if layout.chip_types.iter().any(|chip_type| *chip_type > 3) {
                evidence.push(
                    "CRT contains an unknown CHIP storage type; packet structure remains valid"
                        .to_string(),
                );
            }
            evidence.push(format!(
                "CRT CHIP payload bytes: {}",
                layout.total_image_bytes
            ));
            DiskFormatEvidence {
                format: Some(format),
                platform: Some(format.platform()),
                confidence,
                conclusive,
                evidence,
                bytes_inspected: reader.bytes_read(),
                refusal: None,
                metadata: Some(DiskFormatMetadata::Crt(layout)),
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
) -> Result<CrtLayout, DiskFormatRefusal> {
    let length = reader.len();
    if length < CRT_HEADER_BYTES {
        return Err(DiskFormatRefusal::TooSmall {
            length,
            minimum: CRT_HEADER_BYTES,
        });
    }
    if length > MAX_CRT_BYTES {
        return Err(DiskFormatRefusal::TooLarge {
            length,
            maximum: MAX_CRT_BYTES,
        });
    }
    if super::cancelled(cancel) {
        return Err(DiskFormatRefusal::Cancelled);
    }
    let header = reader.read_exact_at(0, CRT_HEADER_BYTES as usize)?;
    if &header[..16] != CRT_SIGNATURE {
        return Err(malformed("CRT signature is not `C64 CARTRIDGE   `"));
    }
    let header_length =
        be_u32(&header, 0x10).ok_or_else(|| malformed("CRT header length is truncated"))?;
    if u64::from(header_length) < CRT_HEADER_BYTES || u64::from(header_length) > length {
        return Err(malformed("CRT header length is outside the file"));
    }
    let version = be_u16(&header, 0x14).ok_or_else(|| malformed("CRT version is truncated"))?;
    if !matches!(version, 0x0100 | 0x0101 | 0x0200) {
        return Err(malformed(format!(
            "unsupported CRT version 0x{version:04x}"
        )));
    }
    let hardware_type = be_u16(&header, 0x16).unwrap();
    let exrom = header[0x18];
    let game = header[0x19];
    if exrom > 1 || game > 1 {
        return Err(malformed("CRT EXROM/GAME line is not 0 or 1"));
    }
    let name = header[0x20..0x40]
        .iter()
        .position(|byte| *byte == 0)
        .map_or(&header[0x20..0x40], |end| &header[0x20..0x20 + end]);
    let cartridge_name = String::from_utf8_lossy(name).trim().to_string();

    let mut cursor = u64::from(header_length);
    let mut packets = 0usize;
    let mut chip_types = Vec::new();
    let mut banks = Vec::new();
    let mut total_image_bytes = 0u64;
    while cursor < length {
        if super::cancelled(cancel) {
            return Err(DiskFormatRefusal::Cancelled);
        }
        let packet = reader.read_exact_at_crt(cursor, CHIP_HEADER_BYTES as usize)?;
        if &packet[..4] != CHIP_SIGNATURE {
            return Err(malformed(format!(
                "CRT packet {packets} has a bad CHIP signature"
            )));
        }
        let packet_length = u64::from(be_u32(&packet, 4).unwrap());
        let image_size = u64::from(be_u16(&packet, 0x0e).unwrap());
        if packet_length < CHIP_HEADER_BYTES {
            return Err(malformed(format!(
                "CRT packet {packets} length is smaller than its header"
            )));
        }
        let expected_length = CHIP_HEADER_BYTES
            .checked_add(image_size)
            .ok_or_else(|| malformed("CRT packet length overflows"))?;
        if packet_length != expected_length {
            return Err(malformed(format!(
                "CRT packet {packets} length disagrees with image size"
            )));
        }
        let end = cursor
            .checked_add(packet_length)
            .ok_or_else(|| malformed("CRT packet extent overflows"))?;
        if end > length {
            return Err(malformed(format!("CRT packet {packets} extends past EOF")));
        }
        chip_types.push(be_u16(&packet, 8).unwrap());
        banks.push(be_u16(&packet, 0x0a).unwrap());
        total_image_bytes = total_image_bytes
            .checked_add(image_size)
            .ok_or_else(|| malformed("CRT image-size total overflows"))?;
        packets = packets
            .checked_add(1)
            .ok_or_else(|| malformed("CRT packet count overflows"))?;
        cursor = end;
    }
    if packets == 0 {
        return Err(malformed("CRT contains no CHIP packets"));
    }
    Ok(CrtLayout {
        header_length,
        version,
        hardware_type,
        exrom,
        game,
        cartridge_name,
        packets,
        chip_types,
        banks,
        total_image_bytes,
    })
}

fn be_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn be_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn malformed(detail: impl Into<String>) -> DiskFormatRefusal {
    DiskFormatRefusal::Malformed {
        detail: detail.into(),
    }
}
