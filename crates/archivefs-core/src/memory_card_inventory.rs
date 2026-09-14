//! Read-only PS1/PS2 memory-card inventory.
//!
//! The card image is always the shared preservation unit. Entries below are
//! observations only; this module never extracts, rewrites, repairs, or
//! attributes a complete card to one game.

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

pub const PS1_CARD_BYTES: usize = 128 * 1024;
pub const PS1_SLOT_COUNT: usize = 15;
pub const PS1_HEADER_BYTES: usize = 128;
pub const PS1_BLOCK_BYTES: usize = 8192;
pub const PS2_MAX_CARD_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_TITLE_BYTES: usize = 64;
pub const PS2_PAGE_DATA_BYTES: usize = 512;
pub const PS2_SPARE_BYTES: usize = 16;
pub const PS2_PHYSICAL_PAGE_BYTES: usize = PS2_PAGE_DATA_BYTES + PS2_SPARE_BYTES;
pub const PS2_SUPERBLOCK_BYTES: usize = 340;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Ps2PageRepresentation {
    RawDataOnly,
    RawWithSpare,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2Geometry {
    pub file_size_bytes: u64,
    pub page_stride_bytes: u32,
    pub page_data_bytes: u32,
    pub spare_bytes: u32,
    pub pages_per_cluster: u32,
    pub pages_per_block: u32,
    pub clusters_per_card: u32,
    pub alloc_offset: u32,
    pub alloc_end: u32,
    pub rootdir_cluster: u32,
    pub backup_block1: u32,
    pub backup_block2: u32,
    pub superblock_offset: u64,
    pub version: Option<String>,
    /// The two bytes at 0x150/0x151 are preserved without assigning the
    /// disputed card-type/flags names from conflicting documentation.
    pub marker_bytes: [u8; 2],
    pub representation: Ps2PageRepresentation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MemoryCardFormat {
    Ps1Raw,
    Ps2,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MemoryCardFormatConfidence {
    ConfirmedFormat,
    LikelyFormat,
    UnknownFormat,
    Malformed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MemoryCardHealth {
    Healthy,
    StructuralWarning,
    Malformed,
    Truncated,
    UnsupportedVariant,
    CorruptionSuspected,
    OutOfRangeMetadata,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MemoryCardIdentityConfidence {
    ConfirmedIdentity,
    SupportingEvidence,
    HeuristicOnly,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryCardEntryIdentity {
    pub product_code: Option<String>,
    pub display_title: Option<String>,
    pub confidence: MemoryCardIdentityConfidence,
    pub evidence: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryCardEntry {
    pub slot: Option<u32>,
    pub name: String,
    pub size_bytes: u64,
    pub used_blocks: Option<u32>,
    pub identity: MemoryCardEntryIdentity,
    pub deleted: bool,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryCardInventory {
    pub path: PathBuf,
    pub format: MemoryCardFormat,
    pub format_confidence: MemoryCardFormatConfidence,
    pub health: MemoryCardHealth,
    pub card_size_bytes: u64,
    pub entries: Vec<MemoryCardEntry>,
    pub used_blocks: Option<u32>,
    pub free_blocks: Option<u32>,
    pub warnings: Vec<String>,
    pub shared_container: bool,
    pub ps2_geometry: Option<Ps2Geometry>,
}

fn text(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    let value = String::from_utf8_lossy(&bytes[..end]).trim().to_string();
    (!value.is_empty()).then_some(value)
}
fn ps1_identity(header: &[u8], data: &[u8]) -> MemoryCardEntryIdentity {
    let product = text(header.get(12..22).unwrap_or_default());
    let title = text(data.get(4..4 + MAX_TITLE_BYTES).unwrap_or_default());
    let confidence = if product.as_deref().is_some_and(|v| v.contains('-')) {
        MemoryCardIdentityConfidence::ConfirmedIdentity
    } else if title.is_some() {
        MemoryCardIdentityConfidence::SupportingEvidence
    } else {
        MemoryCardIdentityConfidence::Unknown
    };
    let mut evidence = Vec::new();
    if product.is_some() {
        evidence.push("validated PS1 card product-code field".into());
    }
    if title.is_some() {
        evidence.push("PS1 save title field".into());
    }
    MemoryCardEntryIdentity {
        product_code: product,
        display_title: title,
        confidence,
        evidence,
    }
}

fn inspect_ps1(path: &Path, bytes: &[u8]) -> MemoryCardInventory {
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let mut used = 0u32;
    let mut damaged = false;
    for slot in 0..PS1_SLOT_COUNT {
        let offset = slot * PS1_BLOCK_BYTES;
        let header = &bytes[offset..offset + PS1_HEADER_BYTES];
        let kind = header[0];
        let valid = matches!(kind, 0xA0 | 0x51 | 0x52 | 0x53 | 0xA1 | 0xA2 | 0xA3);
        if !valid {
            if kind != 0 {
                damaged = true;
                warnings.push(format!("slot {slot} has unknown block marker 0x{kind:02x}"));
            }
            continue;
        }
        if matches!(kind, 0xA0 | 0x51) {
            used += 1;
            let data = &bytes[offset + PS1_HEADER_BYTES..offset + PS1_BLOCK_BYTES];
            let checksum = header[..127].iter().fold(0u8, |sum, value| sum ^ value);
            let mut entry_warnings = Vec::new();
            if checksum != header[127] {
                entry_warnings
                    .push("header XOR checksum does not match; bytes were not repaired".into());
                damaged = true;
            }
            entries.push(MemoryCardEntry {
                slot: Some(slot as u32),
                name: text(&header[10..30]).unwrap_or_else(|| format!("slot-{slot}")),
                size_bytes: PS1_BLOCK_BYTES as u64,
                used_blocks: Some(1),
                identity: ps1_identity(header, data),
                deleted: false,
                warnings: entry_warnings,
            });
        } else if matches!(kind, 0xA1 | 0xA2 | 0xA3) {
            entries.push(MemoryCardEntry {
                slot: Some(slot as u32),
                name: format!("deleted-slot-{slot}"),
                size_bytes: 0,
                used_blocks: Some(1),
                identity: MemoryCardEntryIdentity {
                    product_code: None,
                    display_title: None,
                    confidence: MemoryCardIdentityConfidence::Unknown,
                    evidence: Vec::new(),
                },
                deleted: true,
                warnings: vec![
                    "Deleted slot state is reported only; deleted data is not recovered.".into(),
                ],
            });
        }
    }
    MemoryCardInventory {
        path: path.into(),
        format: MemoryCardFormat::Ps1Raw,
        format_confidence: MemoryCardFormatConfidence::ConfirmedFormat,
        health: if damaged {
            MemoryCardHealth::CorruptionSuspected
        } else {
            MemoryCardHealth::Healthy
        },
        card_size_bytes: bytes.len() as u64,
        entries,
        used_blocks: Some(used),
        free_blocks: Some((PS1_SLOT_COUNT as u32).saturating_sub(used)),
        warnings,
        shared_container: true,
        ps2_geometry: None,
    }
}

fn looks_like_ps2(bytes: &[u8]) -> bool {
    bytes.get(..27) == Some(b"Sony PS2 Memory Card Format")
}

fn le_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn ps2_geometry(bytes: &[u8]) -> Result<Ps2Geometry, (MemoryCardHealth, String)> {
    if bytes.len() < PS2_SUPERBLOCK_BYTES {
        return Err((
            MemoryCardHealth::Truncated,
            "PS2 superblock is truncated".into(),
        ));
    }
    let page_len = le_u16(bytes, 0x28).ok_or((
        MemoryCardHealth::Truncated,
        "PS2 page length is unavailable".into(),
    ))? as usize;
    let pages_per_cluster = le_u16(bytes, 0x2a).ok_or((
        MemoryCardHealth::Truncated,
        "PS2 cluster geometry is unavailable".into(),
    ))? as usize;
    let pages_per_block = le_u16(bytes, 0x2c).ok_or((
        MemoryCardHealth::Truncated,
        "PS2 block geometry is unavailable".into(),
    ))? as usize;
    let clusters_per_card = le_u32(bytes, 0x30).ok_or((
        MemoryCardHealth::Truncated,
        "PS2 card geometry is unavailable".into(),
    ))? as u64;
    let alloc_offset = le_u32(bytes, 0x34).ok_or((
        MemoryCardHealth::Truncated,
        "PS2 allocation geometry is unavailable".into(),
    ))?;
    let alloc_end = le_u32(bytes, 0x38).ok_or((
        MemoryCardHealth::Truncated,
        "PS2 allocation geometry is unavailable".into(),
    ))?;
    let rootdir_cluster = le_u32(bytes, 0x3c).ok_or((
        MemoryCardHealth::Truncated,
        "PS2 root geometry is unavailable".into(),
    ))?;
    let backup_block1 = le_u32(bytes, 0x40).ok_or((
        MemoryCardHealth::Truncated,
        "PS2 backup geometry is unavailable".into(),
    ))?;
    let backup_block2 = le_u32(bytes, 0x44).ok_or((
        MemoryCardHealth::Truncated,
        "PS2 backup geometry is unavailable".into(),
    ))?;
    if page_len != PS2_PAGE_DATA_BYTES || pages_per_cluster == 0 || pages_per_block == 0 {
        return Err((
            MemoryCardHealth::UnsupportedVariant,
            "PS2 page/cluster/block geometry is unsupported".into(),
        ));
    }
    let cluster_data = pages_per_cluster.checked_mul(page_len).ok_or((
        MemoryCardHealth::OutOfRangeMetadata,
        "PS2 cluster-size arithmetic overflowed".into(),
    ))?;
    if cluster_data == 0 || cluster_data > PS2_MAX_CARD_BYTES {
        return Err((
            MemoryCardHealth::OutOfRangeMetadata,
            "PS2 cluster size is outside the inspection bound".into(),
        ));
    }
    if clusters_per_card == 0 || clusters_per_card > (PS2_MAX_CARD_BYTES / page_len) as u64 {
        return Err((
            MemoryCardHealth::OutOfRangeMetadata,
            "PS2 cluster count is outside the inspection bound".into(),
        ));
    }
    if alloc_offset > alloc_end
        || u64::from(alloc_end) > clusters_per_card
        || (rootdir_cluster != 0
            && (rootdir_cluster < alloc_offset || rootdir_cluster >= alloc_end))
    {
        return Err((
            MemoryCardHealth::OutOfRangeMetadata,
            "PS2 allocation or root metadata is out of range".into(),
        ));
    }
    let pages = clusters_per_card
        .checked_mul(pages_per_cluster as u64)
        .ok_or((
            MemoryCardHealth::OutOfRangeMetadata,
            "PS2 page-count arithmetic overflowed".into(),
        ))?;
    let raw_size = pages.checked_mul(PS2_PAGE_DATA_BYTES as u64).ok_or((
        MemoryCardHealth::OutOfRangeMetadata,
        "PS2 raw geometry arithmetic overflowed".into(),
    ))?;
    let physical_size = pages.checked_mul(PS2_PHYSICAL_PAGE_BYTES as u64).ok_or((
        MemoryCardHealth::OutOfRangeMetadata,
        "PS2 spare geometry arithmetic overflowed".into(),
    ))?;
    let (representation, stride) = if bytes.len() as u64 == physical_size {
        (Ps2PageRepresentation::RawWithSpare, PS2_PHYSICAL_PAGE_BYTES)
    } else if bytes.len() as u64 == raw_size {
        (Ps2PageRepresentation::RawDataOnly, PS2_PAGE_DATA_BYTES)
    } else if (bytes.len() as u64) < raw_size {
        return Err((
            MemoryCardHealth::Truncated,
            "PS2 declared geometry exceeds file length".into(),
        ));
    } else {
        return Err((
            MemoryCardHealth::UnsupportedVariant,
            "PS2 file size does not match a proven page representation".into(),
        ));
    };
    let version = text(&bytes[0x1c..0x28]);
    Ok(Ps2Geometry {
        file_size_bytes: bytes.len() as u64,
        page_stride_bytes: stride as u32,
        page_data_bytes: PS2_PAGE_DATA_BYTES as u32,
        spare_bytes: (stride - PS2_PAGE_DATA_BYTES) as u32,
        pages_per_cluster: pages_per_cluster as u32,
        pages_per_block: pages_per_block as u32,
        clusters_per_card: clusters_per_card as u32,
        alloc_offset,
        alloc_end,
        rootdir_cluster,
        backup_block1,
        backup_block2,
        superblock_offset: 0,
        version,
        marker_bytes: [bytes[0x150], bytes[0x151]],
        representation,
    })
}

pub fn inspect_memory_card(path: &Path) -> Result<MemoryCardInventory, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("memory card must be a regular non-symlink file".into());
    }
    if metadata.len() > PS2_MAX_CARD_BYTES as u64 {
        return Err("memory card exceeds inspection bound".into());
    }
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() == PS1_CARD_BYTES {
        return Ok(inspect_ps1(path, &bytes));
    }
    if looks_like_ps2(&bytes) {
        let geometry = ps2_geometry(&bytes);
        let (health, warning, geometry) = match geometry { Ok(geometry) => (MemoryCardHealth::Healthy, "PS2 geometry and superblock are structurally plausible; filesystem contents were not inspected.".into(), Some(geometry)), Err((health, warning)) => (health, warning, None) };
        return Ok(MemoryCardInventory {
            path: path.into(),
            format: MemoryCardFormat::Ps2,
            format_confidence: MemoryCardFormatConfidence::ConfirmedFormat,
            health,
            card_size_bytes: bytes.len() as u64,
            entries: Vec::new(),
            used_blocks: None,
            free_blocks: None,
            warnings: vec![warning],
            shared_container: true,
            ps2_geometry: geometry,
        });
    }
    Ok(MemoryCardInventory {
        path: path.into(),
        format: MemoryCardFormat::Unknown,
        format_confidence: if bytes.len() < PS1_CARD_BYTES {
            MemoryCardFormatConfidence::Malformed
        } else {
            MemoryCardFormatConfidence::UnknownFormat
        },
        health: if bytes.len() < PS1_CARD_BYTES {
            MemoryCardHealth::Truncated
        } else {
            MemoryCardHealth::Unknown
        },
        card_size_bytes: bytes.len() as u64,
        entries: Vec::new(),
        used_blocks: None,
        free_blocks: None,
        warnings: vec!["Size/structure did not prove a supported PS1 or PS2 card format.".into()],
        shared_container: true,
        ps2_geometry: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    fn fixture() -> Vec<u8> {
        let mut card = vec![0u8; PS1_CARD_BYTES];
        card[0] = 0xA0;
        card[10..26].copy_from_slice(b"BASLUS-00067SAVE");
        card[12..22].copy_from_slice(b"BASLUS-000");
        card[128 + 4..128 + 4 + 10].copy_from_slice(b"Test Save\0");
        card[127] = card[..127].iter().fold(0u8, |sum, value| sum ^ value);
        card
    }
    #[test]
    fn ps1_card_is_shared_and_identity_is_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("card.mcr");
        let before = fixture();
        fs::write(&path, &before).unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        assert!(inv.shared_container);
        assert_eq!(inv.entries.len(), 1);
        assert!(inv.entries[0].identity.product_code.is_some());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    #[test]
    fn truncated_and_unknown_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.bin");
        let mut file = File::create(&path).unwrap();
        file.write_all(b"bad").unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        assert_eq!(inv.health, MemoryCardHealth::Truncated);
        assert!(inv.entries.is_empty());
    }
    #[test]
    fn ps2_header_is_not_falsely_enumerated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("card.ps2");
        let mut bytes = vec![0u8; 8 * 1024 * 1024];
        bytes[..27].copy_from_slice(b"Sony PS2 Memory Card Format");
        fs::write(&path, &bytes).unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        assert_eq!(inv.format, MemoryCardFormat::Ps2);
        assert_eq!(inv.health, MemoryCardHealth::UnsupportedVariant);
        assert!(inv.entries.is_empty());
    }

    fn ps2_fixture(page_stride: usize) -> Vec<u8> {
        let clusters = 8192usize;
        let pages_per_cluster = 2usize;
        let mut card = vec![0u8; clusters * pages_per_cluster * page_stride];
        card[..28].copy_from_slice(b"Sony PS2 Memory Card Format ");
        card[0x1c..0x24].copy_from_slice(b"1.2.0.0");
        card[0x28..0x2a].copy_from_slice(&(PS2_PAGE_DATA_BYTES as u16).to_le_bytes());
        card[0x2a..0x2c].copy_from_slice(&(pages_per_cluster as u16).to_le_bytes());
        card[0x2c..0x2e].copy_from_slice(&(16u16).to_le_bytes());
        card[0x30..0x34].copy_from_slice(&(clusters as u32).to_le_bytes());
        card[0x34..0x38].copy_from_slice(&(41u32).to_le_bytes());
        card[0x38..0x3c].copy_from_slice(&(8135u32).to_le_bytes());
        card[0x3c..0x40].copy_from_slice(&(0u32).to_le_bytes());
        card[0x40..0x44].copy_from_slice(&(1023u32).to_le_bytes());
        card[0x44..0x48].copy_from_slice(&(1022u32).to_le_bytes());
        card
    }

    #[test]
    fn ps2_528_byte_geometry_is_structurally_recognized_without_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Mcd001.ps2");
        let bytes = ps2_fixture(PS2_PHYSICAL_PAGE_BYTES);
        fs::write(&path, &bytes).unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        assert_eq!(inv.health, MemoryCardHealth::Healthy);
        assert!(inv.entries.is_empty());
        let geometry = inv.ps2_geometry.unwrap();
        assert_eq!(geometry.page_stride_bytes, 528);
        assert_eq!(geometry.spare_bytes, 16);
        assert_eq!(geometry.alloc_offset, 41);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn ps2_impossible_geometry_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.ps2");
        let mut bytes = ps2_fixture(PS2_PHYSICAL_PAGE_BYTES);
        bytes[0x28..0x2a].copy_from_slice(&(513u16).to_le_bytes());
        fs::write(&path, &bytes).unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        assert_eq!(inv.health, MemoryCardHealth::UnsupportedVariant);
        assert!(inv.ps2_geometry.is_none());
    }
}
