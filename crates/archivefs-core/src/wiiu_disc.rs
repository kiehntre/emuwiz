//! Bounded, key-free WUD structure and native WUX sector-map inspection.
//! See docs/research/WIIU_WUD_WUX_PRESERVATION.md for evidence and limits.

use sha2::{Digest, Sha256};
use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub(crate) const WUX_HEADER: u64 = 32;
pub const WUD_SECTOR_SIZE: u32 = 0x8000;
pub const WUD_HEADER_SIZE: u64 = 0x20000;
pub const RETAIL_WUD_SIZE: u64 = 0x5d3a00000;
pub const MAX_LOGICAL_SIZE: u64 = 64 * 1024 * 1024 * 1024;
/// 16 MiB table plus at most 4 MiB reference bitmap. Retail images with
/// 32 KiB sectors need about 3 MiB of table; validate before allocation.
pub const MAX_WUX_TABLE_BYTES: u64 = 16 * 1024 * 1024;
const DISC_MAGIC: u32 = 0xcc54_9eb9;
const CONTENTS_MAGIC: u32 = 0xcca6_e67b;
const MAGIC1: u32 = 0x1099_d02e;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WiiUPartitionEvidence {
    /// Region exists; absence of plaintext magic does not prove encryption.
    EncryptedOrOpaque,
    Plaintext {
        block_size: u32,
        partitions: Vec<WiiUPartitionFact>,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WiiUPartitionFact {
    pub volume_id: Option<String>,
    pub volume_offsets: Vec<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WiiUHeaderEvidence {
    /// Printable manufacturer WUP identifier, never a decrypted title ID.
    pub manufacturer_disc_id: Option<String>,
    pub disc_magic: u32,
    pub major_version: u8,
    pub minor_version: u8,
    pub footprint: Option<String>,
    pub partition_table: WiiUPartitionEvidence,
    pub header_sha256: [u8; 32],
}
/// Bounded preview binding; the digest covers inspected header/table evidence,
/// not the entire container. Apply separately captures full-file SHA-256.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WiiUDiscEvidence {
    pub size_bytes: u64,
    pub modified: SystemTime,
    #[cfg(unix)]
    pub dev: u64,
    #[cfg(unix)]
    pub ino: u64,
    #[cfg(unix)]
    pub changed: (i64, i64),
    pub structure_sha256: [u8; 32],
}
impl WiiUDiscEvidence {
    fn from_metadata(m: &Metadata, digest: [u8; 32]) -> std::io::Result<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            size_bytes: m.len(),
            modified: m.modified()?,
            #[cfg(unix)]
            dev: m.dev(),
            #[cfg(unix)]
            ino: m.ino(),
            #[cfg(unix)]
            changed: (m.ctime(), m.ctime_nsec()),
            structure_sha256: digest,
        })
    }
    pub(crate) fn matches_metadata(&self, m: &Metadata) -> bool {
        m.is_file()
            && !m.file_type().is_symlink()
            && Self::from_metadata(m, self.structure_sha256).ok().as_ref() == Some(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WiiUDiscFormat {
    Wud,
    Wux,
    Wua,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WiiUDiscKeyState {
    NotRequiredForContainerInspection,
    RequiredForDeeperInspection,
    AvailableLocally,
    Missing,
    Invalid,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WiiUDiscReadiness {
    ReadyForContainerInspection,
    StructurallyComplete,
    StructurallyIncomplete,
    RequiresKeysForDeeperInspection,
    UnsupportedRepresentation,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WiiUDiscIssue {
    EmptyInput,
    TruncatedHeader,
    InvalidMagic,
    InvalidSectorSize(u32),
    LogicalSizeOverflow,
    AbsurdBlockCount(u64),
    TruncatedIndexTable { expected: u64, available: u64 },
    BlockOutsideContainer { index: u32, offset: u64 },
    InvalidBlockMapping { index: u32 },
    WuaIsSeparateFormat,
    RawWudInnerStructureUnavailable,
    SplitMissingPart { index: u32 },
    SplitDuplicatePart { index: u32 },
    SplitAmbiguousSequence,
    InvalidLogicalSize(u64),
    TruncatedBody { minimum: u64, available: u64 },
    NonRetailDiscSize { expected: u64, actual: u64 },
    UnsupportedFlags(u32),
    InvalidPayloadGeometry,
    InvalidPartitionTable,
    SourceChanged,
    Io(String),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WiiUDiscPart {
    pub path: PathBuf,
    pub index: Option<u32>,
    pub size_bytes: u64,
    pub required: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WiiUDiscStructure {
    pub detected_format: WiiUDiscFormat,
    pub total_size_bytes: u64,
    pub logical_disc_size_bytes: Option<u64>,
    pub physical_container_size_bytes: u64,
    pub compressed: bool,
    pub sector_size_bytes: Option<u32>,
    pub block_count: Option<u64>,
    pub index_table_bytes: Option<u64>,
    pub sector_array_offset: Option<u64>,
    pub referenced_block_count: Option<u64>,
    pub repeated_block_count: Option<u64>,
    pub wud_header: Option<WiiUHeaderEvidence>,
    pub retail_size_matches: bool,
    pub flags: Option<u32>,
    pub parts: Vec<WiiUDiscPart>,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WiiUDiscInspection {
    pub source: PathBuf,
    pub format: WiiUDiscFormat,
    pub structure: Option<WiiUDiscStructure>,
    pub issues: Vec<WiiUDiscIssue>,
    /// Inspected extents are valid; encrypted content and original dump
    /// completeness are not authenticated. See `retail_size_matches`.
    pub structural_complete: bool,
    pub key_state: WiiUDiscKeyState,
    pub readiness: WiiUDiscReadiness,
    pub provenance: String,
    pub source_evidence: Option<WiiUDiscEvidence>,
}

pub fn inspect_wii_u_disc(path: &Path) -> WiiUDiscInspection {
    let mut report = WiiUDiscInspection {
        source: path.into(),
        format: WiiUDiscFormat::Unknown,
        structure: None,
        issues: Vec::new(),
        structural_complete: false,
        key_state: WiiUDiscKeyState::Unknown,
        readiness: WiiUDiscReadiness::StructurallyIncomplete,
        provenance: "native bounded WUD header / WUX sector-map evidence v1; no keys".into(),
        source_evidence: None,
    };
    if let Err(issue) = inspect_into(path, &mut report) {
        report.issues.push(issue);
    }
    report
}
fn io_issue(e: std::io::Error) -> WiiUDiscIssue {
    WiiUDiscIssue::Io(e.to_string())
}

fn inspect_into(path: &Path, report: &mut WiiUDiscInspection) -> Result<(), WiiUDiscIssue> {
    let mut file =
        crate::safe_read::open_bounded_read(path, &crate::safe_read::TrustedRoots::none())
            .map_err(|e| WiiUDiscIssue::Io(format!("{e:?}")))?
            .into_file();
    let before = file.metadata().map_err(io_issue)?;
    if before.len() == 0 {
        return Err(WiiUDiscIssue::EmptyInput);
    }
    let mut header = [0_u8; WUX_HEADER as usize];
    let count = usize::try_from(before.len().min(WUX_HEADER)).unwrap();
    file.read_exact(&mut header[..count]).map_err(io_issue)?;
    let ext = path
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or_default();
    let is_wux = header[..4] == *b"WUX0";
    let mut digest = Sha256::new();
    let mut structure = WiiUDiscStructure {
        detected_format: WiiUDiscFormat::Unknown,
        total_size_bytes: before.len(),
        logical_disc_size_bytes: Some(before.len()),
        physical_container_size_bytes: before.len(),
        compressed: is_wux,
        sector_size_bytes: Some(WUD_SECTOR_SIZE),
        block_count: Some(before.len() / u64::from(WUD_SECTOR_SIZE)),
        index_table_bytes: None,
        sector_array_offset: None,
        referenced_block_count: None,
        repeated_block_count: None,
        flags: None,
        parts: vec![WiiUDiscPart {
            path: path.into(),
            index: None,
            size_bytes: before.len(),
            required: true,
        }],
        wud_header: None,
        retail_size_matches: false,
    };
    let mut wud_header = vec![0_u8; WUD_HEADER_SIZE as usize];
    if is_wux {
        report.format = WiiUDiscFormat::Wux;
        structure.detected_format = report.format;
        if count == WUX_HEADER as usize {
            structure.logical_disc_size_bytes =
                Some(u64::from_le_bytes(header[16..24].try_into().unwrap()));
            structure.sector_size_bytes =
                Some(u32::from_le_bytes(header[8..12].try_into().unwrap()));
            structure.flags = Some(u32::from_le_bytes(header[24..28].try_into().unwrap()));
        }
        report.structure = Some(structure.clone());
        let layout = load_wux(&mut file, before.len())?;
        structure.logical_disc_size_bytes = Some(layout.logical);
        structure.sector_size_bytes = Some(layout.sector);
        structure.block_count = Some(layout.table.len() as u64);
        structure.index_table_bytes = Some(layout.table.len() as u64 * 4);
        structure.sector_array_offset = Some(layout.data_offset);
        structure.referenced_block_count = Some(layout.unique);
        structure.repeated_block_count = Some(layout.table.len() as u64 - layout.unique);
        structure.flags = Some(0);
        digest.update(layout.metadata_sha256);
        read_wux_at(&mut file, &layout, 0, &mut wud_header)?;
    } else {
        // Extensions may select diagnostics, never grant positive detection.
        let (parts, issues) = split_parts(path, before.len());
        structure.parts = parts;
        report.issues.extend(issues);
        if structure.parts.iter().any(|p| p.index.is_some()) {
            // ponytail: split inventory only; add a bound multi-part reader before claiming structure.
            report.issues.push(WiiUDiscIssue::SplitAmbiguousSequence);
            report.structure = Some(structure);
            return Ok(());
        }
        if before.len() < 0x10004 {
            if ext.eq_ignore_ascii_case("wua") {
                report.format = WiiUDiscFormat::Wua;
                report.readiness = WiiUDiscReadiness::UnsupportedRepresentation;
                return Err(WiiUDiscIssue::WuaIsSeparateFormat);
            }
            return Err(
                if count < WUX_HEADER as usize || header.starts_with(b"WUP-") {
                    WiiUDiscIssue::TruncatedHeader
                } else {
                    WiiUDiscIssue::InvalidMagic
                },
            );
        }
        let mut magic = [0_u8; 4];
        file.seek(SeekFrom::Start(0x10000)).map_err(io_issue)?;
        file.read_exact(&mut magic).map_err(io_issue)?;
        if u32::from_be_bytes(magic) != DISC_MAGIC {
            return Err(WiiUDiscIssue::InvalidMagic);
        }
        report.format = WiiUDiscFormat::Wud;
        structure.detected_format = report.format;
        report.structure = Some(structure.clone());
        validate_logical_size(before.len())?;
        file.seek(SeekFrom::Start(0)).map_err(io_issue)?;
        file.read_exact(&mut wud_header).map_err(io_issue)?;
    }
    let logical = structure.logical_disc_size_bytes.unwrap();
    structure.wud_header = Some(parse_wud_header(&wud_header, logical)?);
    digest.update(&wud_header);
    let evidence =
        WiiUDiscEvidence::from_metadata(&before, digest.finalize().into()).map_err(io_issue)?;
    if !evidence.matches_metadata(&file.metadata().map_err(io_issue)?)
        || !evidence.matches_metadata(&std::fs::symlink_metadata(path).map_err(io_issue)?)
    {
        return Err(WiiUDiscIssue::SourceChanged);
    }
    structure.retail_size_matches = logical == RETAIL_WUD_SIZE;
    if !structure.retail_size_matches {
        report.issues.push(WiiUDiscIssue::NonRetailDiscSize {
            expected: RETAIL_WUD_SIZE,
            actual: logical,
        });
    }
    if matches!(
        structure.wud_header.as_ref().unwrap().partition_table,
        WiiUPartitionEvidence::EncryptedOrOpaque
    ) {
        report
            .issues
            .push(WiiUDiscIssue::RawWudInnerStructureUnavailable);
    }
    report.structure = Some(structure);
    report.source_evidence = Some(evidence);
    report.structural_complete = true;
    report.key_state = WiiUDiscKeyState::NotRequiredForContainerInspection;
    report.readiness = WiiUDiscReadiness::RequiresKeysForDeeperInspection;
    Ok(())
}
fn validate_logical_size(size: u64) -> Result<(), WiiUDiscIssue> {
    if size < WUD_HEADER_SIZE {
        return Err(WiiUDiscIssue::TruncatedBody {
            minimum: WUD_HEADER_SIZE,
            available: size,
        });
    }
    if size > MAX_LOGICAL_SIZE || !size.is_multiple_of(u64::from(WUD_SECTOR_SIZE)) {
        return Err(WiiUDiscIssue::InvalidLogicalSize(size));
    }
    Ok(())
}
fn text_field(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|b| *b == 0)?;
    let value = &bytes[..end];
    (!value.is_empty() && value.iter().all(|b| (0x20..=0x7e).contains(b)))
        .then(|| String::from_utf8(value.to_vec()).unwrap())
}
fn parse_wud_header(h: &[u8], logical: u64) -> Result<WiiUHeaderEvidence, WiiUDiscIssue> {
    let be = |offset| u32::from_be_bytes(h[offset..offset + 4].try_into().unwrap());
    if be(0x10000) != DISC_MAGIC {
        return Err(WiiUDiscIssue::InvalidMagic);
    }
    let partition_table = if be(0x18000) != CONTENTS_MAGIC {
        WiiUPartitionEvidence::EncryptedOrOpaque
    } else {
        let block_size = be(0x18004);
        let count = be(0x1801c) as usize;
        if block_size == 0 || count > (0x8000 - 0x800) / 128 {
            return Err(WiiUDiscIssue::InvalidPartitionTable);
        }
        let mut partitions = Vec::with_capacity(count);
        for i in 0..count {
            let base = 0x18800 + i * 128;
            let volumes = h[base + 31] as usize;
            if volumes > 8 {
                return Err(WiiUDiscIssue::InvalidPartitionTable);
            }
            let mut volume_offsets = Vec::with_capacity(volumes);
            for v in 0..volumes {
                let offset = u64::from(be(base + 32 + v * 4))
                    .checked_mul(u64::from(block_size))
                    .ok_or(WiiUDiscIssue::LogicalSizeOverflow)?;
                if offset < WUD_HEADER_SIZE || offset >= logical {
                    return Err(WiiUDiscIssue::InvalidPartitionTable);
                }
                volume_offsets.push(offset);
            }
            partitions.push(WiiUPartitionFact {
                volume_id: text_field(&h[base..base + 31]),
                volume_offsets,
            });
        }
        WiiUPartitionEvidence::Plaintext {
            block_size,
            partitions,
        }
    };
    Ok(WiiUHeaderEvidence {
        manufacturer_disc_id: text_field(&h[..32]).filter(|s| s.starts_with("WUP-")),
        disc_magic: DISC_MAGIC,
        major_version: h[0x10005],
        minor_version: h[0x10006],
        footprint: text_field(&h[0x10020..0x10120]),
        partition_table,
        header_sha256: Sha256::digest(h).into(),
    })
}
pub(crate) struct WuxLayout {
    pub sector: u32,
    pub logical: u64,
    pub physical: u64,
    pub data_offset: u64,
    pub table: Vec<u32>,
    pub unique: u64,
    metadata_sha256: [u8; 32],
}
pub(crate) fn checked_align(value: u64, alignment: u64) -> Option<u64> {
    if alignment == 0 {
        return None;
    }
    value
        .checked_add(alignment - 1)?
        .checked_div(alignment)?
        .checked_mul(alignment)
}
pub(crate) fn load_wux(file: &mut File, physical: u64) -> Result<WuxLayout, WiiUDiscIssue> {
    if physical < WUX_HEADER {
        return Err(WiiUDiscIssue::TruncatedHeader);
    }
    let mut h = [0_u8; WUX_HEADER as usize];
    file.seek(SeekFrom::Start(0)).map_err(io_issue)?;
    file.read_exact(&mut h).map_err(io_issue)?;
    if h[..4] != *b"WUX0" || u32::from_le_bytes(h[4..8].try_into().unwrap()) != MAGIC1 {
        return Err(WiiUDiscIssue::InvalidMagic);
    }
    let sector = u32::from_le_bytes(h[8..12].try_into().unwrap());
    if !(0x100..0x1000_0000).contains(&sector) {
        return Err(WiiUDiscIssue::InvalidSectorSize(sector));
    }
    let logical = u64::from_le_bytes(h[16..24].try_into().unwrap());
    let count = logical
        .checked_add(u64::from(sector) - 1)
        .and_then(|n| n.checked_div(u64::from(sector)))
        .ok_or(WiiUDiscIssue::LogicalSizeOverflow)?;
    validate_logical_size(logical)?;
    let table_bytes = count
        .checked_mul(4)
        .ok_or(WiiUDiscIssue::LogicalSizeOverflow)?;
    if table_bytes > MAX_WUX_TABLE_BYTES {
        return Err(WiiUDiscIssue::AbsurdBlockCount(count));
    }
    let flags = u32::from_le_bytes(h[24..28].try_into().unwrap());
    if flags != 0 {
        return Err(WiiUDiscIssue::UnsupportedFlags(flags));
    }
    let table_end = WUX_HEADER
        .checked_add(table_bytes)
        .ok_or(WiiUDiscIssue::LogicalSizeOverflow)?;
    let data_offset =
        checked_align(table_end, u64::from(sector)).ok_or(WiiUDiscIssue::LogicalSizeOverflow)?;
    if data_offset > physical {
        return Err(WiiUDiscIssue::TruncatedIndexTable {
            expected: data_offset,
            available: physical,
        });
    }
    let payload = physical - data_offset;
    let stored = payload / u64::from(sector);
    if !payload.is_multiple_of(u64::from(sector)) || stored > count || stored == 0 {
        return Err(WiiUDiscIssue::InvalidPayloadGeometry);
    }
    let mut table = Vec::with_capacity(count as usize);
    let mut seen = vec![false; stored as usize];
    let mut unique = 0;
    let mut digest = Sha256::new();
    digest.update(h);
    let mut entries = std::io::BufReader::new(file);
    for _ in 0..count {
        let mut entry = [0_u8; 4];
        entries.read_exact(&mut entry).map_err(io_issue)?;
        digest.update(entry);
        let index = u32::from_le_bytes(entry);
        let offset = u64::from(index)
            .checked_mul(u64::from(sector))
            .and_then(|n| data_offset.checked_add(n))
            .ok_or(WiiUDiscIssue::InvalidBlockMapping { index })?;
        if offset
            .checked_add(u64::from(sector))
            .is_none_or(|end| end > physical)
        {
            return Err(WiiUDiscIssue::BlockOutsideContainer { index, offset });
        }
        if !seen[index as usize] {
            unique += 1;
            seen[index as usize] = true;
        }
        table.push(index);
    }
    Ok(WuxLayout {
        sector,
        logical,
        physical,
        data_offset,
        table,
        unique,
        metadata_sha256: digest.finalize().into(),
    })
}
/// Reads a bounded logical range. Zero sectors are ordinary stored sectors;
/// WUX defines no sparse sentinel: neither zero nor UINT32_MAX means a hole.
pub(crate) fn read_wux_at(
    file: &mut File,
    layout: &WuxLayout,
    mut offset: u64,
    mut out: &mut [u8],
) -> Result<(), WiiUDiscIssue> {
    if offset
        .checked_add(out.len() as u64)
        .is_none_or(|end| end > layout.logical)
    {
        return Err(WiiUDiscIssue::InvalidLogicalSize(offset));
    }
    while !out.is_empty() {
        let sector = u64::from(layout.sector);
        let within = offset % sector;
        let index = *layout
            .table
            .get((offset / sector) as usize)
            .ok_or(WiiUDiscIssue::InvalidLogicalSize(offset))?;
        let physical = u64::from(index)
            .checked_mul(sector)
            .and_then(|n| layout.data_offset.checked_add(n))
            .and_then(|n| n.checked_add(within))
            .ok_or(WiiUDiscIssue::InvalidBlockMapping { index })?;
        let length = (sector - within).min(out.len() as u64) as usize;
        if physical
            .checked_add(length as u64)
            .is_none_or(|end| end > layout.physical)
        {
            return Err(WiiUDiscIssue::BlockOutsideContainer {
                index,
                offset: physical,
            });
        }
        file.seek(SeekFrom::Start(physical)).map_err(io_issue)?;
        file.read_exact(&mut out[..length]).map_err(io_issue)?;
        out = &mut out[length..];
        offset = offset
            .checked_add(length as u64)
            .ok_or(WiiUDiscIssue::LogicalSizeOverflow)?;
    }
    Ok(())
}
fn split_parts(path: &Path, size: u64) -> (Vec<WiiUDiscPart>, Vec<WiiUDiscIssue>) {
    let Some((prefix, current)) = split_name(path) else {
        return (
            vec![WiiUDiscPart {
                path: path.to_path_buf(),
                index: None,
                size_bytes: size,
                required: true,
            }],
            Vec::new(),
        );
    };
    let mut found = Vec::new();
    if let Some(parent) = path.parent()
        && let Ok(entries) = std::fs::read_dir(parent)
    {
        for (scanned, entry) in entries.flatten().enumerate() {
            if scanned >= 4096 {
                break;
            }
            let candidate = entry.path();
            if let Some((p, i)) = split_name(&candidate)
                && p == prefix
                && (1..=64).contains(&i)
                && candidate
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("wud"))
                && let Ok(m) = std::fs::symlink_metadata(&candidate)
                && m.is_file()
            {
                found.push((i, candidate, m.len()));
            }
        }
    }
    found.sort_by_key(|v| (v.0, v.1.clone()));
    let max = found.iter().map(|v| v.0).max().unwrap_or(current).min(64);
    let mut parts = Vec::new();
    let mut issues = Vec::new();
    for i in 1..=max {
        let matches: Vec<_> = found.iter().filter(|v| v.0 == i).collect();
        if matches.is_empty() {
            issues.push(WiiUDiscIssue::SplitMissingPart { index: i });
            continue;
        }
        if matches.len() > 1 {
            issues.push(WiiUDiscIssue::SplitDuplicatePart { index: i });
        }
        let (_, p, s) = matches[0];
        parts.push(WiiUDiscPart {
            path: p.clone(),
            index: Some(i),
            size_bytes: *s,
            required: true,
        });
    }
    if parts.is_empty() {
        parts.push(WiiUDiscPart {
            path: path.to_path_buf(),
            index: Some(current),
            size_bytes: size,
            required: true,
        });
    }
    (parts, issues)
}
fn split_name(path: &Path) -> Option<(String, u32)> {
    let stem = path.file_stem()?.to_str()?.to_ascii_lowercase();
    for marker in ["_part", ".part", "-part"] {
        if let Some(pos) = stem.rfind(marker) {
            let digits = &stem[pos + marker.len()..];
            if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                return Some((stem[..pos].into(), digits.parse().ok()?));
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "wiiu_preservation_tests.rs"]
pub(crate) mod tests;
