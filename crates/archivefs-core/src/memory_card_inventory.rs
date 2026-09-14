//! Read-only PS1/PS2 memory-card inventory.
//!
//! The card image is always the shared preservation unit. Entries below are
//! observations only; this module never extracts, rewrites, repairs, or
//! attributes a complete card to one game.

use serde::Serialize;
use std::collections::HashSet;
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
pub const PS2_DIRECTORY_ENTRY_BYTES: usize = 512;
pub const PS2_NAME_BYTES: usize = 32;
pub const PS2_MAX_DIRECTORY_DEPTH: usize = 8;
pub const PS2_MAX_INVENTORY_ENTRIES: usize = 16_384;

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
pub enum Ps2DirectoryEntryKind {
    Directory,
    RegularFile,
    Special,
    Unused,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Ps2CorruptionKind {
    FatLoop,
    ClusterOutOfRange,
    InvalidFatReference,
    InvalidDirectoryEntry,
    DirectoryTooDeep,
    TooManyEntries,
    InvalidFilename,
    InvalidTimestamp,
    FileSizeExceedsChain,
    TruncatedCard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2InventoryWarning {
    pub kind: Ps2CorruptionKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2Timestamp {
    pub raw: [u8; 8],
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub timezone: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2DirectoryEntry {
    pub raw_entry_offset: u64,
    pub raw_mode: u32,
    pub kind: Ps2DirectoryEntryKind,
    pub raw_name: Vec<u8>,
    pub display_name: String,
    pub length: u32,
    pub start_cluster: u32,
    pub parent_entry: u32,
    pub attributes: u32,
    pub created: Option<Ps2Timestamp>,
    pub modified: Option<Ps2Timestamp>,
    pub warnings: Vec<Ps2InventoryWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2ClusterChainHealth {
    pub clusters: Vec<u32>,
    pub complete: bool,
    pub warnings: Vec<Ps2InventoryWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2SaveFile {
    pub entry: Ps2DirectoryEntry,
    pub declared_size_bytes: u64,
    pub chain_health: Ps2ClusterChainHealth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2SaveDirectory {
    pub entry: Ps2DirectoryEntry,
    pub chain_health: Ps2ClusterChainHealth,
    pub children: Vec<Ps2DirectoryEntry>,
    pub files: Vec<Ps2SaveFile>,
    pub warnings: Vec<Ps2InventoryWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2MemoryCardInventory {
    pub root_chain_health: Ps2ClusterChainHealth,
    pub root_entries: Vec<Ps2DirectoryEntry>,
    pub save_directories: Vec<Ps2SaveDirectory>,
    pub warnings: Vec<Ps2InventoryWarning>,
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
    pub ps2_inventory: Option<Ps2MemoryCardInventory>,
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
        ps2_inventory: None,
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

fn ps2_warning(kind: Ps2CorruptionKind, message: impl Into<String>) -> Ps2InventoryWarning {
    Ps2InventoryWarning {
        kind,
        message: message.into(),
    }
}

fn ps2_cluster_bytes(geometry: &Ps2Geometry) -> usize {
    geometry.pages_per_cluster as usize * geometry.page_data_bytes as usize
}

fn ps2_logical_cluster(
    bytes: &[u8],
    geometry: &Ps2Geometry,
    absolute_cluster: u32,
) -> Option<Vec<u8>> {
    if absolute_cluster >= geometry.clusters_per_card {
        return None;
    }
    let mut cluster = Vec::with_capacity(ps2_cluster_bytes(geometry));
    let page_data = geometry.page_data_bytes as usize;
    let stride = geometry.page_stride_bytes as usize;
    let first_page = absolute_cluster as usize * geometry.pages_per_cluster as usize;
    for page in 0..geometry.pages_per_cluster as usize {
        let offset = (first_page + page).checked_mul(stride)?;
        cluster.extend_from_slice(bytes.get(offset..offset + page_data)?);
    }
    Some(cluster)
}

fn ps2_relative_cluster(
    bytes: &[u8],
    geometry: &Ps2Geometry,
    relative_cluster: u32,
) -> Option<Vec<u8>> {
    let absolute = geometry.alloc_offset.checked_add(relative_cluster)?;
    if absolute >= geometry.alloc_end {
        return None;
    }
    ps2_logical_cluster(bytes, geometry, absolute)
}

fn ps2_fat_next(
    bytes: &[u8],
    geometry: &Ps2Geometry,
    relative_cluster: u32,
) -> Result<Option<u32>, Ps2InventoryWarning> {
    let available = geometry.alloc_end.saturating_sub(geometry.alloc_offset);
    if relative_cluster >= available {
        return Err(ps2_warning(
            Ps2CorruptionKind::ClusterOutOfRange,
            format!("relative cluster {relative_cluster} is outside the allocation area"),
        ));
    }
    let entries_per_cluster = ps2_cluster_bytes(geometry) / 4;
    if entries_per_cluster == 0 {
        return Err(ps2_warning(
            Ps2CorruptionKind::InvalidFatReference,
            "PS2 cluster has no room for FAT entries",
        ));
    }
    let indirect_index = relative_cluster as usize / entries_per_cluster;
    let indirect_slot = indirect_index / entries_per_cluster;
    let ifc_offset = 0x50usize
        .checked_add(indirect_slot.checked_mul(4).ok_or_else(|| {
            ps2_warning(
                Ps2CorruptionKind::InvalidFatReference,
                "IFC slot arithmetic overflowed",
            )
        })?)
        .ok_or_else(|| {
            ps2_warning(
                Ps2CorruptionKind::InvalidFatReference,
                "IFC offset arithmetic overflowed",
            )
        })?;
    let ifc_cluster = le_u32(bytes, ifc_offset).ok_or_else(|| {
        ps2_warning(
            Ps2CorruptionKind::TruncatedCard,
            "IFC entry is outside the card",
        )
    })?;
    let fat_cluster_index = indirect_index % entries_per_cluster;
    let fat_cluster = ps2_logical_cluster(bytes, geometry, ifc_cluster).ok_or_else(|| {
        ps2_warning(
            Ps2CorruptionKind::ClusterOutOfRange,
            format!("IFC points to unavailable FAT cluster {ifc_cluster}"),
        )
    })?;
    let fat_offset = fat_cluster_index.checked_mul(4).ok_or_else(|| {
        ps2_warning(
            Ps2CorruptionKind::InvalidFatReference,
            "FAT entry arithmetic overflowed",
        )
    })?;
    let raw = le_u32(&fat_cluster, fat_offset).ok_or_else(|| {
        ps2_warning(
            Ps2CorruptionKind::TruncatedCard,
            "FAT entry is outside the card",
        )
    })?;
    if raw == 0x7fff_ffff || raw == 0xffff_ffff {
        return Ok(None);
    }
    if raw & 0x8000_0000 == 0 {
        return Err(ps2_warning(
            Ps2CorruptionKind::InvalidFatReference,
            format!("FAT entry {raw:#x} is free or reserved, not a chain link"),
        ));
    }
    let next = raw & 0x7fff_ffff;
    if next >= available {
        return Err(ps2_warning(
            Ps2CorruptionKind::ClusterOutOfRange,
            format!("FAT points to relative cluster {next} outside the allocation area"),
        ));
    }
    Ok(Some(next))
}

fn ps2_chain(bytes: &[u8], geometry: &Ps2Geometry, start: u32) -> Ps2ClusterChainHealth {
    let available = geometry.alloc_end.saturating_sub(geometry.alloc_offset) as usize;
    if start >= available as u32 {
        return Ps2ClusterChainHealth {
            clusters: Vec::new(),
            complete: false,
            warnings: vec![ps2_warning(
                Ps2CorruptionKind::ClusterOutOfRange,
                format!("start cluster {start} is outside the allocation area"),
            )],
        };
    }
    let mut clusters = Vec::new();
    let mut visited = HashSet::new();
    let mut current = start;
    let mut warnings = Vec::new();
    while clusters.len() < available {
        if !visited.insert(current) {
            warnings.push(ps2_warning(
                Ps2CorruptionKind::FatLoop,
                format!("FAT chain loops at relative cluster {current}"),
            ));
            return Ps2ClusterChainHealth {
                clusters,
                complete: false,
                warnings,
            };
        }
        clusters.push(current);
        match ps2_fat_next(bytes, geometry, current) {
            Ok(Some(next)) => current = next,
            Ok(None) => {
                return Ps2ClusterChainHealth {
                    clusters,
                    complete: true,
                    warnings,
                };
            }
            Err(warning) => {
                warnings.push(warning);
                return Ps2ClusterChainHealth {
                    clusters,
                    complete: false,
                    warnings,
                };
            }
        }
    }
    warnings.push(ps2_warning(
        Ps2CorruptionKind::InvalidFatReference,
        "FAT chain exceeded the allocation-area cluster bound",
    ));
    Ps2ClusterChainHealth {
        clusters,
        complete: false,
        warnings,
    }
}

fn ps2_timestamp(raw: [u8; 8]) -> Result<Ps2Timestamp, Ps2InventoryWarning> {
    let timestamp = Ps2Timestamp {
        raw,
        year: u16::from_le_bytes([raw[6], raw[7]]),
        month: raw[5],
        day: raw[4],
        hour: raw[3],
        minute: raw[2],
        second: raw[1],
        timezone: "JST (UTC+09:00)",
    };
    if timestamp.year == 0
        || !(1..=12).contains(&timestamp.month)
        || timestamp.day == 0
        || timestamp.day > 31
        || timestamp.hour > 23
        || timestamp.minute > 59
        || timestamp.second > 59
    {
        return Err(ps2_warning(
            Ps2CorruptionKind::InvalidTimestamp,
            "timestamp fields are outside the validated PS2 range",
        ));
    }
    Ok(timestamp)
}

fn ps2_entry(raw: &[u8], raw_entry_offset: u64) -> Ps2DirectoryEntry {
    let raw_mode = le_u32(raw, 0).unwrap_or(0);
    let length = le_u32(raw, 4).unwrap_or(0);
    let start_cluster = le_u32(raw, 0x10).unwrap_or(u32::MAX);
    let parent_entry = le_u32(raw, 0x14).unwrap_or(0);
    let attributes = le_u32(raw, 0x20).unwrap_or(0);
    let raw_name = raw
        .get(0x40..0x40 + PS2_NAME_BYTES)
        .unwrap_or_default()
        .to_vec();
    let name_end = raw_name.iter().position(|byte| *byte == 0);
    let mut warnings = Vec::new();
    let name_bytes = match name_end {
        Some(end) => raw_name[..end].to_vec(),
        None => {
            warnings.push(ps2_warning(
                Ps2CorruptionKind::InvalidFilename,
                "name field has no NUL terminator within 32 bytes",
            ));
            raw_name.clone()
        }
    };
    let display_name = match String::from_utf8(name_bytes) {
        Ok(value) => value,
        Err(error) => {
            warnings.push(ps2_warning(
                Ps2CorruptionKind::InvalidFilename,
                "name bytes are not valid UTF-8; display uses replacement characters",
            ));
            String::from_utf8_lossy(error.as_bytes()).into_owned()
        }
    };
    let created_raw = raw
        .get(0x08..0x10)
        .and_then(|value| value.try_into().ok())
        .unwrap_or([0; 8]);
    let modified_raw = raw
        .get(0x18..0x20)
        .and_then(|value| value.try_into().ok())
        .unwrap_or([0; 8]);
    let created = match ps2_timestamp(created_raw) {
        Ok(value) => Some(value),
        Err(warning) => {
            if created_raw.iter().any(|byte| *byte != 0) {
                warnings.push(warning);
            }
            None
        }
    };
    let modified = match ps2_timestamp(modified_raw) {
        Ok(value) => Some(value),
        Err(warning) => {
            if modified_raw.iter().any(|byte| *byte != 0) {
                warnings.push(warning);
            }
            None
        }
    };
    let low_mode = raw_mode as u16;
    let used = low_mode & 0x8000 != 0;
    let is_directory = low_mode & 0x0020 != 0;
    let is_file = low_mode & 0x0010 != 0;
    let kind = if raw_mode == 0 || raw_mode == u32::MAX || !used {
        Ps2DirectoryEntryKind::Unused
    } else if is_directory && !is_file {
        Ps2DirectoryEntryKind::Directory
    } else if is_file && !is_directory {
        Ps2DirectoryEntryKind::RegularFile
    } else if is_directory || is_file {
        warnings.push(ps2_warning(
            Ps2CorruptionKind::InvalidDirectoryEntry,
            "directory and regular-file mode bits are contradictory",
        ));
        Ps2DirectoryEntryKind::Invalid
    } else {
        Ps2DirectoryEntryKind::Special
    };
    Ps2DirectoryEntry {
        raw_entry_offset,
        raw_mode,
        kind,
        raw_name,
        display_name,
        length,
        start_cluster,
        parent_entry,
        attributes,
        created,
        modified,
        warnings,
    }
}

fn ps2_read_directory(
    bytes: &[u8],
    geometry: &Ps2Geometry,
    start_cluster: u32,
    declared_entries: u32,
) -> (
    Ps2ClusterChainHealth,
    Vec<Ps2DirectoryEntry>,
    Vec<Ps2InventoryWarning>,
) {
    let mut warnings = Vec::new();
    let available = geometry.alloc_end.saturating_sub(geometry.alloc_offset) as usize;
    let max_entries = available
        .saturating_mul(ps2_cluster_bytes(geometry) / PS2_DIRECTORY_ENTRY_BYTES)
        .min(PS2_MAX_INVENTORY_ENTRIES);
    let requested = declared_entries as usize;
    if requested > max_entries {
        warnings.push(ps2_warning(
            Ps2CorruptionKind::TooManyEntries,
            format!("directory declares {declared_entries} entries; bounded at {max_entries}"),
        ));
    }
    let count = requested.min(max_entries);
    let chain = ps2_chain(bytes, geometry, start_cluster);
    let records_per_cluster = ps2_cluster_bytes(geometry) / PS2_DIRECTORY_ENTRY_BYTES;
    let needed_clusters = count.div_ceil(records_per_cluster.max(1));
    if chain.clusters.len() < needed_clusters {
        warnings.push(ps2_warning(
            Ps2CorruptionKind::TruncatedCard,
            format!(
                "directory needs {needed_clusters} clusters but its chain has {}",
                chain.clusters.len()
            ),
        ));
    }
    let mut entries = Vec::new();
    for index in 0..count {
        let cluster_index = index / records_per_cluster.max(1);
        let slot = index % records_per_cluster.max(1);
        let Some(&relative_cluster) = chain.clusters.get(cluster_index) else {
            break;
        };
        let Some(cluster) = ps2_relative_cluster(bytes, geometry, relative_cluster) else {
            warnings.push(ps2_warning(
                Ps2CorruptionKind::TruncatedCard,
                format!("directory cluster {relative_cluster} is outside the card"),
            ));
            break;
        };
        let offset = slot * PS2_DIRECTORY_ENTRY_BYTES;
        let Some(raw) = cluster.get(offset..offset + PS2_DIRECTORY_ENTRY_BYTES) else {
            warnings.push(ps2_warning(
                Ps2CorruptionKind::TruncatedCard,
                "directory entry is truncated",
            ));
            break;
        };
        let logical_offset = (geometry.alloc_offset as u64 + relative_cluster as u64)
            * ps2_cluster_bytes(geometry) as u64
            + (slot * PS2_DIRECTORY_ENTRY_BYTES) as u64;
        entries.push(ps2_entry(raw, logical_offset));
    }
    (chain, entries, warnings)
}

fn ps2_file_chain(
    bytes: &[u8],
    geometry: &Ps2Geometry,
    entry: &Ps2DirectoryEntry,
) -> Ps2ClusterChainHealth {
    if entry.length == 0 && entry.start_cluster == u32::MAX {
        return Ps2ClusterChainHealth {
            clusters: Vec::new(),
            complete: true,
            warnings: Vec::new(),
        };
    }
    if entry.length > 0 && entry.start_cluster == u32::MAX {
        return Ps2ClusterChainHealth {
            clusters: Vec::new(),
            complete: false,
            warnings: vec![ps2_warning(
                Ps2CorruptionKind::InvalidFatReference,
                "non-empty file uses the PS2 empty-file cluster marker",
            )],
        };
    }
    let mut chain = ps2_chain(bytes, geometry, entry.start_cluster);
    let cluster_bytes = ps2_cluster_bytes(geometry) as u64;
    let needed = (u64::from(entry.length) + cluster_bytes.saturating_sub(1)) / cluster_bytes;
    if (chain.clusters.len() as u64) < needed {
        chain.complete = false;
        chain.warnings.push(ps2_warning(
            Ps2CorruptionKind::FileSizeExceedsChain,
            format!(
                "file declares {} bytes but its chain has {} clusters",
                entry.length,
                chain.clusters.len()
            ),
        ));
    } else if (chain.clusters.len() as u64) > needed {
        chain.warnings.push(ps2_warning(
            Ps2CorruptionKind::FileSizeExceedsChain,
            format!(
                "file chain has {} clusters for a declared {}-byte payload requiring {needed}",
                chain.clusters.len(),
                entry.length
            ),
        ));
    }
    chain
}

fn ps2_save_directory(
    bytes: &[u8],
    geometry: &Ps2Geometry,
    entry: Ps2DirectoryEntry,
) -> Ps2SaveDirectory {
    let (chain_health, entries, mut warnings) =
        ps2_read_directory(bytes, geometry, entry.start_cluster, entry.length);
    warnings.extend(chain_health.warnings.clone());
    let mut children = Vec::new();
    let mut files = Vec::new();
    for child in entries {
        if child.display_name == "." || child.display_name == ".." {
            continue;
        }
        if child.kind == Ps2DirectoryEntryKind::RegularFile {
            let declared_size_bytes = u64::from(child.length);
            files.push(Ps2SaveFile {
                chain_health: ps2_file_chain(bytes, geometry, &child),
                entry: child,
                declared_size_bytes,
            });
        } else {
            if child.kind == Ps2DirectoryEntryKind::Directory {
                warnings.push(ps2_warning(
                    Ps2CorruptionKind::DirectoryTooDeep,
                    format!(
                        "nested directory {} was not descended beyond the bounded save inventory depth {}",
                        child.display_name, PS2_MAX_DIRECTORY_DEPTH
                    ),
                ));
            }
            children.push(child);
        }
    }
    Ps2SaveDirectory {
        entry,
        chain_health,
        children,
        files,
        warnings,
    }
}

fn inspect_ps2_filesystem(bytes: &[u8], geometry: &Ps2Geometry) -> Ps2MemoryCardInventory {
    let mut warnings = Vec::new();
    let root_chain = ps2_chain(bytes, geometry, geometry.rootdir_cluster);
    let root_count = ps2_relative_cluster(bytes, geometry, geometry.rootdir_cluster)
        .map(|cluster| {
            cluster
                .get(..PS2_DIRECTORY_ENTRY_BYTES)
                .and_then(|raw| le_u32(raw, 4))
                .unwrap_or(0)
        })
        .unwrap_or(0);
    let (root_chain_health, root_entries, root_warnings) =
        ps2_read_directory(bytes, geometry, geometry.rootdir_cluster, root_count);
    warnings.extend(root_warnings);
    warnings.extend(root_chain.warnings);
    let mut save_directories = Vec::new();
    for entry in &root_entries {
        if entry.kind != Ps2DirectoryEntryKind::Directory
            || entry.display_name == "."
            || entry.display_name == ".."
            || entry.display_name == "BEDATA-SYSTEM"
        {
            continue;
        }
        if save_directories.len() >= PS2_MAX_INVENTORY_ENTRIES {
            warnings.push(ps2_warning(
                Ps2CorruptionKind::TooManyEntries,
                "save-directory inventory bound reached",
            ));
            break;
        }
        save_directories.push(ps2_save_directory(bytes, geometry, entry.clone()));
    }
    Ps2MemoryCardInventory {
        root_chain_health,
        root_entries,
        save_directories,
        warnings,
    }
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
        let (mut health, warning, geometry) = match geometry {
            Ok(geometry) => (
                MemoryCardHealth::Healthy,
                "PS2 geometry and superblock are structurally plausible; filesystem contents were inspected read-only."
                    .into(),
                Some(geometry),
            ),
            Err((health, warning)) => (health, warning, None),
        };
        let ps2_inventory = geometry
            .as_ref()
            .map(|geometry| inspect_ps2_filesystem(&bytes, geometry));
        if let Some(inventory) = &ps2_inventory {
            let entry_warning = inventory
                .root_entries
                .iter()
                .any(|entry| !entry.warnings.is_empty())
                || inventory.save_directories.iter().any(|directory| {
                    !directory.warnings.is_empty()
                        || directory
                            .children
                            .iter()
                            .any(|entry| !entry.warnings.is_empty())
                        || directory
                            .files
                            .iter()
                            .any(|file| !file.chain_health.warnings.is_empty())
                });
            if !inventory.warnings.is_empty() || entry_warning {
                health = MemoryCardHealth::StructuralWarning;
            }
        }
        return Ok(MemoryCardInventory {
            path: path.into(),
            format: MemoryCardFormat::Ps2,
            format_confidence: MemoryCardFormatConfidence::ConfirmedFormat,
            health,
            card_size_bytes: bytes.len() as u64,
            entries: Vec::new(),
            used_blocks: None,
            free_blocks: None,
            warnings: {
                let mut warnings = vec![warning];
                if let Some(inventory) = &ps2_inventory {
                    warnings.extend(
                        inventory
                            .warnings
                            .iter()
                            .map(|warning| warning.message.clone()),
                    );
                }
                warnings
            },
            shared_container: true,
            ps2_geometry: geometry,
            ps2_inventory,
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
        ps2_inventory: None,
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

    fn ps2_inventory_fixture() -> Vec<u8> {
        let mut card = ps2_fixture(PS2_PAGE_DATA_BYTES);
        card[0x50..0x54].copy_from_slice(&8u32.to_le_bytes());
        card[8 * 1024..8 * 1024 + 4].copy_from_slice(&9u32.to_le_bytes());
        for relative in [0usize, 2, 3] {
            let offset = 9 * 1024 + relative * 4;
            card[offset..offset + 4].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        }

        fn entry(
            card: &mut [u8],
            relative: usize,
            slot: usize,
            mode: u32,
            length: u32,
            cluster: u32,
            name: &[u8],
        ) {
            let offset = (41 + relative) * 1024 + slot * PS2_DIRECTORY_ENTRY_BYTES;
            card[offset..offset + 4].copy_from_slice(&mode.to_le_bytes());
            card[offset + 4..offset + 8].copy_from_slice(&length.to_le_bytes());
            card[offset + 0x08..offset + 0x10].copy_from_slice(&[0, 38, 44, 12, 24, 6, 0xea, 0x07]);
            card[offset + 0x10..offset + 0x14].copy_from_slice(&cluster.to_le_bytes());
            card[offset + 0x18..offset + 0x20].copy_from_slice(&[0, 39, 44, 12, 24, 6, 0xea, 0x07]);
            card[offset + 0x40..offset + 0x40 + name.len()].copy_from_slice(name);
        }

        entry(&mut card, 0, 0, 0x8427, 3, 0, b".");
        entry(&mut card, 0, 1, 0xa426, 0, 0, b"..");
        entry(&mut card, 0, 2, 0x8427, 3, 2, b"BASLUS-00001TEST");
        entry(&mut card, 2, 0, 0x8427, 3, 2, b".");
        entry(&mut card, 2, 1, 0xa426, 0, 0, b"..");
        entry(&mut card, 2, 2, 0x8497, 100, 3, b"icon.sys");
        card[3 * 1024..3 * 1024 + 100].fill(0x5a);
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

    #[test]
    fn ps2_inventory_walks_root_save_and_file_without_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Mcd001.ps2");
        let before = ps2_inventory_fixture();
        fs::write(&path, &before).unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        let ps2 = inv.ps2_inventory.unwrap();
        assert!(ps2.root_chain_health.complete);
        assert_eq!(ps2.root_entries.len(), 3);
        assert_eq!(ps2.save_directories.len(), 1);
        let save = &ps2.save_directories[0];
        assert_eq!(save.entry.display_name, "BASLUS-00001TEST");
        assert_eq!(save.files.len(), 1);
        assert_eq!(save.files[0].entry.display_name, "icon.sys");
        assert_eq!(save.files[0].declared_size_bytes, 100);
        assert!(save.files[0].chain_health.complete);
        assert_eq!(save.files[0].chain_health.clusters, vec![3]);
        assert_eq!(save.files[0].entry.created.as_ref().unwrap().year, 2026);
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn ps2_inventory_reports_fat_loop_and_oversized_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("loop.ps2");
        let mut bytes = ps2_inventory_fixture();
        bytes[9 * 1024 + 3 * 4..9 * 1024 + 4 * 4].copy_from_slice(&0x8000_0003u32.to_le_bytes());
        fs::write(&path, bytes).unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        let ps2 = inv.ps2_inventory.unwrap();
        let file = &ps2.save_directories[0].files[0];
        assert!(
            file.chain_health
                .warnings
                .iter()
                .any(|warning| warning.kind == Ps2CorruptionKind::FatLoop)
        );

        let mut bytes = ps2_inventory_fixture();
        let file_entry_offset = (41 + 2) * 1024 + 2 * PS2_DIRECTORY_ENTRY_BYTES;
        bytes[file_entry_offset + 4..file_entry_offset + 8].copy_from_slice(&2000u32.to_le_bytes());
        fs::write(&path, bytes).unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        let file = &inv.ps2_inventory.unwrap().save_directories[0].files[0];
        assert!(
            file.chain_health
                .warnings
                .iter()
                .any(|warning| warning.kind == Ps2CorruptionKind::FileSizeExceedsChain)
        );
    }

    #[test]
    fn ps2_inventory_keeps_raw_name_and_reports_invalid_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metadata.ps2");
        let mut bytes = ps2_inventory_fixture();
        let offset = 41 * 1024 + 2 * PS2_DIRECTORY_ENTRY_BYTES;
        bytes[offset + 0x08..offset + 0x10].copy_from_slice(&[0, 0, 0, 25, 0, 13, 0, 0]);
        bytes[offset + 0x40..offset + 0x40 + PS2_NAME_BYTES]
            .copy_from_slice(&[0xff; PS2_NAME_BYTES]);
        fs::write(&path, bytes).unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        let root = &inv.ps2_inventory.unwrap().root_entries[2];
        assert_eq!(root.raw_name, vec![0xff; PS2_NAME_BYTES]);
        assert!(
            root.warnings
                .iter()
                .any(|warning| warning.kind == Ps2CorruptionKind::InvalidFilename)
        );
        assert!(
            root.warnings
                .iter()
                .any(|warning| warning.kind == Ps2CorruptionKind::InvalidTimestamp)
        );
    }
}
