//! Read-only bounded Amiga HDF/RDB container inspection. No filesystem traversal.
use crate::platform_evidence_fusion::evidence_lineage::{
    ClaimStrength, ClaimType, EvidenceChannel, EvidenceObservation, IdentityScope, LineageRelation,
    Provenance, Representation, SourceFamily,
};
use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const RDSK: &[u8; 4] = b"RDSK";
const PART: &[u8; 4] = b"PART";
const NONE: u32 = 0xffff_ffff;
pub const MAX_RDB_SCAN_BLOCKS: u64 = 16;
pub const MAX_PARTITIONS: usize = 128;
const MIN_BLOCK: usize = 512;
const MAX_BLOCK: usize = 32768;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskError {
    Io(String),
    TooSmall,
    InvalidBlockSize(u32),
    Truncated,
    NoRdb,
    BadReference,
    Cycle,
    TooManyPartitions,
    BadGeometry,
    Overflow,
    PartitionOutsideImage,
}
impl std::fmt::Display for DiskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Amiga disk inspection error: {self:?}")
    }
}
impl std::error::Error for DiskError {}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileSystem {
    Dos(u8),
    Pfs,
    Sfs,
    MuFs,
    Unknown(u32),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partition {
    pub block_index: u32,
    pub next: u32,
    pub name: Option<String>,
    pub low_cyl: u32,
    pub high_cyl: u32,
    pub surfaces: u32,
    pub blocks_per_track: u32,
    pub boot_priority: i32,
    pub dos_type: u32,
    pub byte_offset: u64,
    pub byte_length: u64,
    pub boot_signature: Option<u32>,
    pub filesystem: FileSystem,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rdb {
    pub block_index: u64,
    pub block_size: u32,
    pub partition_head: u32,
    pub cylinders: u32,
    pub sectors: u32,
    pub heads: u32,
    pub rdb_blocks_low: u32,
    pub rdb_blocks_high: u32,
    pub partitions: Vec<Partition>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaDisk {
    pub path: PathBuf,
    pub image_size: u64,
    pub rdb: Rdb,
}
pub fn inspect_hdf(path: &Path) -> Result<AmigaDisk, DiskError> {
    let mut f = std::fs::File::open(path).map_err(|e| DiskError::Io(e.to_string()))?;
    let len = f
        .metadata()
        .map_err(|e| DiskError::Io(e.to_string()))?
        .len();
    if len < 512 {
        return Err(DiskError::TooSmall);
    };
    let (index, first) = find_rdb(&mut f, len)?;
    let block_size = be32(&first, 16)?;
    if !(MIN_BLOCK..=MAX_BLOCK).contains(&(block_size as usize)) || block_size % 512 != 0 {
        return Err(DiskError::InvalidBlockSize(block_size));
    }
    let block = read_block(&mut f, len, index, block_size as usize)?;
    if &block[..4] != RDSK {
        return Err(DiskError::NoRdb);
    }
    let mut r = Rdb {
        block_index: index,
        block_size,
        partition_head: be32(&block, 28)?,
        cylinders: be32(&block, 64)?,
        sectors: be32(&block, 68)?,
        heads: be32(&block, 72)?,
        rdb_blocks_low: be32(&block, 96)?,
        rdb_blocks_high: be32(&block, 100)?,
        partitions: Vec::new(),
    };
    let mut current = r.partition_head;
    let mut visited = BTreeSet::new();
    while current != NONE {
        if r.partitions.len() >= MAX_PARTITIONS {
            return Err(DiskError::TooManyPartitions);
        }
        if !visited.insert(current) {
            return Err(DiskError::Cycle);
        }
        let pblock = read_block(&mut f, len, current.into(), block_size as usize)?;
        if &pblock[..4] != PART {
            return Err(DiskError::BadReference);
        }
        let next = be32(&pblock, 16)?;
        let name = bstr(&pblock, 36);
        let surfaces = be32(&pblock, 140)?;
        let tracks = be32(&pblock, 148)?;
        let low = be32(&pblock, 164)?;
        let high = be32(&pblock, 168)?;
        let priority = be32(&pblock, 188)? as i32;
        let dos = be32(&pblock, 192)?;
        if surfaces == 0 || tracks == 0 || high < low {
            return Err(DiskError::BadGeometry);
        }
        let start = (low as u64)
            .checked_mul(surfaces as u64)
            .and_then(|v| v.checked_mul(tracks as u64))
            .ok_or(DiskError::Overflow)?;
        let cylinders = (high as u64)
            .checked_sub(low as u64)
            .and_then(|v| v.checked_add(1))
            .ok_or(DiskError::Overflow)?;
        let blocks = cylinders
            .checked_mul(surfaces as u64)
            .and_then(|v| v.checked_mul(tracks as u64))
            .ok_or(DiskError::Overflow)?;
        let offset = start
            .checked_mul(block_size as u64)
            .ok_or(DiskError::Overflow)?;
        let length = blocks
            .checked_mul(block_size as u64)
            .ok_or(DiskError::Overflow)?;
        if offset.checked_add(length).ok_or(DiskError::Overflow)? > len {
            return Err(DiskError::PartitionOutsideImage);
        }
        let boot = read_at(&mut f, len, offset, 4)
            .ok()
            .and_then(|b| b.try_into().ok())
            .map(u32::from_be_bytes);
        let fs = boot.map(classify).unwrap_or_else(|| classify(dos));
        r.partitions.push(Partition {
            block_index: current,
            next,
            name,
            low_cyl: low,
            high_cyl: high,
            surfaces,
            blocks_per_track: tracks,
            boot_priority: priority,
            dos_type: dos,
            byte_offset: offset,
            byte_length: length,
            boot_signature: boot,
            filesystem: fs,
        });
        current = next;
    }
    Ok(AmigaDisk {
        path: path.into(),
        image_size: len,
        rdb: r,
    })
}
pub fn structural_hdf_observation(_: &AmigaDisk) -> EvidenceObservation {
    EvidenceObservation {
        provenance: Provenance {
            channel: EvidenceChannel::LocalStructural,
            upstream_source: SourceFamily::Unknown,
            upstream_version: None,
            source_artifact: None,
            imported_at_unix: None,
            retrieved_at_unix: None,
            generator_version: None,
            lineage: LineageRelation::Independent,
            representation: Representation::WholeHdf,
        },
        claim: ClaimType::PlatformCandidate,
        claim_strength: ClaimStrength::Strong,
        identity_scope: IdentityScope::PlatformIdentity,
        hash_or_value: None,
        platform_candidate: Some("Amiga".into()),
        release_candidate: None,
        notes: Some("validated Amiga RDB hard-disk container".into()),
    }
}
fn find_rdb(f: &mut std::fs::File, len: u64) -> Result<(u64, Vec<u8>), DiskError> {
    for i in 0..MAX_RDB_SCAN_BLOCKS {
        if let Ok(b) = read_block(f, len, i, 512)
            && &b[..4] == RDSK
        {
            return Ok((i, b));
        }
    }
    Err(DiskError::NoRdb)
}
fn read_at(
    f: &mut std::fs::File,
    len: u64,
    offset: u64,
    count: usize,
) -> Result<Vec<u8>, DiskError> {
    let end = offset
        .checked_add(count as u64)
        .ok_or(DiskError::Overflow)?;
    if end > len {
        return Err(DiskError::Truncated);
    }
    f.seek(SeekFrom::Start(offset))
        .map_err(|e| DiskError::Io(e.to_string()))?;
    let mut b = vec![0; count];
    f.read_exact(&mut b)
        .map_err(|e| DiskError::Io(e.to_string()))?;
    Ok(b)
}
fn read_block(
    f: &mut std::fs::File,
    len: u64,
    index: u64,
    size: usize,
) -> Result<Vec<u8>, DiskError> {
    read_at(
        f,
        len,
        index.checked_mul(size as u64).ok_or(DiskError::Overflow)?,
        size,
    )
}
fn be32(b: &[u8], o: usize) -> Result<u32, DiskError> {
    let e = o.checked_add(4).ok_or(DiskError::Truncated)?;
    Ok(u32::from_be_bytes(
        b.get(o..e).ok_or(DiskError::Truncated)?.try_into().unwrap(),
    ))
}
fn bstr(b: &[u8], o: usize) -> Option<String> {
    let n = *b.get(o)? as usize;
    if n == 0 || n > 31 || o + 1 + n > b.len() {
        return None;
    }
    let raw = &b[o + 1..o + 1 + n];
    raw.iter()
        .all(|v| v.is_ascii() && (*v >= b' ' || *v == b'\t'))
        .then(|| String::from_utf8_lossy(raw).into_owned())
}
fn classify(raw: u32) -> FileSystem {
    let b = raw.to_be_bytes();
    match &b[..3] {
        b"DOS" if b[3] <= 7 => FileSystem::Dos(b[3]),
        b"PFS" => FileSystem::Pfs,
        b"SFS" => FileSystem::Sfs,
        b"MuF" => FileSystem::MuFs,
        _ => FileSystem::Unknown(raw),
    }
}
#[cfg(test)]
mod tests;
