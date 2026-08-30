//! Read-only bounded Amiga HDF/RDB container inspection and OFS/FFS traversal.
//!
//! Filesystem access is deliberately constrained to an RDB partition range;
//! it never mounts, repairs, or writes an HDF.
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
/// Read-only inspection of an Amiga disk image that tolerates two real
/// on-disk shapes: an RDB-partitioned HDF ([`inspect_hdf`], unchanged) and
/// a flat, unpartitioned AmigaDOS image with no RDB wrapper at all - the
/// shape real-world WHDLoad CD32 packs commonly ship as, where the whole
/// image is one boot-block-identified filesystem starting at byte 0.
///
/// A flat image is only recognised when its own boot block carries a
/// signature [`classify`] already treats as a real filesystem (`DOS0`
/// through `DOS7`, `PFS`, `SFS`, `MuFS`) - never merely because RDB
/// parsing failed. The resulting single synthetic partition spans the
/// whole file (`byte_offset: 0`, `byte_length` = file size) so downstream
/// callers (e.g. [`super::filesystem::discover_whdload_slaves`]) work
/// against it exactly as they would a real RDB partition.
pub fn inspect_amiga_image(path: &Path) -> Result<AmigaDisk, DiskError> {
    match inspect_hdf(path) {
        Ok(disk) => Ok(disk),
        Err(DiskError::NoRdb) => inspect_flat_amigados(path),
        Err(other) => Err(other),
    }
}

fn inspect_flat_amigados(path: &Path) -> Result<AmigaDisk, DiskError> {
    let mut f = std::fs::File::open(path).map_err(|e| DiskError::Io(e.to_string()))?;
    let len = f
        .metadata()
        .map_err(|e| DiskError::Io(e.to_string()))?
        .len();
    if len < 512 {
        return Err(DiskError::TooSmall);
    }
    let boot = read_at(&mut f, len, 0, 4)
        .ok()
        .and_then(|b| b.try_into().ok())
        .map(u32::from_be_bytes)
        .ok_or(DiskError::NoRdb)?;
    let filesystem = classify(boot);
    if matches!(filesystem, FileSystem::Unknown(_)) {
        return Err(DiskError::NoRdb);
    }
    Ok(AmigaDisk {
        path: path.into(),
        image_size: len,
        rdb: Rdb {
            block_index: 0,
            block_size: 512,
            partition_head: NONE,
            cylinders: 0,
            sectors: 0,
            heads: 0,
            rdb_blocks_low: 0,
            rdb_blocks_high: 0,
            partitions: vec![Partition {
                block_index: 0,
                next: NONE,
                name: None,
                low_cyl: 0,
                high_cyl: 0,
                surfaces: 0,
                blocks_per_track: 0,
                boot_priority: 0,
                dos_type: boot,
                byte_offset: 0,
                byte_length: len,
                boot_signature: Some(boot),
                filesystem,
            }],
        },
    })
}

/// The content-derived result of confirming one Amiga floppy / flat disk
/// image (`.adf`): the validated container plus the OFS/FFS filesystem
/// metadata the existing bounded reader was able to prove. Produced only
/// when both [`inspect_amiga_image`] and [`inspect_amiga_filesystem`]
/// succeed - never from an extension alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaFloppyInspection {
    pub disk: AmigaDisk,
    pub filesystem: AmigaFilesystem,
}

/// Why an `.adf` / flat Amiga image could not be structurally confirmed.
/// Every variant means "not trusted as Amiga content", never "probably
/// fine".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AmigaFloppyError {
    /// The container itself is not a readable Amiga image (bad/absent boot
    /// signature, truncation, impossible geometry, a ZIP or random bytes,
    /// an Acorn ADFS image, ...). Carries the underlying [`DiskError`].
    Container(DiskError),
    /// [`inspect_amiga_image`] succeeded but produced no partition to
    /// traverse - a shape no real AmigaDOS floppy has.
    NoPartition,
    /// The boot signature matched a DOS type, but the OFS/FFS boot and
    /// root structures did not validate through the existing bounded
    /// reader (malformed root block, unsupported block geometry, a
    /// non-DOS Amiga filesystem such as PFS/SFS/MuFS that stays
    /// detection-only). Carries the underlying [`FilesystemError`].
    Filesystem(FilesystemError),
}

