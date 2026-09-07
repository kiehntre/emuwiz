//! Bounded, read-only Apple II disk structural evidence.
//!
//! This module validates the small, well-defined structures common to Apple II
//! disk images. It does not emulate GCR, repair sectors, or flatten WOZ/NIB
//! preservation images into logical blocks.

use std::path::Path;

pub const DOS33_BYTES: usize = 35 * 16 * 256;
pub const PRODOS_BLOCK_BYTES: usize = 512;
pub const MAX_WOZ_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Apple2Container {
    DskDo,
    Po,
    TwoMg,
    Woz,
    Nib,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Apple2Filesystem {
    Dos33,
    ProDos,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Apple2Confidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Apple2DiskEvidence {
    pub container: Apple2Container,
    pub filesystem: Apple2Filesystem,
    pub tracks: Option<u16>,
    pub sectors: Option<u16>,
    pub blocks: Option<u32>,
    pub volume_number: Option<u8>,
    pub volume_name: Option<String>,
    pub catalogue_entries: usize,
    pub woz_track_map_entries: Option<usize>,
    pub confidence: Apple2Confidence,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Apple2DiskError {
    TooLarge,
    Malformed(String),
    Unsupported(String),
}

impl std::fmt::Display for Apple2DiskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => f.write_str("Apple II image exceeds bounded inspection limit"),
            Self::Malformed(s) => write!(f, "malformed Apple II image: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported Apple II image: {s}"),
        }
    }
}
impl std::error::Error for Apple2DiskError {}

fn clean_name(bytes: &[u8]) -> Option<String> {
    let value: String = bytes
        .iter()
        .copied()
        .filter(|b| *b >= 0x20 && *b < 0x7f)
        .map(char::from)
        .collect::<String>()
        .trim()
        .to_string();
    (!value.is_empty()).then_some(value)
}

fn parse_dos33(
    bytes: &[u8],
    container: Apple2Container,
) -> Result<Apple2DiskEvidence, Apple2DiskError> {
    if bytes.len() != DOS33_BYTES {
        return Err(Apple2DiskError::Malformed(
            "140 KiB DOS image has incorrect geometry".into(),
        ));
    }
    let vtoc = 17 * 16 * 256;
    if bytes[vtoc] != 17 || bytes[vtoc + 1] != 0 {
        return Err(Apple2DiskError::Malformed(
            "VTOC does not identify track 17/sector 0".into(),
        ));
    }
    let volume = bytes[vtoc + 6];
    let mut entries = 0usize;
    let mut warnings = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut track = 17u8;
    let mut sector = 1u8;
    for _ in 0..32 {
        if track == 0 {
            break;
        }
        if track >= 35 || sector >= 16 {
            return Err(Apple2DiskError::Malformed(
                "catalogue pointer out of range".into(),
            ));
        }
        if !seen.insert((track, sector)) {
            return Err(Apple2DiskError::Malformed("catalogue chain loops".into()));
        }
        let off = (track as usize * 16 + sector as usize) * 256;
        let next_track = bytes[off];
        let next_sector = bytes[off + 1];
        for slot in 0..7 {
            let e = off + 2 + slot * 35;
            let first_track = bytes[e + 0];
            let first_sector = bytes[e + 1];
            if first_track != 0 || first_sector != 0 {
                entries += 1;
                if first_track >= 35 || first_sector >= 16 {
                    warnings.push(format!(
                        "catalogue entry {slot} has out-of-range file pointer"
                    ));
                }
            }
        }
        track = next_track;
        sector = next_sector;
    }
    if track != 0 {
        warnings.push("catalogue chain exceeds bounded length".into());
    }
    Ok(Apple2DiskEvidence {
        container,
        filesystem: Apple2Filesystem::Dos33,
        tracks: Some(35),
        sectors: Some(16),
        blocks: None,
        volume_number: Some(volume),
        volume_name: None,
        catalogue_entries: entries,
        woz_track_map_entries: None,
        confidence: if warnings.is_empty() {
            Apple2Confidence::High
        } else {
            Apple2Confidence::Medium
        },
        warnings,
    })
}

