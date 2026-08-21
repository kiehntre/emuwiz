//! Bounded, read-only traversal of a single RDB-discovered OFS/FFS partition.
//!
//! `affs-read` is intentionally only given [`PartitionRangeDevice`], not the
//! HDF.  The adapter uses positional reads, rejects offsets outside the
//! discovered partition, and enforces a read budget in addition to the
//! traversal limits below.

use super::{AmigaDisk, FileSystem, Partition};
use crate::identity_source::whdload::{
    ParsedWHDLoadSlave, SlaveArtifact, SlaveHashes, parse_whdload_slave,
    structural_slave_observation,
};
use affs_read::{AffsReader, AffsReaderVar, BlockDevice, FileReader, FsType};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, VecDeque};
use std::fs::File;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const SECTOR_BYTES: u64 = 512;

/// The OFS/FFS family encoded by the AmigaDOS byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmigaDosFamily {
    /// Original File System.
    Ofs,
    /// Fast File System.
    Ffs,
}

/// Metadata validated from the boot/root blocks of a traversable partition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaFilesystem {
    /// Exact DOS byte, limited to DOS\0 through DOS\7.
    pub dos_type: u8,
    /// OFS or FFS, determined from the validated boot block.
    pub family: AmigaDosFamily,
    /// Whether the DOS variant enables international name comparison.
    pub international: bool,
    /// Whether the DOS variant enables directory-cache mode.
    pub directory_cache: bool,
    /// The logical filesystem block size used for traversal.
    pub block_size: u16,
    /// A display-only volume label, if safely representable.
    pub volume_label: Option<String>,
}

/// Explicit resource limits for one filesystem traversal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionTraversalLimits {
    /// Maximum nested directory depth below the root.
    pub max_directory_depth: usize,
    /// Maximum directory entries yielded across the traversal.
    pub max_directory_entries: usize,
    /// Maximum unique filesystem nodes (including the root) accepted.
    pub max_nodes_visited: usize,
    /// Maximum filename-hint candidates considered.
    pub max_slave_candidates: usize,
    /// Maximum bytes accepted for one candidate slave.
    pub max_individual_slave_bytes: u64,
    /// Maximum aggregate bytes read for candidate slaves.
    pub max_total_slave_bytes: u64,
    /// Maximum 512-byte positional reads made through the partition adapter.
    /// This additionally contains malformed iterator/data-block chains inside
    /// the third-party reader.
    pub max_block_reads: u64,
}

impl Default for PartitionTraversalLimits {
    fn default() -> Self {
        Self {
            max_directory_depth: 32,
            max_directory_entries: 10_000,
            max_nodes_visited: 20_000,
            max_slave_candidates: 64,
            max_individual_slave_bytes: 16 * 1024 * 1024,
            max_total_slave_bytes: 64 * 1024 * 1024,
            max_block_reads: 100_000,
        }
    }
}

/// A valid embedded WHDLoad slave and its in-image provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredSlave {
    /// Escaped, full path within the AmigaDOS volume. It is display/provenance
    /// metadata only and never contributes identity authority.
    pub in_image_path: String,
    /// The parsed slave and hashes of exactly the embedded slave bytes.
    pub artifact: SlaveArtifact,
}

/// The complete, deliberately non-resolved result for one partition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HdfSlaveDiscovery {
    /// The inspected HDF/RDB container.
    pub disk: AmigaDisk,
    /// The partition whose bounded range was traversed.
    pub partition: Partition,
    /// Filesystem metadata validated before traversal.
    pub filesystem: AmigaFilesystem,
    /// Every valid slave found; no candidate is selected as "best" here.
    pub candidates: Vec<DiscoveredSlave>,
    /// Corruption and budget-limit reports. Traversal returns safe partial
    /// results for these recoverable conditions.
    pub warnings: Vec<String>,
}

/// Read-only filesystem inspection refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesystemError {
    /// PFS, SFS, MuFS, and unknown filesystems remain detection-only.
    UnsupportedFilesystem(FileSystem),
    /// `affs-read`'s file streaming API is limited to 512-byte filesystem
    /// blocks; larger blocks are intentionally not partially traversed.
    UnsupportedBlockSize { bytes: usize },
    /// Partition geometry cannot be represented safely by the adapter.
    InvalidPartitionRange,
    /// The DOS boot/root structures were malformed or unreadable.
    InvalidFilesystem(String),
    /// Cancellation was observed between bounded operations.
    Cancelled,
}