impl std::fmt::Display for AmigaFloppyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Container(error) => write!(f, "{error}"),
            Self::NoPartition => f.write_str("Amiga image inspection error: NoPartition"),
            Self::Filesystem(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for AmigaFloppyError {}

/// Content-aware inspection of an Amiga floppy / flat disk image (`.adf`).
///
/// This adds **no** new parser: it reuses [`inspect_amiga_image`] (which
/// already accepts both an RDB-partitioned image and a flat, unpartitioned
/// AmigaDOS image) and then [`inspect_amiga_filesystem`] on the first
/// partition to validate the on-disc OFS/FFS boot and root blocks with the
/// existing bounded reader and its traversal limits.
///
/// `.adf` is a real cross-platform extension collision (Acorn ADFS /
/// Archimedes floppy images use it too), so identity here is taken only
/// from disc contents. A file that merely ends in `.adf` but whose bytes
/// do not present a valid AmigaDOS boot block and a structurally valid
/// root block is refused, not trusted.
pub fn inspect_amiga_floppy(path: &Path) -> Result<AmigaFloppyInspection, AmigaFloppyError> {
    let disk = inspect_amiga_image(path).map_err(AmigaFloppyError::Container)?;
    let partition = disk
        .rdb
        .partitions
        .first()
        .cloned()
        .ok_or(AmigaFloppyError::NoPartition)?;
    let filesystem =
        inspect_amiga_filesystem(&disk, &partition).map_err(AmigaFloppyError::Filesystem)?;
    Ok(AmigaFloppyInspection { disk, filesystem })
}

/// Strong, local, structural Amiga *platform* evidence for a `.adf` / flat
/// image whose OFS/FFS structures validated. Mirrors
/// [`structural_hdf_observation`], but for a floppy-shaped image
/// ([`Representation::StructuralMetadata`], not
/// [`Representation::WholeHdf`]). It is a platform candidate only: it never
/// asserts a game/release identity, so `release_candidate` and
/// `hash_or_value` are always `None` - a volume label is descriptive
/// context in `notes`, never an identity.
pub fn structural_amiga_floppy_observation(
    inspection: &AmigaFloppyInspection,
) -> EvidenceObservation {
    let filesystem = &inspection.filesystem;
    let family = match filesystem.family {
        AmigaDosFamily::Ofs => "OFS",
        AmigaDosFamily::Ffs => "FFS",
    };
    let mut notes = format!(
        "validated Amiga floppy image: DOS\\{} ({family}), {}-byte logical blocks",
        filesystem.dos_type, filesystem.block_size
    );
    if filesystem.international {
        notes.push_str(", international");
    }
    if filesystem.directory_cache {
        notes.push_str(", directory-cache");
    }
    if let Some(label) = filesystem
        .volume_label
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty())
    {
        notes.push_str(&format!(", volume {label:?}"));
    }
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
            representation: Representation::StructuralMetadata,
        },
        claim: ClaimType::PlatformCandidate,
        claim_strength: ClaimStrength::Strong,
        identity_scope: IdentityScope::PlatformIdentity,
        hash_or_value: None,
        platform_candidate: Some("Amiga".into()),
        release_candidate: None,
        notes: Some(notes),
    }
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

mod filesystem;
pub use filesystem::{
    AmigaDosFamily, AmigaFilesystem, DiscoveredSlave, FilesystemError, HdfSlaveDiscovery,
    PartitionTraversalLimits, discover_whdload_slaves, inspect_amiga_filesystem,
    structural_discovered_slave_observation,
};
#[cfg(test)]
mod tests;