fn parse_prodos(
    bytes: &[u8],
    container: Apple2Container,
) -> Result<Apple2DiskEvidence, Apple2DiskError> {
    if bytes.len() % PRODOS_BLOCK_BYTES != 0 || bytes.len() < 3 * PRODOS_BLOCK_BYTES {
        return Err(Apple2DiskError::Malformed(
            "ProDOS image is not block aligned".into(),
        ));
    }
    let b = &bytes[2 * PRODOS_BLOCK_BYTES..3 * PRODOS_BLOCK_BYTES];
    let entry_len = b[4];
    let entries_per_block = b[5];
    let count = u16::from_le_bytes([b[6], b[7]]);
    if entry_len != 39 || entries_per_block != 13 || count == 0 {
        return Err(Apple2DiskError::Malformed(
            "invalid ProDOS directory header".into(),
        ));
    }
    let first = &b[8..];
    let storage = first[0] >> 4;
    let name_len = (first[0] & 0x0f) as usize;
    if storage == 0 || name_len == 0 || name_len > 15 {
        return Err(Apple2DiskError::Malformed(
            "invalid ProDOS volume entry".into(),
        ));
    }
    let name = clean_name(&first[1..1 + name_len]);
    let mut warnings = Vec::new();
    let max_entries = (count as usize).min(13);
    for index in 0..max_entries {
        let off = 8 + index * entry_len as usize;
        if off + entry_len as usize > b.len() {
            warnings.push("directory entry extends beyond block".into());
            break;
        }
        let st = b[off] >> 4;
        if st > 0xF {
            warnings.push("invalid storage type".into());
        }
    }
    Ok(Apple2DiskEvidence {
        container,
        filesystem: Apple2Filesystem::ProDos,
        tracks: None,
        sectors: None,
        blocks: Some((bytes.len() / 512) as u32),
        volume_number: None,
        volume_name: name,
        catalogue_entries: count as usize,
        woz_track_map_entries: None,
        confidence: if warnings.is_empty() {
            Apple2Confidence::High
        } else {
            Apple2Confidence::Medium
        },
        warnings,
    })
}

fn parse_2mg(bytes: &[u8]) -> Result<Apple2DiskEvidence, Apple2DiskError> {
    if bytes.len() < 64 || &bytes[..4] != b"2IMG" {
        return Err(Apple2DiskError::Malformed(
            "missing 2IMG signature/header".into(),
        ));
    }
    let header_len = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
    let format = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
    let blocks = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
    let offset = u32::from_le_bytes(bytes[24..28].try_into().unwrap()) as usize;
    let length = u32::from_le_bytes(bytes[28..32].try_into().unwrap()) as usize;
    if !(header_len >= 64
        && offset >= header_len
        && offset.checked_add(length).is_some_and(|e| e <= bytes.len()))
    {
        return Err(Apple2DiskError::Malformed(
            "2IMG data range is out of bounds".into(),
        ));
    }
    let filesystem = match format {
        0 => Apple2Filesystem::Dos33,
        1 => Apple2Filesystem::ProDos,
        _ => Apple2Filesystem::Unknown,
    };
    Ok(Apple2DiskEvidence {
        container: Apple2Container::TwoMg,
        filesystem,
        tracks: None,
        sectors: None,
        blocks: Some(blocks),
        volume_number: None,
        volume_name: None,
        catalogue_entries: 0,
        woz_track_map_entries: None,
        confidence: Apple2Confidence::High,
        warnings: if filesystem == Apple2Filesystem::Unknown {
            vec!["2IMG format is not a recognized logical filesystem".into()]
        } else {
            Vec::new()
        },
    })
}

fn parse_woz(bytes: &[u8]) -> Result<Apple2DiskEvidence, Apple2DiskError> {
    if bytes.len() > MAX_WOZ_BYTES
        || bytes.len() < 12
        || !(&bytes[..4] == b"WOZ1" || &bytes[..4] == b"WOZ2")
    {
        return Err(Apple2DiskError::Malformed(
            "missing WOZ signature or bounded size".into(),
        ));
    }
    if bytes[4..8] != [0xff, 0x0a, 0x0d, 0x0a] {
        return Err(Apple2DiskError::Malformed(
            "invalid WOZ marker bytes".into(),
        ));
    }
    let mut pos = 12usize;
    let mut map = None;
    let mut warnings = Vec::new();
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        pos += 8;
        let end = pos
            .checked_add(len)
            .ok_or_else(|| Apple2DiskError::Malformed("WOZ chunk overflow".into()))?;
        if end > bytes.len() {
            return Err(Apple2DiskError::Malformed("WOZ chunk exceeds image".into()));
        }
        if id == b"TMAP" {
            if len != 160 {
                warnings.push("WOZ TMAP chunk has non-standard length".into());
            }
            map = Some(len.min(160));
        }
        pos = end;
    }
    if map.is_none() {
        warnings.push("WOZ track map chunk not present".into());
    }
    Ok(Apple2DiskEvidence {
        container: Apple2Container::Woz,
        filesystem: Apple2Filesystem::Unknown,
        tracks: None,
        sectors: None,
        blocks: None,
        volume_number: None,
        volume_name: None,
        catalogue_entries: 0,
        woz_track_map_entries: map,
        confidence: if warnings.is_empty() {
            Apple2Confidence::High
        } else {
            Apple2Confidence::Medium
        },
        warnings,
    })
}

