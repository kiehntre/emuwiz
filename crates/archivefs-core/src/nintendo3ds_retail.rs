//! Bounded, read-only structural identity for Nintendo 3DS retail media.
//!
//! This module deliberately stops at headers and container metadata.  It does
//! not decrypt NCCH content, read keys, or turn a CIA into a launch target.

use std::io::{Read, Seek, SeekFrom};

const MEDIA_UNIT: u64 = 0x200;
const NCSD_HEADER: u64 = 0x200;
const NCCH_HEADER: u64 = 0x200;
const MAX_CIA_HEADER: u32 = 0x100000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreeDsRetailFormat {
    Cci,
    Cia,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn cci_fixture(no_crypto: bool) -> Vec<u8> {
        let mut data = vec![0u8; 0x800];
        data[0x100..0x104].copy_from_slice(b"NCSD");
        data[0x104..0x108].copy_from_slice(&4u32.to_le_bytes());
        data[0x120..0x124].copy_from_slice(&2u32.to_le_bytes());
        data[0x124..0x128].copy_from_slice(&2u32.to_le_bytes());
        let h = &mut data[0x400..0x600];
        h[0x100..0x104].copy_from_slice(b"NCCH");
        h[0x118..0x120].copy_from_slice(&0x0004000000123456u64.to_le_bytes());
        h[0x150..0x15a].copy_from_slice(b"CTR-P-TEST");
        h[0x18d] = 2;
        if no_crypto {
            h[0x18f] = 4;
        }
        data
    }

    #[test]
    fn cci_reads_bounded_identity_and_encryption() {
        let mut data = cci_fixture(true);
        let observation = parse_cci(&mut Cursor::new(&mut data), 0x800).unwrap();
        assert_eq!(observation.title_id.as_deref(), Some("0004000000123456"));
        assert_eq!(observation.product_code.as_deref(), Some("CTR-P-TEST"));
        assert_eq!(observation.encryption, ThreeDsEncryption::Unencrypted);
        assert_eq!(observation.title_kind, ThreeDsTitleKind::Base);
    }

    #[test]
    fn cci_rejects_partition_outside_declared_image() {
        let mut data = cci_fixture(false);
        data[0x124..0x128].copy_from_slice(&4u32.to_le_bytes());
        assert!(parse_cci(&mut Cursor::new(&mut data), 0x800).is_err());
    }

    #[test]
    fn cia_reads_tmd_title_id_without_reading_content() {
        let mut data = vec![0u8; 0x400];
        data[0..4].copy_from_slice(&0x20u32.to_le_bytes());
        data[0x10..0x14].copy_from_slice(&0x200u32.to_le_bytes());
        data[0x18..0x20].copy_from_slice(&0u64.to_le_bytes());
        data[0x40 + 0x18c..0x40 + 0x194].copy_from_slice(&0x0004000e12345678u64.to_be_bytes());
        let length = data.len() as u64;
        let observation = parse_cia(&mut Cursor::new(&mut data), length).unwrap();
        assert_eq!(observation.title_id.as_deref(), Some("0004000E12345678"));
        assert_eq!(observation.encryption, ThreeDsEncryption::Encrypted);
        assert_eq!(observation.title_kind, ThreeDsTitleKind::Update);
    }

    #[test]
    fn cia_rejects_section_overflow() {
        let mut data = vec![0u8; 0x40];
        data[0..4].copy_from_slice(&0x20u32.to_le_bytes());
        data[0x10..0x14].copy_from_slice(&u32::MAX.to_le_bytes());
        let length = data.len() as u64;
        assert!(parse_cia(&mut Cursor::new(&mut data), length).is_err());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreeDsEncryption {
    Encrypted,
    Unencrypted,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreeDsTitleKind {
    Base,
    Update,
    Dlc,
    System,
    Manual,
    Child,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreeDsPartition {
    pub index: usize,
    pub offset: u64,
    pub length: u64,
    pub content_type: Option<u8>,
    pub title_id: Option<String>,
    pub product_code: Option<String>,
    pub encryption: ThreeDsEncryption,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreeDsRetailEvidence {
    pub format: ThreeDsRetailFormat,
    pub title_id: Option<String>,
    pub product_code: Option<String>,
    pub title_kind: ThreeDsTitleKind,
    pub title_version: Option<u16>,
    pub encryption: ThreeDsEncryption,
    pub partitions: Vec<ThreeDsPartition>,
    pub region_flags: Option<String>,
    pub warnings: Vec<String>,
}

fn slice<const N: usize>(data: &[u8], offset: usize) -> Option<&[u8; N]> {
    data.get(offset..offset.checked_add(N)?)
        .and_then(|v| v.try_into().ok())
}

fn u32le(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(*slice(data, offset)?))
}
fn u64le(data: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(*slice(data, offset)?))
}

fn title_id(data: &[u8], offset: usize) -> Option<String> {
    Some(format!("{:016X}", u64le(data, offset)?))
}

fn tmd_title_id(data: &[u8], offset: usize) -> Option<String> {
    let raw = data.get(offset..offset.checked_add(8)?)?;
    Some(raw.iter().map(|byte| format!("{byte:02X}")).collect())
}

fn product_code(data: &[u8], offset: usize) -> Option<String> {
    let raw = data.get(offset..offset.checked_add(16)?)?;
    let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
    let value = std::str::from_utf8(&raw[..end]).ok()?.trim();
    (!value.is_empty() && value.bytes().all(|b| b.is_ascii_graphic())).then(|| value.to_owned())
}

fn checked_media_range(offset: u32, length: u32, file_len: u64) -> Result<(u64, u64), String> {
    let start = u64::from(offset)
        .checked_mul(MEDIA_UNIT)
        .ok_or("partition offset overflow")?;
    let len = u64::from(length)
        .checked_mul(MEDIA_UNIT)
        .ok_or("partition length overflow")?;
    let end = start.checked_add(len).ok_or("partition end overflow")?;
    if len < NCCH_HEADER || end > file_len {
        return Err("partition is outside the image".into());
    }
    Ok((start, len))
}

fn classify_content(index: usize, title: Option<&str>, content: Option<u8>) -> ThreeDsTitleKind {
    if index == 6 || index == 7 {
        return ThreeDsTitleKind::Update;
    }
    if let Some(id) = title {
        if id.starts_with("0004000E") {
            return ThreeDsTitleKind::Update;
        }
        if id.starts_with("0004008C") {
            return ThreeDsTitleKind::Dlc;
        }
        if id.starts_with("00040000") {
            return ThreeDsTitleKind::Base;
        }
    }
    match content {
        Some(0x04) => ThreeDsTitleKind::System,
        Some(0x08) => ThreeDsTitleKind::Manual,
        Some(0x10) => ThreeDsTitleKind::Child,
        Some(0x02) | Some(0x01) => ThreeDsTitleKind::Base,
        _ => ThreeDsTitleKind::Unknown,
    }
}

fn parse_ncch_header(
    header: &[u8],
    index: usize,
    offset: u64,
    length: u64,
) -> Result<ThreeDsPartition, String> {
    if header.get(0x100..0x104) != Some(b"NCCH") {
        return Err("partition is not an NCCH".into());
    }
    let tid = title_id(header, 0x118);
    let product = product_code(header, 0x150);
    let content = header.get(0x18d).copied().map(|v| v & 0x1f);
    let no_crypto = header.get(0x18f).is_some_and(|v| v & 0x04 != 0);
    Ok(ThreeDsPartition {
        index,
        offset,
        length,
        content_type: content,
        title_id: tid,
        product_code: product,
        encryption: if no_crypto {
            ThreeDsEncryption::Unencrypted
        } else {
            ThreeDsEncryption::Encrypted
        },
    })
}

pub fn parse_cci<R: Read + Seek>(
    reader: &mut R,
    file_len: u64,
) -> Result<ThreeDsRetailEvidence, String> {
    if file_len < NCSD_HEADER {
        return Err("NCSD header is truncated".into());
    }
    let mut header = [0u8; NCSD_HEADER as usize];
    reader.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    reader.read_exact(&mut header).map_err(|e| e.to_string())?;
    if &header[0x100..0x104] != b"NCSD" {
        return Err("NCSD magic is missing".into());
    }
    let declared = u64::from(u32le(&header, 0x104).ok_or("NCSD size is missing")?)
        .checked_mul(MEDIA_UNIT)
        .ok_or("NCSD size overflow")?;
    if declared < NCSD_HEADER || declared > file_len {
        return Err("NCSD declared size exceeds the image".into());
    }
    let mut partitions = Vec::new();
    let mut warnings = vec![
        "Region is not available from the unencrypted retail headers; no decryption was attempted."
            .into(),
    ];
    for index in 0..8 {
        let base = 0x120 + index * 8;
        let off = u32le(&header, base).ok_or("NCSD partition table is truncated")?;
        let len = u32le(&header, base + 4).ok_or("NCSD partition table is truncated")?;
        if len == 0 {
            continue;
        }
        let (start, length) = checked_media_range(off, len, declared)?;
        let mut ncch = [0u8; NCCH_HEADER as usize];
        reader
            .seek(SeekFrom::Start(start))
            .map_err(|e| e.to_string())?;
        reader.read_exact(&mut ncch).map_err(|e| e.to_string())?;
        partitions.push(parse_ncch_header(&ncch, index, start, length)?);
    }
    if partitions.is_empty() {
        warnings.push("NCSD has no non-empty partitions.".into());
    }
    let first = partitions.first();
    let title_kind = first
        .map(|p| classify_content(p.index, p.title_id.as_deref(), p.content_type))
        .unwrap_or(ThreeDsTitleKind::Unknown);
    let encryption = if partitions
        .iter()
        .all(|p| p.encryption == ThreeDsEncryption::Unencrypted)
    {
        ThreeDsEncryption::Unencrypted
    } else {
        ThreeDsEncryption::Encrypted
    };
    Ok(ThreeDsRetailEvidence {
        format: ThreeDsRetailFormat::Cci,
        title_id: first.and_then(|p| p.title_id.clone()),
        product_code: first.and_then(|p| p.product_code.clone()),
        title_kind,
        title_version: u16le(&header, 0x310),
        encryption,
        partitions,
        region_flags: None,
        warnings,
    })
}

fn u16le(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(*slice(data, offset)?))
}
fn align64(value: u64) -> Option<u64> {
    value.checked_add(63).map(|v| v & !63)
}

pub fn parse_cia<R: Read + Seek>(
    reader: &mut R,
    file_len: u64,
) -> Result<ThreeDsRetailEvidence, String> {
    if file_len < 0x20 {
        return Err("CIA header is truncated".into());
    }
    let mut header = [0u8; 0x20];
    reader.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    reader.read_exact(&mut header).map_err(|e| e.to_string())?;
    let header_size = u32le(&header, 0).ok_or("CIA header size is missing")?;
    if !(0x20..=MAX_CIA_HEADER).contains(&header_size) {
        return Err("CIA header size is invalid".into());
    }
    let cert = u64::from(u32le(&header, 8).ok_or("CIA certificate size is missing")?);
    let ticket = u64::from(u32le(&header, 0xc).ok_or("CIA ticket size is missing")?);
    let tmd = u64::from(u32le(&header, 0x10).ok_or("CIA TMD size is missing")?);
    let content = u64::from(u64le(&header, 0x18).ok_or("CIA content size is missing")?);
    let cert_start = align64(u64::from(header_size)).ok_or("CIA section overflow")?;
    let ticket_start = align64(
        cert_start
            .checked_add(cert)
            .ok_or("CIA certificate overflow")?,
    )
    .ok_or("CIA section overflow")?;
    let tmd_start = align64(
        ticket_start
            .checked_add(ticket)
            .ok_or("CIA ticket overflow")?,
    )
    .ok_or("CIA section overflow")?;
    let content_start = align64(tmd_start.checked_add(tmd).ok_or("CIA TMD overflow")?)
        .ok_or("CIA section overflow")?;
    let end = content_start
        .checked_add(content)
        .ok_or("CIA content overflow")?;
    if end > file_len {
        return Err("CIA section exceeds the image".into());
    }
    if tmd < 0x194 {
        return Err("CIA TMD is too small for a title ID".into());
    }
    let mut tmd_header = [0u8; 0x194];
    reader
        .seek(SeekFrom::Start(tmd_start))
        .map_err(|e| e.to_string())?;
    reader
        .read_exact(&mut tmd_header)
        .map_err(|e| e.to_string())?;
    let tmd_title_id = tmd_title_id(&tmd_header, 0x18c);
    let mut partitions = Vec::new();
    if content >= NCCH_HEADER {
        let mut ncch = [0u8; NCCH_HEADER as usize];
        reader
            .seek(SeekFrom::Start(content_start))
            .map_err(|e| e.to_string())?;
        reader.read_exact(&mut ncch).map_err(|e| e.to_string())?;
        if &ncch[0x100..0x104] == b"NCCH" {
            partitions.push(parse_ncch_header(&ncch, 0, content_start, content)?);
        }
    }
    let title_id = partitions
        .first()
        .and_then(|partition| partition.title_id.clone())
        .or(tmd_title_id);
    let encryption = partitions
        .first()
        .map(|partition| partition.encryption)
        .unwrap_or(ThreeDsEncryption::Encrypted);
    Ok(ThreeDsRetailEvidence {
        format: ThreeDsRetailFormat::Cia,
        title_kind: classify_content(
            0,
            title_id.as_deref(),
            partitions.first().and_then(|partition| partition.content_type),
        ),
        title_id,
        product_code: partitions
            .first()
            .and_then(|partition| partition.product_code.clone()),
        title_version: None,
        encryption,
        partitions,
        region_flags: None,
        warnings: vec![
            "CIA is an installable package and is not a direct launch target; no decryption was attempted."
                .into(),
        ],
    })
}