impl std::fmt::Display for FilesystemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Amiga filesystem inspection error: {self:?}")
    }
}

impl std::error::Error for FilesystemError {}

/// Inspect a partition's OFS/FFS metadata without traversing its directories.
pub fn inspect_amiga_filesystem(
    disk: &AmigaDisk,
    partition: &Partition,
) -> Result<AmigaFilesystem, FilesystemError> {
    let limits = PartitionTraversalLimits::default();
    let device = open_partition(disk, partition, limits.max_block_reads)?;
    reader_metadata(&device, partition)
}

/// Discover valid WHDLoad slaves in one RDB-discovered OFS/FFS partition.
///
/// The filename extension is only a bounded discovery hint. A result is added
/// only after the embedded bytes pass `parse_whdload_slave`.
pub fn discover_whdload_slaves(
    disk: &AmigaDisk,
    partition: &Partition,
    limits: &PartitionTraversalLimits,
    cancel: Option<&AtomicBool>,
) -> Result<HdfSlaveDiscovery, FilesystemError> {
    check_cancel(cancel)?;
    let device = open_partition(disk, partition, limits.max_block_reads)?;
    let filesystem = reader_metadata(&device, partition)?;
    let sector_count = u32::try_from(partition.byte_length / SECTOR_BYTES)
        .map_err(|_| FilesystemError::InvalidPartitionRange)?;
    let reader = AffsReader::with_size(&device, sector_count)
        .map_err(|error| FilesystemError::InvalidFilesystem(format!("{error:?}")))?;
    let mut warnings = Vec::new();
    let mut candidates = Vec::new();
    let mut visited = BTreeSet::new();
    let mut directories = VecDeque::from([(reader.root_block(), String::new(), 0_usize)]);
    visited.insert(reader.root_block());
    let mut entry_count = 0_usize;
    let mut hinted_candidates = 0_usize;
    let mut candidate_bytes = 0_u64;
    let mut stop = false;

    while let Some((directory, parent_path, depth)) = directories.pop_front() {
        check_cancel(cancel)?;
        let iter = match reader.read_dir(directory) {
            Ok(iter) => iter,
            Err(error) => {
                warnings.push(format!("directory block {directory} rejected: {error:?}"));
                continue;
            }
        };

        for entry in iter {
            check_cancel(cancel)?;
            if entry_count >= limits.max_directory_entries {
                warnings.push(format!(
                    "directory-entry limit {} reached; traversal stopped",
                    limits.max_directory_entries
                ));
                stop = true;
                break;
            }
            entry_count += 1;
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    warnings.push(format!(
                        "directory block {directory} contains an unreadable entry: {error:?}"
                    ));
                    break;
                }
            };
            if visited.contains(&entry.block) {
                warnings.push(format!(
                    "revisited filesystem node {}; skipped to prevent a cycle",
                    entry.block
                ));
                continue;
            }
            if visited.len() >= limits.max_nodes_visited {
                warnings.push(format!(
                    "filesystem-node limit {} reached; traversal stopped",
                    limits.max_nodes_visited
                ));
                stop = true;
                break;
            }
            visited.insert(entry.block);

            let component = display_component(entry.name());
            let path = join_image_path(&parent_path, &component);
            if entry.is_dir() {
                if depth >= limits.max_directory_depth {
                    warnings.push(format!(
                        "directory depth limit {} reached at {path}; skipped",
                        limits.max_directory_depth
                    ));
                } else {
                    directories.push_back((entry.block, path, depth + 1));
                }
                continue;
            }
            if !entry.is_file() || !is_slave_hint(entry.name()) {
                continue;
            }
            if hinted_candidates >= limits.max_slave_candidates {
                warnings.push(format!(
                    "slave-candidate limit {} reached; remaining hints skipped",
                    limits.max_slave_candidates
                ));
                stop = true;
                break;
            }
            hinted_candidates += 1;
            let size = u64::from(entry.size);
            if size > limits.max_individual_slave_bytes {
                warnings.push(format!(
                    "candidate {path} is {size} bytes, over individual limit {}",
                    limits.max_individual_slave_bytes
                ));
                continue;
            }
            let Some(next_total) = candidate_bytes.checked_add(size) else {
                warnings.push(format!("candidate-byte accounting overflow at {path}"));
                continue;
            };
            if next_total > limits.max_total_slave_bytes {
                warnings.push(format!(
                    "candidate {path} exceeds total candidate-byte limit {}",
                    limits.max_total_slave_bytes
                ));
                continue;
            }
            let mut bytes = vec![0; entry.size as usize];
            let mut file = match FileReader::new(&device, reader.fs_type(), entry.block) {
                Ok(file) => file,
                Err(error) => {
                    warnings.push(format!("candidate {path} header rejected: {error:?}"));
                    continue;
                }
            };
            match file.read_all(&mut bytes) {
                Ok(read) if read == bytes.len() => {
                    candidate_bytes = next_total;
                }
                Ok(read) => {
                    warnings.push(format!(
                        "candidate {path} ended after {read} of {} bytes",
                        bytes.len()
                    ));
                    continue;
                }
                Err(error) => {
                    warnings.push(format!("candidate {path} data rejected: {error:?}"));
                    continue;
                }
            }
            let parsed = match parse_whdload_slave(&bytes) {
                Ok(parsed) => parsed,
                Err(_) => continue,
            };
            candidates.push(DiscoveredSlave {
                in_image_path: path,
                artifact: embedded_artifact(disk.path.clone(), component, bytes, parsed),
            });
        }
        if stop {
            break;
        }
    }
    if device.read_budget_exhausted() {
        warnings.push(format!(
            "filesystem block-read limit {} reached; traversal stopped safely",
            limits.max_block_reads
        ));
    }
    Ok(HdfSlaveDiscovery {
        disk: disk.clone(),
        partition: partition.clone(),
        filesystem,
        candidates,
        warnings,
    })
}