/// Inspect Apple II bytes. `extension` is only used to choose an explicitly
/// named container; it never creates platform identity by itself.
pub fn inspect_apple2_disk(
    bytes: &[u8],
    extension: &str,
) -> Result<Apple2DiskEvidence, Apple2DiskError> {
    if bytes.len() > MAX_WOZ_BYTES {
        return Err(Apple2DiskError::TooLarge);
    }
    let ext = extension.trim_start_matches('.').to_ascii_lowercase();
    match ext.as_str() {
        "2mg" => parse_2mg(bytes),
        "woz" => parse_woz(bytes),
        "nib" => {
            if bytes.len() == 35 * 6656 {
                Ok(Apple2DiskEvidence {
                    container: Apple2Container::Nib,
                    filesystem: Apple2Filesystem::Unknown,
                    tracks: Some(35),
                    sectors: None,
                    blocks: None,
                    volume_number: None,
                    volume_name: None,
                    catalogue_entries: 0,
                    woz_track_map_entries: None,
                    confidence: Apple2Confidence::Medium,
                    warnings: vec![
                        "NIB is a nibble-stream preservation image; logical filesystem not decoded"
                            .into(),
                    ],
                })
            } else {
                Err(Apple2DiskError::Malformed(
                    "unsupported NIB geometry".into(),
                ))
            }
        }
        "po" => parse_prodos(bytes, Apple2Container::Po),
        "do" | "dsk" => parse_dos33(bytes, Apple2Container::DskDo)
            .or_else(|_| parse_prodos(bytes, Apple2Container::DskDo)),
        _ => Err(Apple2DiskError::Unsupported(
            "unknown Apple II container extension".into(),
        )),
    }
}

pub fn inspect_apple2_disk_file(path: &Path) -> Result<Apple2DiskEvidence, Apple2DiskError> {
    let bytes = std::fs::read(path).map_err(|e| Apple2DiskError::Malformed(e.to_string()))?;
    inspect_apple2_disk(
        &bytes,
        path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dos33_fixture() -> Vec<u8> {
        let mut d = vec![0; DOS33_BYTES];
        let vtoc = 17 * 16 * 256;
        d[vtoc] = 17;
        d[vtoc + 1] = 0;
        d[vtoc + 6] = 254;
        let cat = (17 * 16 + 1) * 256;
        d[cat + 2] = 1;
        d[cat + 3] = 2;
        d[cat + 5..cat + 9].copy_from_slice(b"TEST");
        d
    }

    fn prodos_fixture() -> Vec<u8> {
        let mut d = vec![0; 280 * PRODOS_BLOCK_BYTES];
        let b = &mut d[2 * PRODOS_BLOCK_BYTES..3 * PRODOS_BLOCK_BYTES];
        b[4] = 39;
        b[5] = 13;
        b[6..8].copy_from_slice(&1u16.to_le_bytes());
        b[8] = 0xD0 | 4;
        b[9..13].copy_from_slice(b"TEST");
        d
    }

    #[test]
    fn valid_dos33_vtoc_and_catalogue_are_reported() {
        let e = inspect_apple2_disk(&dos33_fixture(), "dsk").unwrap();
        assert_eq!(e.filesystem, Apple2Filesystem::Dos33);
        assert_eq!(e.volume_number, Some(254));
        assert_eq!(e.catalogue_entries, 1);
    }

    #[test]
    fn valid_prodos_directory_header_is_reported() {
        let e = inspect_apple2_disk(&prodos_fixture(), "po").unwrap();
        assert_eq!(e.filesystem, Apple2Filesystem::ProDos);
        assert_eq!(e.volume_name.as_deref(), Some("TEST"));
    }

    #[test]
    fn valid_2mg_range_is_reported() {
        let mut d = vec![0; 64 + 512];
        d[..4].copy_from_slice(b"2IMG");
        d[8..10].copy_from_slice(&64u16.to_le_bytes());
        d[20..24].copy_from_slice(&1u32.to_le_bytes());
        d[24..28].copy_from_slice(&64u32.to_le_bytes());
        d[28..32].copy_from_slice(&512u32.to_le_bytes());
        let e = inspect_apple2_disk(&d, "2mg").unwrap();
        assert_eq!(e.blocks, Some(1));
    }

    #[test]
    fn valid_woz_header_and_track_map_are_reported() {
        let mut d = vec![0; 12 + 8 + 160];
        d[..4].copy_from_slice(b"WOZ2");
        d[4..8].copy_from_slice(&[0xff, 0x0a, 0x0d, 0x0a]);
        d[12..16].copy_from_slice(b"TMAP");
        d[16..20].copy_from_slice(&160u32.to_le_bytes());
        let e = inspect_apple2_disk(&d, "woz").unwrap();
        assert_eq!(e.woz_track_map_entries, Some(160));
    }

    #[test]
    fn random_140k_dsk_is_rejected() {
        let d = vec![0; DOS33_BYTES];
        assert!(inspect_apple2_disk(&d, "dsk").is_err());
    }
    #[test]
    fn nib_is_preservation_only() {
        let d = vec![0; 35 * 6656];
        let e = inspect_apple2_disk(&d, "nib").unwrap();
        assert_eq!(e.filesystem, Apple2Filesystem::Unknown);
    }
    #[test]
    fn malformed_2mg_is_refused() {
        assert!(inspect_apple2_disk(b"2IMG", "2mg").is_err());
    }
}