/// Convert a validated embedded slave to direct local WHDLoad evidence.
/// Exact-slave evidence remains available only to callers which first match
/// `candidate.artifact.hashes.sha1` against a known slave catalogue.
pub fn structural_discovered_slave_observation(
    candidate: &DiscoveredSlave,
) -> crate::platform_evidence_fusion::evidence_lineage::EvidenceObservation {
    structural_slave_observation(&candidate.artifact)
}

fn reader_metadata(
    device: &PartitionRangeDevice,
    partition: &Partition,
) -> Result<AmigaFilesystem, FilesystemError> {
    let dos_type = match partition.filesystem {
        FileSystem::Dos(value) => value,
        ref other => return Err(FilesystemError::UnsupportedFilesystem(other.clone())),
    };
    let mut boot = [0_u8; 512];
    device.read_block(0, &mut boot).map_err(|()| {
        FilesystemError::InvalidFilesystem("cannot read partition boot block".to_string())
    })?;
    if boot[..3] != *b"DOS" || boot[3] != dos_type || dos_type > 7 {
        return Err(FilesystemError::InvalidFilesystem(
            "partition boot block does not match a supported DOS\\0..DOS\\7 type".to_string(),
        ));
    }
    let sectors = partition.byte_length / SECTOR_BYTES;
    let sectors = u32::try_from(sectors).map_err(|_| FilesystemError::InvalidPartitionRange)?;
    let reader = match AffsReader::with_size(device, sectors) {
        Ok(reader) => reader,
        Err(error) => {
            // The upstream variable-block reader can safely identify a larger
            // FFS block size, but it has no file-streaming API. Report that
            // limitation rather than pretending directory listing alone is
            // usable for slave validation.
            if let Ok(probe) = AffsReaderVar::new(device, u64::from(sectors))
                && probe.block_size() != SECTOR_BYTES as usize
            {
                return Err(FilesystemError::UnsupportedBlockSize {
                    bytes: probe.block_size(),
                });
            }
            return Err(FilesystemError::InvalidFilesystem(format!(
                "512-byte OFS/FFS reader rejected filesystem: {error:?}"
            )));
        }
    };
    let family = match reader.fs_type() {
        FsType::Ofs => AmigaDosFamily::Ofs,
        FsType::Ffs => AmigaDosFamily::Ffs,
    };
    let flags = reader.fs_flags();
    Ok(AmigaFilesystem {
        dos_type,
        family,
        international: flags.intl,
        directory_cache: flags.dircache,
        block_size: SECTOR_BYTES as u16,
        volume_label: safe_label(reader.disk_name()),
    })
}

fn open_partition(
    disk: &AmigaDisk,
    partition: &Partition,
    max_block_reads: u64,
) -> Result<PartitionRangeDevice, FilesystemError> {
    let end = partition
        .byte_offset
        .checked_add(partition.byte_length)
        .ok_or(FilesystemError::InvalidPartitionRange)?;
    if partition.byte_length < 1024
        || !partition.byte_length.is_multiple_of(SECTOR_BYTES)
        || end > disk.image_size
    {
        return Err(FilesystemError::InvalidPartitionRange);
    }
    PartitionRangeDevice::open(
        &disk.path,
        partition.byte_offset,
        partition.byte_length,
        max_block_reads,
    )
}

/// The only storage view handed to `affs-read`: a read-only slice of one HDF
/// partition. It has no API for writes, mounts, metadata changes, or paths.
struct PartitionRangeDevice {
    file: File,
    offset: u64,
    length: u64,
    max_reads: u64,
    reads: AtomicU64,
}

impl PartitionRangeDevice {
    fn open(
        path: &std::path::Path,
        offset: u64,
        length: u64,
        max_reads: u64,
    ) -> Result<Self, FilesystemError> {
        let file = File::open(path).map_err(|error| {
            FilesystemError::InvalidFilesystem(format!("cannot open HDF: {error}"))
        })?;
        let image_size = file
            .metadata()
            .map_err(|error| {
                FilesystemError::InvalidFilesystem(format!("cannot stat HDF: {error}"))
            })?
            .len();
        if offset
            .checked_add(length)
            .is_none_or(|end| end > image_size)
        {
            return Err(FilesystemError::InvalidPartitionRange);
        }
        Ok(Self {
            file,
            offset,
            length,
            max_reads,
            reads: AtomicU64::new(0),
        })
    }

    fn read_budget_exhausted(&self) -> bool {
        self.reads.load(Ordering::Relaxed) >= self.max_reads
    }

    fn read_sector(&self, sector: u32, out: &mut [u8; 512]) -> Result<(), ()> {
        let prior = self.reads.fetch_add(1, Ordering::Relaxed);
        if prior >= self.max_reads {
            return Err(());
        }
        let relative = u64::from(sector).checked_mul(SECTOR_BYTES).ok_or(())?;
        let end = relative.checked_add(SECTOR_BYTES).ok_or(())?;
        if end > self.length {
            return Err(());
        }
        let absolute = self.offset.checked_add(relative).ok_or(())?;
        read_exact_at(&self.file, absolute, out).map_err(|_| ())
    }
}

impl BlockDevice for PartitionRangeDevice {
    fn read_block(&self, block: u32, buf: &mut [u8; 512]) -> Result<(), ()> {
        self.read_sector(block, buf)
    }
}

#[cfg(unix)]
fn read_exact_at(file: &File, mut offset: u64, mut out: &mut [u8]) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    while !out.is_empty() {
        let count = file.read_at(out, offset)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "short positional read",
            ));
        }
        offset = offset
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "offset overflow"))?;
        out = &mut out[count..];
    }
    Ok(())
}

#[cfg(windows)]
fn read_exact_at(file: &File, mut offset: u64, mut out: &mut [u8]) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !out.is_empty() {
        let count = file.seek_read(out, offset)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "short positional read",
            ));
        }
        offset = offset
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "offset overflow"))?;
        out = &mut out[count..];
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn read_exact_at(_: &File, _: u64, _: &mut [u8]) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "positional reads are unavailable on this platform",
    ))
}

fn check_cancel(cancel: Option<&AtomicBool>) -> Result<(), FilesystemError> {
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        Err(FilesystemError::Cancelled)
    } else {
        Ok(())
    }
}

fn is_slave_hint(name: &[u8]) -> bool {
    let lower = name.iter().map(u8::to_ascii_lowercase).collect::<Vec<_>>();
    lower.ends_with(b".slave") || lower.ends_with(b".islave")
}

fn display_component(name: &[u8]) -> String {
    let mut output = String::new();
    for &byte in name {
        if byte.is_ascii_graphic() && byte != b'/' && byte != b'%' {
            output.push(char::from(byte));
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    if output.is_empty() {
        "%00".to_string()
    } else {
        output
    }
}

fn join_image_path(parent: &str, component: &str) -> String {
    if parent.is_empty() {
        format!("/{component}")
    } else {
        format!("{parent}/{component}")
    }
}

fn safe_label(label: &[u8]) -> Option<String> {
    label
        .iter()
        .all(|byte| byte.is_ascii_graphic() || *byte == b' ')
        .then(|| String::from_utf8_lossy(label).into_owned())
}

fn embedded_artifact(
    disk_path: PathBuf,
    name: String,
    bytes: Vec<u8>,
    parsed: ParsedWHDLoadSlave,
) -> SlaveArtifact {
    let mut sha1 = Sha1::new();
    let mut sha256 = Sha256::new();
    sha1.update(&bytes);
    sha256.update(&bytes);
    SlaveArtifact {
        path: disk_path,
        name,
        size_bytes: bytes.len() as u64,
        parsed,
        hashes: SlaveHashes {
            sha1: hexadecimal(sha1.finalize().as_slice()),
            sha256: hexadecimal(sha256.finalize().as_slice()),
        },
    }
}

fn hexadecimal(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amiga_disk::{ClaimType, Representation, inspect_hdf, structural_hdf_observation};
    use crate::platform_evidence_fusion::evidence_lineage::{EvidenceChannel, SourceFamily};
    use tempfile::tempdir;

    const PARTITION_START: usize = 128;
    const PARTITION_BLOCKS: usize = 128;
    const ROOT: usize = 64;
    const NONE: u32 = u32::MAX;

    fn put32(block: &mut [u8], offset: usize, value: u32) {
        block[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    fn put_i32(block: &mut [u8], offset: usize, value: i32) {
        put32(block, offset, value as u32);
    }

    fn checksum(block: &mut [u8; 512]) {
        let mut sum = 0_u32;
        for offset in (0..512).step_by(4) {
            if offset != 20 {
                sum = sum.wrapping_add(u32::from_be_bytes(
                    block[offset..offset + 4].try_into().unwrap(),
                ));
            }
        }
        put32(block, 20, (sum as i32).wrapping_neg() as u32);
    }

    fn root(entries: &[(usize, u32)]) -> [u8; 512] {
        let mut block = [0_u8; 512];
        put_i32(&mut block, 0, 2);
        put32(&mut block, 12, 72);
        for &(slot, target) in entries {
            put32(&mut block, 24 + slot * 4, target);
        }
        block[0x1B0] = 4;
        block[0x1B1..0x1B5].copy_from_slice(b"Work");
        put_i32(&mut block, 508, 1);
        checksum(&mut block);
        block
    }

    fn directory(name: &[u8], entries: &[(usize, u32)]) -> [u8; 512] {
        let mut block = [0_u8; 512];
        put_i32(&mut block, 0, 2);
        for &(slot, target) in entries {
            put32(&mut block, 24 + slot * 4, target);
        }
        block[0x1B0] = name.len() as u8;
        block[0x1B1..0x1B1 + name.len()].copy_from_slice(name);
        put_i32(&mut block, 508, 2);
        checksum(&mut block);
        block
    }

    fn file_header(name: &[u8], bytes: usize, data_block: u32, next_same_hash: u32) -> [u8; 512] {
        let mut block = [0_u8; 512];
        put_i32(&mut block, 0, 2);
        put_i32(&mut block, 8, 1);
        put32(&mut block, 16, data_block);
        put32(&mut block, 24 + 71 * 4, data_block);
        put32(&mut block, 0x144, bytes as u32);
        put32(&mut block, 0x1F0, next_same_hash);
        block[0x1B0] = name.len() as u8;
        block[0x1B1..0x1B1 + name.len()].copy_from_slice(name);
        put_i32(&mut block, 0x1FC, -3);
        checksum(&mut block);
        block
    }

    fn data_block(dos: u8, header: u32, bytes: &[u8]) -> [u8; 512] {
        let mut block = [0_u8; 512];
        if dos & 1 == 0 {
            put_i32(&mut block, 0, 8);
            put32(&mut block, 4, header);
            put32(&mut block, 8, 1);
            put32(&mut block, 12, bytes.len() as u32);
            block[24..24 + bytes.len()].copy_from_slice(bytes);
            checksum(&mut block);
        } else {
            block[..bytes.len()].copy_from_slice(bytes);
        }
        block
    }

    fn slave() -> Vec<u8> {
        let mut code = vec![0_u8; 56];
        code[..4].copy_from_slice(&[0x70, 0xff, 0x4e, 0x75]);
        code[4..12].copy_from_slice(b"WHDLOADS");
        code[12..14].copy_from_slice(&20_u16.to_be_bytes());
        code[16..20].copy_from_slice(&512_u32.to_be_bytes());
        let mut output = Vec::new();
        for word in [0x3f3_u32, 0, 1, 0, 0, 14, 0x3e9, 14] {
            output.extend_from_slice(&word.to_be_bytes());
        }
        output.extend_from_slice(&code);
        output.extend_from_slice(&0x3f2_u32.to_be_bytes());
        output
    }

    fn set_block(partition: &mut [u8], index: usize, block: [u8; 512]) {
        partition[index * 512..(index + 1) * 512].copy_from_slice(&block);
    }

    fn hdf(dos: u8, partition: &[u8]) -> Vec<u8> {
        assert_eq!(partition.len(), PARTITION_BLOCKS * 512);
        let mut image = vec![0_u8; (PARTITION_START + PARTITION_BLOCKS) * 512];
        image[..4].copy_from_slice(b"RDSK");
        put32(&mut image, 16, 512);
        put32(&mut image, 28, 1);
        let part = 512;
        image[part..part + 4].copy_from_slice(b"PART");
        put32(&mut image, part + 16, NONE);
        image[part + 36] = 4;
        image[part + 37..part + 41].copy_from_slice(b"Work");
        put32(&mut image, part + 140, 1);
        put32(&mut image, part + 148, PARTITION_BLOCKS as u32);
        put32(&mut image, part + 164, 1);
        put32(&mut image, part + 168, 1);
        put32(
            &mut image,
            part + 192,
            u32::from_be_bytes(*b"DOS\0") + u32::from(dos),
        );
        image[PARTITION_START * 512..(PARTITION_START + PARTITION_BLOCKS) * 512]
            .copy_from_slice(partition);
        image
    }

    fn write_fixture(bytes: &[u8]) -> (tempfile::TempDir, PathBuf, AmigaDisk, Partition) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("fixture.hdf");
        std::fs::write(&path, bytes).unwrap();
        let disk = inspect_hdf(&path).unwrap();
        let partition = disk.rdb.partitions[0].clone();
        (dir, path, disk, partition)
    }

    fn basic_partition(dos: u8, name: &[u8], contents: &[u8]) -> Vec<u8> {
        let mut partition = vec![0_u8; PARTITION_BLOCKS * 512];
        partition[..3].copy_from_slice(b"DOS");
        partition[3] = dos;
        put32(&mut partition, 8, ROOT as u32);
        set_block(&mut partition, ROOT, root(&[(0, 10)]));
        set_block(&mut partition, 10, file_header(name, contents.len(), 11, 0));
        set_block(&mut partition, 11, data_block(dos, 10, contents));
        partition
    }

    #[test]
    fn all_dos_variants_traverse_and_preserve_flags() {
        for dos in 0..=7 {
            let bytes = slave();
            let partition = basic_partition(dos, b"Game.SlAvE", &bytes);
            let (_dir, _path, disk, partition) = write_fixture(&hdf(dos, &partition));
            let discovery = discover_whdload_slaves(
                &disk,
                &partition,
                &PartitionTraversalLimits::default(),
                None,
            )
            .unwrap();
            assert_eq!(
                discovery.candidates.len(),
                1,
                "DOS\\{dos}: {:?}",
                discovery.warnings
            );
            assert_eq!(discovery.filesystem.dos_type, dos);
            assert_eq!(
                discovery.filesystem.family,
                if dos & 1 == 0 {
                    AmigaDosFamily::Ofs
                } else {
                    AmigaDosFamily::Ffs
                }
            );
            assert_eq!(discovery.filesystem.international, dos & 2 != 0);
            assert_eq!(discovery.filesystem.directory_cache, dos & 4 != 0);
        }
    }

    #[test]
    fn nested_path_and_valid_slave_are_preserved() {
        let bytes = slave();
        let mut partition = vec![0_u8; PARTITION_BLOCKS * 512];
        partition[..4].copy_from_slice(b"DOS\x01");
        put32(&mut partition, 8, ROOT as u32);
        set_block(&mut partition, ROOT, root(&[(0, 20)]));
        set_block(&mut partition, 20, directory(b"Games", &[(0, 21)]));
        set_block(
            &mut partition,
            21,
            file_header(b"Golden.islave", bytes.len(), 22, 0),
        );
        set_block(&mut partition, 22, data_block(1, 21, &bytes));
        let (_dir, _path, disk, partition) = write_fixture(&hdf(1, &partition));
        let discovery =
            discover_whdload_slaves(&disk, &partition, &Default::default(), None).unwrap();
        assert_eq!(
            discovery.candidates[0].in_image_path,
            "/Games/Golden.islave"
        );
        assert_eq!(discovery.candidates[0].artifact.parsed.runtime_version, 20);
        let evidence = structural_discovered_slave_observation(&discovery.candidates[0]);
        assert_eq!(
            evidence.provenance.representation,
            Representation::WHDLoadSlave
        );
        assert_eq!(evidence.provenance.channel, EvidenceChannel::LocalWHDLoad);
        assert_eq!(evidence.provenance.upstream_source, SourceFamily::WHDLoad);
        assert_eq!(evidence.platform_candidate.as_deref(), Some("Amiga"));
        assert_ne!(evidence.claim, ClaimType::ExactSlaveMatch);
    }

    #[test]
    fn invalid_extension_contents_are_not_evidence() {
        let partition = basic_partition(1, b"NotASlave.slave", b"not a WHDLoad slave");
        let (_dir, _path, disk, partition) = write_fixture(&hdf(1, &partition));
        assert!(
            discover_whdload_slaves(&disk, &partition, &Default::default(), None)
                .unwrap()
                .candidates
                .is_empty()
        );
    }

    #[test]
    fn multiple_slaves_are_preserved_without_selection() {
        let first = slave();
        let second = slave();
        let mut partition = vec![0_u8; PARTITION_BLOCKS * 512];
        partition[..4].copy_from_slice(b"DOS\x01");
        put32(&mut partition, 8, ROOT as u32);
        set_block(&mut partition, ROOT, root(&[(0, 10)]));
        set_block(
            &mut partition,
            10,
            file_header(b"First.slave", first.len(), 11, 12),
        );
        set_block(&mut partition, 11, data_block(1, 10, &first));
        set_block(
            &mut partition,
            12,
            file_header(b"Second.slave", second.len(), 13, 0),
        );
        set_block(&mut partition, 13, data_block(1, 12, &second));
        let (_dir, _path, disk, partition) = write_fixture(&hdf(1, &partition));
        let discovery =
            discover_whdload_slaves(&disk, &partition, &Default::default(), None).unwrap();
        assert_eq!(discovery.candidates.len(), 2);
        assert_eq!(discovery.candidates[0].in_image_path, "/First.slave");
        assert_eq!(discovery.candidates[1].in_image_path, "/Second.slave");
    }

    #[test]
    fn traversal_limits_and_revisits_stop_safely() {
        let bytes = slave();
        let mut nested = vec![0_u8; PARTITION_BLOCKS * 512];
        nested[..4].copy_from_slice(b"DOS\x01");
        put32(&mut nested, 8, ROOT as u32);
        set_block(&mut nested, ROOT, root(&[(0, 20)]));
        set_block(&mut nested, 20, directory(b"Deep", &[(0, 21)]));
        set_block(
            &mut nested,
            21,
            file_header(b"Game.slave", bytes.len(), 22, 0),
        );
        set_block(&mut nested, 22, data_block(1, 21, &bytes));
        let (_dir, _path, disk, partition) = write_fixture(&hdf(1, &nested));
        let depth = PartitionTraversalLimits {
            max_directory_depth: 0,
            ..Default::default()
        };
        let discovery = discover_whdload_slaves(&disk, &partition, &depth, None).unwrap();
        assert!(discovery.candidates.is_empty());
        assert!(
            discovery
                .warnings
                .iter()
                .any(|warning| warning.contains("depth limit"))
        );

        let mut cycle = vec![0_u8; PARTITION_BLOCKS * 512];
        cycle[..4].copy_from_slice(b"DOS\x01");
        put32(&mut cycle, 8, ROOT as u32);
        set_block(&mut cycle, ROOT, root(&[(0, 20)]));
        set_block(&mut cycle, 20, directory(b"Loop", &[(0, 20)]));
        let (_dir, _path, disk, partition) = write_fixture(&hdf(1, &cycle));
        let discovery =
            discover_whdload_slaves(&disk, &partition, &Default::default(), None).unwrap();
        assert!(
            discovery
                .warnings
                .iter()
                .any(|warning| warning.contains("revisited"))
        );
    }

    #[test]
    fn entry_corruption_and_candidate_size_limits_are_recoverable() {
        let mut corrupt = vec![0_u8; PARTITION_BLOCKS * 512];
        corrupt[..4].copy_from_slice(b"DOS\x01");
        put32(&mut corrupt, 8, ROOT as u32);
        set_block(&mut corrupt, ROOT, root(&[(0, 10)]));
        let (_dir, _path, disk, partition) = write_fixture(&hdf(1, &corrupt));
        let discovery =
            discover_whdload_slaves(&disk, &partition, &Default::default(), None).unwrap();
        assert!(
            discovery
                .warnings
                .iter()
                .any(|warning| warning.contains("unreadable entry"))
        );

        let mut oversized = basic_partition(1, b"Large.slave", b"x");
        set_block(&mut oversized, 10, file_header(b"Large.slave", 4096, 11, 0));
        let (_dir, _path, disk, partition) = write_fixture(&hdf(1, &oversized));
        let limits = PartitionTraversalLimits {
            max_individual_slave_bytes: 128,
            ..Default::default()
        };
        let discovery = discover_whdload_slaves(&disk, &partition, &limits, None).unwrap();
        assert!(discovery.candidates.is_empty());
        assert!(
            discovery
                .warnings
                .iter()
                .any(|warning| warning.contains("individual limit"))
        );
    }

    #[test]
    fn entry_and_node_limits_are_enforced_before_unbounded_iteration() {
        let first = slave();
        let second = slave();
        let mut partition = vec![0_u8; PARTITION_BLOCKS * 512];
        partition[..4].copy_from_slice(b"DOS\x01");
        put32(&mut partition, 8, ROOT as u32);
        set_block(&mut partition, ROOT, root(&[(0, 10)]));
        set_block(
            &mut partition,
            10,
            file_header(b"First.slave", first.len(), 11, 12),
        );
        set_block(&mut partition, 11, data_block(1, 10, &first));
        set_block(
            &mut partition,
            12,
            file_header(b"Second.slave", second.len(), 13, 0),
        );
        set_block(&mut partition, 13, data_block(1, 12, &second));
        let (_dir, _path, disk, partition) = write_fixture(&hdf(1, &partition));
        let entries = PartitionTraversalLimits {
            max_directory_entries: 1,
            ..Default::default()
        };
        let discovery = discover_whdload_slaves(&disk, &partition, &entries, None).unwrap();
        assert_eq!(discovery.candidates.len(), 1);
        assert!(
            discovery
                .warnings
                .iter()
                .any(|warning| warning.contains("directory-entry limit"))
        );

        let nodes = PartitionTraversalLimits {
            max_nodes_visited: 1,
            ..Default::default()
        };
        let discovery = discover_whdload_slaves(&disk, &partition, &nodes, None).unwrap();
        assert!(discovery.candidates.is_empty());
        assert!(
            discovery
                .warnings
                .iter()
                .any(|warning| warning.contains("filesystem-node limit"))
        );
    }

    #[test]
    fn unsupported_filesystems_and_partition_boundaries_do_not_traverse() {
        let partition = vec![0_u8; PARTITION_BLOCKS * 512];
        let mut image = hdf(1, &partition);
        image[PARTITION_START * 512..PARTITION_START * 512 + 4].copy_from_slice(b"PFS\x03");
        let (_dir, path, disk, partition) = write_fixture(&image);
        assert!(matches!(
            inspect_amiga_filesystem(&disk, &partition),
            Err(FilesystemError::UnsupportedFilesystem(FileSystem::Pfs))
        ));
        for (signature, expected) in [(b"SFS\0", FileSystem::Sfs), (b"MuFS", FileSystem::MuFs)] {
            let mut image = hdf(1, &vec![0_u8; PARTITION_BLOCKS * 512]);
            image[PARTITION_START * 512..PARTITION_START * 512 + 4].copy_from_slice(signature);
            let (_dir, _path, disk, partition) = write_fixture(&image);
            assert_eq!(
                inspect_amiga_filesystem(&disk, &partition),
                Err(FilesystemError::UnsupportedFilesystem(expected))
            );
        }

        let device =
            PartitionRangeDevice::open(&path, (PARTITION_START * 512) as u64, 1024, 8).unwrap();
        let mut block = [0_u8; 512];
        assert!(device.read_block(0, &mut block).is_ok());
        assert!(device.read_block(1, &mut block).is_ok());
        assert!(device.read_block(2, &mut block).is_err());
    }

    #[test]
    fn names_and_whole_hdf_never_gain_exact_slave_authority() {
        let bytes = slave();
        let partition = basic_partition(1, b"PretendExact.slave", &bytes);
        let (_dir, _path, disk, partition) = write_fixture(&hdf(1, &partition));
        let discovery =
            discover_whdload_slaves(&disk, &partition, &Default::default(), None).unwrap();
        let observation = structural_discovered_slave_observation(&discovery.candidates[0]);
        assert!(observation.hash_or_value.is_none());
        assert_ne!(observation.claim, ClaimType::ExactSlaveMatch);
        let hdf_observation = structural_hdf_observation(&disk);
        assert_eq!(
            hdf_observation.provenance.representation,
            Representation::WholeHdf
        );
        assert_ne!(hdf_observation.claim, ClaimType::ExactSlaveMatch);
    }
}
