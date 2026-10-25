//! Bounded, read-only XDVDFS filesystem traversal - shared by
//! [`crate::xbox_boot_evidence`] (original Xbox) and
//! [`crate::xbox360_boot_evidence`] (Xbox 360), since both consoles use the
//! same disc filesystem. Builds on the format signature already verified in
//! [`crate::xdvdfs_signature`].
//!
//! # What this wires up, and what it deliberately does not
//!
//! [`crate::xdvdfs_signature`]'s own documentation explains why the
//! previous milestone stopped at the volume-descriptor magic check alone:
//! the `xdvdfs` crate (`https://crates.io/crates/xdvdfs`, pure Rust,
//! already a dependency) was in place but not yet integrated. This module
//! is that integration - but still deliberately small:
//!
//! - [`list_root`]: every entry directly under the volume root.
//! - [`find_path`]: one exact path, resolved case-insensitively (the
//!   crate's own `find_dirent` already does this - XDVDFS is not
//!   case-sensitive, matching the real Xbox/Xbox 360 kernels).
//! - [`read_file_prefix`]: a bounded prefix of one file's bytes.
//!
//! There is no recursive whole-disc walk, no virtual filesystem
//! abstraction, and no directory-tree cache. A caller that needs
//! `/default.xbe` or `/default.xex` calls [`find_path`] once and, if it
//! exists, [`read_file_prefix`] once - exactly the shape
//! [`crate::xbox_boot_evidence`]/[`crate::xbox360_boot_evidence`] need.
//!
//! # Real Xbox disc images: bounded, random-access, never materialized whole
//!
//! [`list_root`]/[`find_path`]/[`read_file_prefix`] above all require the
//! entire image as one in-memory `&[u8]` - safe for a small archive member,
//! unsafe for a multi-gigabyte Xbox disc image. [`list_root_in_disc_image`]/
//! [`find_path_in_disc_image`]/[`read_file_prefix_in_disc_image`] are the
//! same operations, unchanged, over an already-open [`std::fs::File`]
//! instead: every read is one bounded `seek`+`read_exact` for exactly the
//! bytes requested, under the same [`MAX_TRAVERSAL_READS`] budget - never a
//! whole-file read. They also transparently handle both real-world disc
//! layouts via the `xdvdfs` crate's own [`xdvdfs::blockdev::OffsetWrapper`]:
//! a raw/stripped XISO (volume descriptor at byte `0`) and a full,
//! unstripped Redump-style dump (volume descriptor at one of three fixed
//! XGD1/XGD2/XGD3 offsets) - see [`open_disc_volume`]'s own doc comment.
//! No offset is guessed here; every candidate comes from the vetted
//! upstream crate.
//!
//! # Safety bounds (never a panic, always an explicit error)
//!
//! - [`MAX_ROOT_ENTRIES`]: the root listing is truncated (never silently
//!   grown) past this many entries.
//! - [`MAX_PATH_DEPTH`]/[`MAX_PATH_BYTES`]: enforced before any traversal
//!   begins, so a pathological path can never even start walking.
//! - [`MAX_FILE_PREFIX_BYTES`]: the hard ceiling on any one
//!   [`read_file_prefix`] call, regardless of the caller's requested size.
//! - **Cycle/corruption protection**: the `xdvdfs` crate's own
//!   `walk_dirent_tree`/`walk_path` do not themselves bound the number of
//!   block-device reads they perform, so a directory table with a
//!   corrupted or cyclic left/right-child offset could otherwise force
//!   unbounded work. [`BoundedSliceReader`] wraps the input bytes with a
//!   hard read-call budget ([`MAX_TRAVERSAL_READS`]) far above what any
//!   well-formed disc needs, so a malformed tree fails closed with
//!   [`XdvdfsTraversalError::TraversalBudgetExceeded`] instead of spinning.
//!
//! # Format verified, not assumed
//!
//! The traversal algorithm itself (binary-tree directory entries, root
//! table walk, `walk_path`/`walk_dirent_tree`) is the `xdvdfs` crate's own,
//! already-published implementation - not re-derived here. This module
//! only adds the bounds above and translates results into this crate's own
//! evidence-shaped types.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use alloc_free_prelude::*;
use xdvdfs::blockdev::{BlockDeviceRead, OffsetWrapper};
use xdvdfs::layout::VolumeDescriptor;
use xdvdfs::read::read_volume;
use xdvdfs::util::Error as XdvdfsCrateError;

/// std re-exports under one name so the module doc above reads cleanly;
/// this crate is not `no_std`, so this is just `std`.
mod alloc_free_prelude {
    pub use std::string::String;
    pub use std::vec::Vec;
}

/// Root-listing bound - see the module documentation.
pub const MAX_ROOT_ENTRIES: usize = 16_384;
/// Maximum accepted path length, in bytes, before any traversal begins.
pub const MAX_PATH_BYTES: usize = 4096;
/// Maximum accepted number of `/`-separated path segments.
pub const MAX_PATH_DEPTH: usize = 16;
/// Maximum bytes [`read_file_prefix`] will ever return, regardless of the
/// caller's requested `max_bytes`.
pub const MAX_FILE_PREFIX_BYTES: usize = 1024 * 1024;
/// Total block-device `read()` calls allowed for one [`list_root`],
/// [`find_path`], or [`read_file_prefix`] call. Each real directory entry
/// costs two reads (its fixed-size node, then its name), so this
/// comfortably covers [`MAX_ROOT_ENTRIES`] legitimate entries with a wide
/// safety margin, while still failing closed well before a corrupted or
/// cyclic tree could force meaningful CPU time.
const MAX_TRAVERSAL_READS: u32 = 40_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XdvdfsTraversalError {
    /// The volume descriptor magic/structure did not validate.
    InvalidVolume,
    /// A read fell outside the supplied bytes.
    OutOfBounds,
    /// The read-call budget was exhausted - see [`MAX_TRAVERSAL_READS`].
    TraversalBudgetExceeded,
    /// `path` exceeded [`MAX_PATH_BYTES`] or [`MAX_PATH_DEPTH`].
    PathTooLong,
    /// [`read_file_prefix`] was asked to read a directory.
    NotAFile,
    /// A path segment traversed through something that was not a
    /// directory.
    NotADirectory,
}

impl std::fmt::Display for XdvdfsTraversalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::InvalidVolume => "not a valid XDVDFS volume",
            Self::OutOfBounds => "read fell outside the supplied bytes",
            Self::TraversalBudgetExceeded => {
                "directory traversal read budget exceeded (possible corrupt/cyclic directory table)"
            }
            Self::PathTooLong => "path exceeded the traversal length/depth bound",
            Self::NotAFile => "requested path is a directory, not a file",
            Self::NotADirectory => "path traversed through a non-directory entry",
        };
        f.write_str(text)
    }
}

impl std::error::Error for XdvdfsTraversalError {}

/// One directory entry, as observed - never a claim about platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XdvdfsEntryFact {
    pub name: String,
    pub is_directory: bool,
    pub size: u32,
}

/// The (possibly truncated) root directory listing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct XdvdfsRootObservation {
    pub entries: Vec<XdvdfsEntryFact>,
    /// `true` when more entries existed than [`MAX_ROOT_ENTRIES`] allowed
    /// through - the list is a bounded prefix, not the full directory, in
    /// that case.
    pub truncated: bool,
}

/// A [`xdvdfs::blockdev::BlockDeviceRead`] over an in-memory byte slice with
/// a hard call budget - see the module documentation's cycle-protection
/// note. Never mutates `bytes`.
struct BoundedSliceReader<'a> {
    bytes: &'a [u8],
    reads_remaining: u32,
}

impl<'a> BoundedSliceReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            reads_remaining: MAX_TRAVERSAL_READS,
        }
    }
}

impl<'a> BlockDeviceRead<XdvdfsTraversalError> for BoundedSliceReader<'a> {
    fn read(&mut self, offset: u64, buffer: &mut [u8]) -> Result<(), XdvdfsTraversalError> {
        if self.reads_remaining == 0 {
            return Err(XdvdfsTraversalError::TraversalBudgetExceeded);
        }
        self.reads_remaining -= 1;

        let offset = usize::try_from(offset).map_err(|_| XdvdfsTraversalError::OutOfBounds)?;
        let end = offset
            .checked_add(buffer.len())
            .ok_or(XdvdfsTraversalError::OutOfBounds)?;
        let slice = self
            .bytes
            .get(offset..end)
            .ok_or(XdvdfsTraversalError::OutOfBounds)?;
        buffer.copy_from_slice(slice);
        Ok(())
    }
}

fn map_error(error: XdvdfsCrateError<XdvdfsTraversalError>) -> XdvdfsTraversalError {
    match error {
        XdvdfsCrateError::IOError(inner) => inner,
        XdvdfsCrateError::InvalidVolume => XdvdfsTraversalError::InvalidVolume,
        XdvdfsCrateError::IsNotDirectory => XdvdfsTraversalError::NotADirectory,
        XdvdfsCrateError::SizeOutOfBounds(_, _) => XdvdfsTraversalError::OutOfBounds,
        // DoesNotExist/NoDirent/DirectoryEmpty/StringEncodingError/
        // NameTooLong/InvalidFileName/TooManyDirectoryEntries/FileTooLarge/
        // SerializationFailed: none of these are ever surfaced from the
        // call sites in this module in a way callers here treat
        // differently from a generic "could not resolve this" outcome -
        // find_path/read_file_prefix intercept DoesNotExist/NoDirent
        // themselves (mapped to `Ok(None)`, never reaching this function)
        // before this fallback applies.
        _ => XdvdfsTraversalError::OutOfBounds,
    }
}

fn open_volume(
    bytes: &[u8],
) -> Result<(BoundedSliceReader<'_>, VolumeDescriptor), XdvdfsTraversalError> {
    let mut dev = BoundedSliceReader::new(bytes);
    let volume = read_volume(&mut dev).map_err(map_error)?;
    Ok((dev, volume))
}

fn validate_path(path: &str) -> Result<(), XdvdfsTraversalError> {
    if path.len() > MAX_PATH_BYTES {
        return Err(XdvdfsTraversalError::PathTooLong);
    }
    let depth = path.trim_start_matches('/').split_terminator('/').count();
    if depth > MAX_PATH_DEPTH {
        return Err(XdvdfsTraversalError::PathTooLong);
    }
    Ok(())
}

fn entry_fact(node: &xdvdfs::layout::DirectoryEntryNode) -> XdvdfsEntryFact {
    let name = node
        .name_str::<XdvdfsTraversalError>()
        .map(|cow| cow.into_owned())
        .unwrap_or_default();
    XdvdfsEntryFact {
        name,
        is_directory: node.node.dirent.is_directory(),
        size: node.node.dirent.data.size(),
    }
}

/// Every entry directly under the volume root, bounded and sorted by name
/// for deterministic output. `Err` only for a structurally invalid volume
/// or an exhausted read budget - never a panic.
pub fn list_root(bytes: &[u8]) -> Result<XdvdfsRootObservation, XdvdfsTraversalError> {
    let (mut dev, volume) = open_volume(bytes)?;
    list_root_generic(&mut dev, &volume)
}

/// Resolves one exact path from the volume root, case-insensitively (the
/// real XDVDFS on-disk ordering is already case-insensitive - see the
/// module documentation). `Ok(None)` for a well-formed volume that simply
/// does not contain `path`; `Err` for a structurally invalid volume,
/// oversized/over-deep path, or exhausted read budget.
pub fn find_path(
    bytes: &[u8],
    path: &str,
) -> Result<Option<XdvdfsEntryFact>, XdvdfsTraversalError> {
    validate_path(path)?;
    let (mut dev, volume) = open_volume(bytes)?;
    find_path_generic(&mut dev, &volume, path)
}

/// Reads up to `max_bytes` (itself capped at [`MAX_FILE_PREFIX_BYTES`])
/// from the start of the file at `path`. `Ok(None)` when `path` does not
/// exist; [`XdvdfsTraversalError::NotAFile`] when it resolves to a
/// directory. Never reads past the file's own declared size or the byte
/// bound, whichever is smaller - never a whole-file read for a large file.
pub fn read_file_prefix(
    bytes: &[u8],
    path: &str,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, XdvdfsTraversalError> {
    validate_path(path)?;
    let (mut dev, volume) = open_volume(bytes)?;
    read_file_prefix_generic(&mut dev, &volume, path, max_bytes)
}

// ---------------------------------------------------------------------------
// Generic core - shared by the in-memory (`bytes: &[u8]`) API above and the
// bounded, random-access, file-backed disc-image API below. Neither backend
// re-derives the traversal algorithm; both only supply a
// `BlockDeviceRead<XdvdfsTraversalError>` and get the identical, already-
// reviewed walk/bound behavior.
// ---------------------------------------------------------------------------

fn list_root_generic<D: BlockDeviceRead<XdvdfsTraversalError>>(
    dev: &mut D,
    volume: &VolumeDescriptor,
) -> Result<XdvdfsRootObservation, XdvdfsTraversalError> {
    let nodes = volume.root_table.walk_dirent_tree(dev).map_err(map_error)?;

    let truncated = nodes.len() > MAX_ROOT_ENTRIES;
    let mut entries: Vec<XdvdfsEntryFact> = nodes
        .iter()
        .take(MAX_ROOT_ENTRIES)
        .map(entry_fact)
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(XdvdfsRootObservation { entries, truncated })
}

fn find_path_generic<D: BlockDeviceRead<XdvdfsTraversalError>>(
    dev: &mut D,
    volume: &VolumeDescriptor,
    path: &str,
) -> Result<Option<XdvdfsEntryFact>, XdvdfsTraversalError> {
    match volume.root_table.walk_path(dev, path) {
        Ok(node) => Ok(Some(entry_fact(&node))),
        Err(XdvdfsCrateError::DoesNotExist | XdvdfsCrateError::NoDirent) => Ok(None),
        Err(error) => Err(map_error(error)),
    }
}

fn read_file_prefix_generic<D: BlockDeviceRead<XdvdfsTraversalError>>(
    dev: &mut D,
    volume: &VolumeDescriptor,
    path: &str,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, XdvdfsTraversalError> {
    let bound = max_bytes.min(MAX_FILE_PREFIX_BYTES);
    let node = match volume.root_table.walk_path(dev, path) {
        Ok(node) => node,
        Err(XdvdfsCrateError::DoesNotExist | XdvdfsCrateError::NoDirent) => return Ok(None),
        Err(error) => return Err(map_error(error)),
    };
    if node.node.dirent.is_directory() {
        return Err(XdvdfsTraversalError::NotAFile);
    }

    let file_size = node.node.dirent.data.size() as usize;
    let read_len = file_size.min(bound);
    let mut buf = vec![0u8; read_len];
    node.node
        .dirent
        .read_data(dev, &mut buf)
        .map_err(map_error)?;
    Ok(Some(buf))
}

// ---------------------------------------------------------------------------
// Bounded, random-access, file-backed disc-image API
// ---------------------------------------------------------------------------
//
// The functions above require the entire image as one in-memory `&[u8]` -
// fine for a small archive member, unsafe for a multi-gigabyte Xbox disc
// image. [`BoundedFileReader`] implements the same
// [`xdvdfs::blockdev::BlockDeviceRead`] contract over an already-open,
// caller-authorized [`File`] instead: every read is a bounded `seek` +
// `read_exact` for exactly the bytes the `xdvdfs` crate's own traversal
// algorithm asks for - never the whole file. The same [`MAX_TRAVERSAL_READS`]
// call budget applies, so a corrupt/cyclic directory table fails closed
// exactly as it does for the in-memory path.
//
// Wrapped in [`xdvdfs::blockdev::OffsetWrapper`] - the `xdvdfs` crate's own,
// already-published mechanism for locating the XDVDFS volume descriptor at
// one of the handful of well-known fixed byte offsets real Xbox disc dumps
// use: `0` (a raw/stripped XISO, e.g. `extract-xiso` output) or one of the
// three fixed XGD1/XGD2/XGD3 offsets a full, unstripped Redump-style disc
// dump (padding/security sectors included) places it at. This module invents
// no offset of its own; every value tried comes from the vetted upstream
// crate.

/// A [`BlockDeviceRead`] implementation over an already-open file - the
/// caller is responsible for having opened it safely (e.g. via
/// [`crate::safe_read::open_bounded_read`]); this module never opens a path
/// itself. Never materializes the whole file: every call is one bounded
/// `seek` + `read_exact`, and the same [`MAX_TRAVERSAL_READS`] call budget
/// as [`BoundedSliceReader`] applies.
struct BoundedFileReader<'a> {
    file: &'a mut File,
    reads_remaining: u32,
}

impl<'a> BoundedFileReader<'a> {
    fn new(file: &'a mut File) -> Self {
        Self {
            file,
            reads_remaining: MAX_TRAVERSAL_READS,
        }
    }
}

impl<'a> BlockDeviceRead<XdvdfsTraversalError> for BoundedFileReader<'a> {
    fn read(&mut self, offset: u64, buffer: &mut [u8]) -> Result<(), XdvdfsTraversalError> {
        if self.reads_remaining == 0 {
            return Err(XdvdfsTraversalError::TraversalBudgetExceeded);
        }
        self.reads_remaining -= 1;
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|_| XdvdfsTraversalError::OutOfBounds)?;
        self.file
            .read_exact(buffer)
            .map_err(|_| XdvdfsTraversalError::OutOfBounds)
    }
}

/// Opens the XDVDFS volume on `file`, trying each of the `xdvdfs` crate's
/// own well-known fixed offsets (see the module documentation) until one
/// validates. `Err(InvalidVolume)` when none do - never a guessed offset,
/// never a partial/best-effort volume.
fn open_disc_volume(
    file: &mut File,
) -> Result<
    (
        OffsetWrapper<BoundedFileReader<'_>, XdvdfsTraversalError>,
        VolumeDescriptor,
    ),
    XdvdfsTraversalError,
> {
    let dev = BoundedFileReader::new(file);
    let mut wrapper = OffsetWrapper::new(dev).map_err(|_| XdvdfsTraversalError::InvalidVolume)?;
    let volume = read_volume(&mut wrapper).map_err(map_error)?;
    Ok((wrapper, volume))
}

/// Every entry directly under a disc image's volume root - the file-backed,
/// offset-aware equivalent of [`list_root`]. Never reads the whole image.
pub fn list_root_in_disc_image(
    file: &mut File,
) -> Result<XdvdfsRootObservation, XdvdfsTraversalError> {
    let (mut dev, volume) = open_disc_volume(file)?;
    list_root_generic(&mut dev, &volume)
}

/// Resolves one exact path in a disc image, case-insensitively - the
/// file-backed, offset-aware equivalent of [`find_path`]. Never reads the
/// whole image.
pub fn find_path_in_disc_image(
    file: &mut File,
    path: &str,
) -> Result<Option<XdvdfsEntryFact>, XdvdfsTraversalError> {
    validate_path(path)?;
    let (mut dev, volume) = open_disc_volume(file)?;
    find_path_generic(&mut dev, &volume, path)
}

/// Reads a bounded prefix of one file in a disc image - the file-backed,
/// offset-aware equivalent of [`read_file_prefix`]. Never reads the whole
/// image and never reads past the file's own declared size or the byte
/// bound, whichever is smaller.
pub fn read_file_prefix_in_disc_image(
    file: &mut File,
    path: &str,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, XdvdfsTraversalError> {
    validate_path(path)?;
    let (mut dev, volume) = open_disc_volume(file)?;
    read_file_prefix_generic(&mut dev, &volume, path, max_bytes)
}

/// Test-only helpers for building minimal, hand-assembled XDVDFS images -
/// shared with [`crate::xbox_boot_evidence`]/[`crate::xbox360_boot_evidence`]'s
/// own tests so neither has to re-derive this module's on-disk layout.
#[cfg(test)]
pub(crate) mod test_support {
    const SECTOR: usize = xdvdfs::layout::SECTOR_SIZE as usize;

    fn pad_to_sector(buf: &mut Vec<u8>) {
        let rem = buf.len() % SECTOR;
        if rem != 0 {
            buf.resize(buf.len() + (SECTOR - rem), 0);
        }
    }

    /// A minimal valid XDVDFS image containing exactly one root file named
    /// `filename` with `content` as its bytes.
    pub(crate) fn synthetic_single_root_file_image(filename: &str, content: &[u8]) -> Vec<u8> {
        // Sector 32 is reserved for the volume descriptor (written last);
        // real content starts at sector 33.
        let mut image = vec![0u8; 33 * SECTOR];

        let file_sector = (image.len() / SECTOR) as u32;
        let mut padded = content.to_vec();
        pad_to_sector(&mut padded);
        image.extend_from_slice(&padded);

        let root_sector = (image.len() / SECTOR) as u32;
        let mut root_table = Vec::new();
        root_table.extend_from_slice(&0u16.to_le_bytes()); // left
        root_table.extend_from_slice(&0u16.to_le_bytes()); // right
        root_table.extend_from_slice(&file_sector.to_le_bytes());
        root_table.extend_from_slice(&(content.len() as u32).to_le_bytes());
        root_table.push(0x00); // attrs: not a directory
        root_table.push(filename.len() as u8);
        root_table.extend_from_slice(filename.as_bytes());
        while !root_table.len().is_multiple_of(4) {
            root_table.push(0);
        }
        pad_to_sector(&mut root_table);
        let root_table_len = root_table.len() as u32;
        image.extend_from_slice(&root_table);

        let mut volume = vec![0u8; SECTOR];
        volume[0..20].copy_from_slice(xdvdfs::layout::VOLUME_HEADER_MAGIC.as_slice());
        volume[20..24].copy_from_slice(&root_sector.to_le_bytes());
        volume[24..28].copy_from_slice(&root_table_len.to_le_bytes());
        volume[2028..2048].copy_from_slice(xdvdfs::layout::VOLUME_HEADER_MAGIC.as_slice());
        image[32 * SECTOR..33 * SECTOR].copy_from_slice(&volume);

        image
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // A hand-assembled, minimal-but-valid XDVDFS image: volume descriptor
    // at sector 32, a root directory table with two entries (one file, one
    // subdirectory), and the subdirectory's own table with one file. Sector
    // size is xdvdfs::layout::SECTOR_SIZE (2048).
    //
    // The directory table is a binary search tree ordered by
    // case-insensitive name, walked starting at table-relative byte offset
    // 0; `left_entry_offset`/`right_entry_offset` are `real_byte_offset /
    // 4`, and the crate treats the value `0` as "no child" - so the first
    // entry (real offset 0) can never validly be any other entry's child
    // target. Entries are appended with placeholder (0, 0) child pointers,
    // then patched afterward once every entry's real offset is known.
    const SECTOR: usize = xdvdfs::layout::SECTOR_SIZE as usize;

    fn pad_to_sector(buf: &mut Vec<u8>) {
        let rem = buf.len() % SECTOR;
        if rem != 0 {
            buf.resize(buf.len() + (SECTOR - rem), 0);
        }
    }

    /// Appends one directory entry node (fixed 0xe-byte struct + name,
    /// padded to a 4-byte boundary) with placeholder (0, 0) child pointers,
    /// returning its byte offset within the *directory table region* (not
    /// the whole image).
    fn append_dirent(
        table: &mut Vec<u8>,
        data_sector: u32,
        data_size: u32,
        is_dir: bool,
        name: &str,
    ) -> u32 {
        let start = table.len() as u32;
        table.extend_from_slice(&0u16.to_le_bytes()); // left_entry_offset placeholder
        table.extend_from_slice(&0u16.to_le_bytes()); // right_entry_offset placeholder
        table.extend_from_slice(&data_sector.to_le_bytes());
        table.extend_from_slice(&data_size.to_le_bytes());
        let attrs: u8 = if is_dir { 0x10 } else { 0x00 };
        table.push(attrs);
        table.push(name.len() as u8);
        table.extend_from_slice(name.as_bytes());
        while !table.len().is_multiple_of(4) {
            table.push(0);
        }
        start
    }

    /// Patches an already-appended entry's `left_entry_offset` (`field` =
    /// 0) or `right_entry_offset` (`field` = 1) to point at
    /// `child_byte_offset` (itself divided by 4, per the on-disk
    /// encoding).
    fn patch_child(table: &mut [u8], entry_byte_offset: u32, field: usize, child_byte_offset: u32) {
        let field_start = entry_byte_offset as usize + field * 2;
        table[field_start..field_start + 2]
            .copy_from_slice(&((child_byte_offset / 4) as u16).to_le_bytes());
    }

    /// Builds a minimal valid XDVDFS image with one root file
    /// (`DEFAULT.XBE`, `xbe_bytes` as its content) and one root
    /// subdirectory (`SUBDIR`) containing one file (`INNER.TXT`).
    fn synthetic_xdvdfs_image(xbe_bytes: &[u8]) -> Vec<u8> {
        // Sector 32 is reserved for the volume descriptor itself (written
        // at the end, into image[32*SECTOR..33*SECTOR]) - real content
        // must start at sector 33 or it collides with that write.
        let mut image = vec![0u8; 33 * SECTOR];

        // --- subdirectory table (one entry: INNER.TXT) ---
        let subdir_sector = (image.len() / SECTOR) as u32;
        let mut subdir_table = Vec::new();
        let inner_content = b"inner file contents";
        append_dirent(
            &mut subdir_table,
            0,
            inner_content.len() as u32,
            false,
            "INNER.TXT",
        );
        pad_to_sector(&mut subdir_table);
        let subdir_table_sector = subdir_sector;
        image.extend_from_slice(&subdir_table);

        // --- INNER.TXT content ---
        let inner_sector = (image.len() / SECTOR) as u32;
        let mut inner_padded = inner_content.to_vec();
        pad_to_sector(&mut inner_padded);
        image.extend_from_slice(&inner_padded);
        // patch the subdirectory table's data_sector field for INNER.TXT
        // (the only entry, at table-relative byte offset 0; data_sector is
        // at field offset 4 within one entry - see DirectoryEntryDiskNode).
        let inner_field_offset = subdir_table_sector as usize * SECTOR + 4;
        image[inner_field_offset..inner_field_offset + 4]
            .copy_from_slice(&inner_sector.to_le_bytes());

        // --- DEFAULT.XBE content ---
        let xbe_sector = (image.len() / SECTOR) as u32;
        let mut xbe_padded = xbe_bytes.to_vec();
        pad_to_sector(&mut xbe_padded);
        image.extend_from_slice(&xbe_padded);

        // --- root table (two entries: DEFAULT.XBE, SUBDIR) ---
        // "DEFAULT.XBE" < "SUBDIR" case-insensitively, so DEFAULT.XBE (the
        // first-appended, real-offset-0 entry) needs its right child
        // pointing at SUBDIR.
        let root_sector = (image.len() / SECTOR) as u32;
        let mut root_table = Vec::new();
        let default_xbe_off = append_dirent(
            &mut root_table,
            xbe_sector,
            xbe_bytes.len() as u32,
            false,
            "DEFAULT.XBE",
        );
        let subdir_off = append_dirent(
            &mut root_table,
            subdir_table_sector,
            subdir_table.len() as u32,
            true,
            "SUBDIR",
        );
        patch_child(&mut root_table, default_xbe_off, 1, subdir_off);
        pad_to_sector(&mut root_table);
        let root_table_len = root_table.len() as u32;
        image.extend_from_slice(&root_table);

        // --- volume descriptor at sector 32 ---
        let mut volume = vec![0u8; SECTOR];
        volume[0..20].copy_from_slice(xdvdfs::layout::VOLUME_HEADER_MAGIC.as_slice());
        volume[20..24].copy_from_slice(&root_sector.to_le_bytes());
        volume[24..28].copy_from_slice(&root_table_len.to_le_bytes());
        volume[2028..2048].copy_from_slice(xdvdfs::layout::VOLUME_HEADER_MAGIC.as_slice());
        image[32 * SECTOR..33 * SECTOR].copy_from_slice(&volume);

        image
    }

    #[test]
    fn root_listing_finds_both_entries() {
        let image = synthetic_xdvdfs_image(b"XBEH synthetic header bytes");
        let observation = list_root(&image).expect("valid synthetic volume");
        assert_eq!(observation.entries.len(), 2);
        assert!(!observation.truncated);
        let names: Vec<&str> = observation
            .entries
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert!(names.contains(&"DEFAULT.XBE"));
        assert!(names.contains(&"SUBDIR"));
    }

    #[test]
    fn root_listing_reports_directory_flag_correctly() {
        let image = synthetic_xdvdfs_image(b"content");
        let observation = list_root(&image).unwrap();
        let file = observation
            .entries
            .iter()
            .find(|e| e.name == "DEFAULT.XBE")
            .unwrap();
        assert!(!file.is_directory);
        let dir = observation
            .entries
            .iter()
            .find(|e| e.name == "SUBDIR")
            .unwrap();
        assert!(dir.is_directory);
    }

    #[test]
    fn find_path_locates_root_file() {
        let image = synthetic_xdvdfs_image(b"XBEH payload");
        let entry = find_path(&image, "DEFAULT.XBE").unwrap().unwrap();
        assert_eq!(entry.name, "DEFAULT.XBE");
        assert!(!entry.is_directory);
    }

    #[test]
    fn find_path_is_case_insensitive() {
        let image = synthetic_xdvdfs_image(b"XBEH payload");
        let entry = find_path(&image, "default.xbe").unwrap().unwrap();
        assert_eq!(entry.name, "DEFAULT.XBE");
    }

    #[test]
    fn find_path_locates_nested_file() {
        let image = synthetic_xdvdfs_image(b"XBEH payload");
        let entry = find_path(&image, "SUBDIR/INNER.TXT").unwrap().unwrap();
        assert_eq!(entry.name, "INNER.TXT");
    }

    #[test]
    fn find_path_missing_file_is_none_not_error() {
        let image = synthetic_xdvdfs_image(b"XBEH payload");
        assert_eq!(find_path(&image, "NOPE.BIN").unwrap(), None);
    }

    #[test]
    fn find_path_through_a_file_as_directory_fails_closed() {
        let image = synthetic_xdvdfs_image(b"XBEH payload");
        let result = find_path(&image, "DEFAULT.XBE/NOPE.BIN");
        assert_eq!(result, Err(XdvdfsTraversalError::NotADirectory));
    }

    #[test]
    fn read_file_prefix_reads_exact_small_file() {
        let image = synthetic_xdvdfs_image(b"XBEH payload bytes here");
        let data = read_file_prefix(&image, "DEFAULT.XBE", 4096)
            .unwrap()
            .unwrap();
        assert_eq!(data, b"XBEH payload bytes here");
    }

    #[test]
    fn read_file_prefix_truncates_to_requested_bound() {
        let image = synthetic_xdvdfs_image(b"XBEH payload bytes here");
        let data = read_file_prefix(&image, "DEFAULT.XBE", 4).unwrap().unwrap();
        assert_eq!(data, b"XBEH");
    }

    #[test]
    fn read_file_prefix_missing_file_is_none() {
        let image = synthetic_xdvdfs_image(b"content");
        assert_eq!(read_file_prefix(&image, "NOPE.BIN", 16).unwrap(), None);
    }

    #[test]
    fn read_file_prefix_on_a_directory_fails_closed() {
        let image = synthetic_xdvdfs_image(b"content");
        let result = read_file_prefix(&image, "SUBDIR", 16);
        assert_eq!(result, Err(XdvdfsTraversalError::NotAFile));
    }

    #[test]
    fn read_file_prefix_never_exceeds_the_hard_cap_even_if_requested() {
        let image = synthetic_xdvdfs_image(b"short");
        // Requesting far more than MAX_FILE_PREFIX_BYTES must not panic or
        // allocate anywhere near that - the actual file is tiny, so the
        // real bound exercised here is the min() with the file's own size,
        // but this also documents the cap exists and is applied first.
        let data = read_file_prefix(&image, "DEFAULT.XBE", usize::MAX)
            .unwrap()
            .unwrap();
        assert_eq!(data, b"short");
    }

    #[test]
    fn invalid_magic_fails_closed() {
        let mut image = vec![0u8; 33 * SECTOR];
        image[32 * SECTOR..32 * SECTOR + 20].copy_from_slice(b"not the xdvdfs magic");
        assert_eq!(list_root(&image), Err(XdvdfsTraversalError::InvalidVolume));
        assert_eq!(
            find_path(&image, "ANYTHING"),
            Err(XdvdfsTraversalError::InvalidVolume)
        );
    }

    #[test]
    fn truncated_image_fails_closed_not_panics() {
        // The `xdvdfs` crate's own `read_volume` maps every failure to read
        // the volume descriptor - out-of-bounds included - to
        // `InvalidVolume` (it does not distinguish "too short to even try"
        // from "wrong magic"), so that is what surfaces here too.
        let short = vec![0u8; 100];
        assert_eq!(list_root(&short), Err(XdvdfsTraversalError::InvalidVolume));
    }

    #[test]
    fn empty_image_fails_closed_not_panics() {
        assert_eq!(list_root(&[]), Err(XdvdfsTraversalError::InvalidVolume));
    }

    #[test]
    fn oversized_path_is_rejected_before_traversal() {
        let image = synthetic_xdvdfs_image(b"content");
        let long_path = "A".repeat(MAX_PATH_BYTES + 1);
        assert_eq!(
            find_path(&image, &long_path),
            Err(XdvdfsTraversalError::PathTooLong)
        );
    }

    #[test]
    fn overly_deep_path_is_rejected_before_traversal() {
        let image = synthetic_xdvdfs_image(b"content");
        let deep_path = "A/".repeat(MAX_PATH_DEPTH + 1) + "FILE.BIN";
        assert_eq!(
            find_path(&image, &deep_path),
            Err(XdvdfsTraversalError::PathTooLong)
        );
    }

    #[test]
    fn self_referential_directory_table_fails_closed_via_read_budget() {
        // Real offset 0 can never be a valid child target (the crate
        // treats offset `0` as the "no child" sentinel), so a genuine cycle
        // needs at least two entries: a real root entry (at offset 0)
        // whose right child points at a second "LOOP" entry, and LOOP's own
        // left AND right children both pointing back at itself. Each visit
        // to LOOP pushes two more (identical) visits to LOOP, so the
        // traversal stack grows without bound until MAX_TRAVERSAL_READS
        // trips - exactly the corruption class this budget exists to
        // catch.
        let mut image = vec![0u8; 33 * SECTOR];
        let root_sector = 33u32;
        image.resize(image.len() + SECTOR, 0);

        let mut root_table = Vec::new();
        let root_off = append_dirent(&mut root_table, 0, 0, false, "AAA.BIN");
        let loop_off = append_dirent(&mut root_table, 0, 0, false, "LOOP.BIN");
        patch_child(&mut root_table, root_off, 1, loop_off); // root.right -> LOOP
        patch_child(&mut root_table, loop_off, 0, loop_off); // LOOP.left -> LOOP
        patch_child(&mut root_table, loop_off, 1, loop_off); // LOOP.right -> LOOP
        pad_to_sector(&mut root_table);
        let root_table_len = root_table.len() as u32;
        let root_offset = root_sector as usize * SECTOR;
        image[root_offset..root_offset + root_table.len()].copy_from_slice(&root_table);

        let mut volume = vec![0u8; SECTOR];
        volume[0..20].copy_from_slice(xdvdfs::layout::VOLUME_HEADER_MAGIC.as_slice());
        volume[20..24].copy_from_slice(&root_sector.to_le_bytes());
        volume[24..28].copy_from_slice(&root_table_len.to_le_bytes());
        volume[2028..2048].copy_from_slice(xdvdfs::layout::VOLUME_HEADER_MAGIC.as_slice());
        image[32 * SECTOR..33 * SECTOR].copy_from_slice(&volume);

        let result = list_root(&image);
        assert_eq!(result, Err(XdvdfsTraversalError::TraversalBudgetExceeded));
    }

    #[test]
    fn empty_root_table_yields_empty_listing_not_error() {
        let mut image = vec![0u8; 33 * SECTOR];
        let root_sector = 33u32;
        image.resize(image.len() + SECTOR, 0);

        let mut volume = vec![0u8; SECTOR];
        volume[0..20].copy_from_slice(xdvdfs::layout::VOLUME_HEADER_MAGIC.as_slice());
        volume[20..24].copy_from_slice(&root_sector.to_le_bytes());
        volume[24..28].copy_from_slice(&0u32.to_le_bytes()); // zero-size root table
        volume[2028..2048].copy_from_slice(xdvdfs::layout::VOLUME_HEADER_MAGIC.as_slice());
        image[32 * SECTOR..33 * SECTOR].copy_from_slice(&volume);

        let observation = list_root(&image).unwrap();
        assert!(observation.entries.is_empty());
        assert!(!observation.truncated);
    }

    // ------------------------------------------------------------------
    // Bounded, random-access, file-backed disc-image API
    // ------------------------------------------------------------------

    /// The exact XGD1 offset the `xdvdfs` crate itself tries - a real,
    /// well-known fixed offset a full, unstripped Redump-style Xbox disc
    /// dump places its XDVDFS volume descriptor at. Not invented here.
    const XGD1_OFFSET: u64 = 405_798_912;

    /// Writes `bytes` into a fresh temp file starting at `offset`, leaving
    /// everything before `offset` as a sparse hole (never actually written
    /// to disk) - exactly what a real Redump-style dump's leading
    /// padding/security-sector region is, as far as this test cares: bytes
    /// this code must never need to read, let alone materialize.
    fn file_with_bytes_at(offset: u64, bytes: &[u8]) -> File {
        let mut file = tempfile::tempfile().unwrap();
        file.set_len(offset + bytes.len() as u64).unwrap();
        file.seek(SeekFrom::Start(offset)).unwrap();
        file.write_all(bytes).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        file
    }

    #[test]
    fn disc_image_raw_xiso_shape_resolves_default_xbe() {
        let mut file = file_with_bytes_at(0, &synthetic_xdvdfs_image(b"XBEH payload"));
        let entry = find_path_in_disc_image(&mut file, "default.xbe")
            .unwrap()
            .unwrap();
        assert_eq!(entry.name, "DEFAULT.XBE");
        assert!(!entry.is_directory);
        let data = read_file_prefix_in_disc_image(&mut file, "default.xbe", 4096)
            .unwrap()
            .unwrap();
        assert_eq!(data, b"XBEH payload");
    }

    #[test]
    fn disc_image_redump_style_offset_shape_resolves_default_xbe() {
        // The same content as the raw-XISO test above, but with the whole
        // volume shifted to the real XGD1 offset - simulating a full,
        // unstripped Redump-style dump. `OffsetWrapper` must find it without
        // this module guessing the offset itself.
        let mut file = file_with_bytes_at(XGD1_OFFSET, &synthetic_xdvdfs_image(b"XBEH payload"));
        let entry = find_path_in_disc_image(&mut file, "default.xbe")
            .unwrap()
            .unwrap();
        assert_eq!(entry.name, "DEFAULT.XBE");
        let data = read_file_prefix_in_disc_image(&mut file, "default.xbe", 4096)
            .unwrap()
            .unwrap();
        assert_eq!(data, b"XBEH payload");
    }

    #[test]
    fn disc_image_list_root_finds_both_entries() {
        let mut file = file_with_bytes_at(0, &synthetic_xdvdfs_image(b"content"));
        let observation = list_root_in_disc_image(&mut file).unwrap();
        assert_eq!(observation.entries.len(), 2);
        let names: Vec<&str> = observation
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert!(names.contains(&"DEFAULT.XBE"));
        assert!(names.contains(&"SUBDIR"));
    }

    #[test]
    fn disc_image_malformed_volume_fails_closed_at_every_known_offset() {
        // No valid magic at offset 0 or at any of the crate's other known
        // XGD offsets - must be refused, never a guessed/partial volume.
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"not an xdvdfs volume at all").unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        assert_eq!(
            find_path_in_disc_image(&mut file, "default.xbe"),
            Err(XdvdfsTraversalError::InvalidVolume)
        );
        assert_eq!(
            list_root_in_disc_image(&mut file),
            Err(XdvdfsTraversalError::InvalidVolume)
        );
    }

    #[test]
    fn disc_image_missing_default_xbe_is_none_not_error() {
        let image = synthetic_single_root_file_image_for_test("SOMETHING.ELSE", b"not an xbe");
        let mut file = file_with_bytes_at(0, &image);
        assert_eq!(
            find_path_in_disc_image(&mut file, "default.xbe").unwrap(),
            None
        );
    }

    #[test]
    fn disc_image_default_xbe_as_a_directory_fails_closed() {
        // A root entry named DEFAULT.XBE that is itself a directory (attrs
        // marks it as one) must never be treated as the boot executable.
        let mut image = vec![0u8; 33 * SECTOR];
        let root_sector = 33u32;
        let mut root_table = Vec::new();
        append_dirent(&mut root_table, 0, 0, true, "DEFAULT.XBE");
        pad_to_sector(&mut root_table);
        let root_table_len = root_table.len() as u32;
        image.resize(image.len() + SECTOR, 0);
        let root_offset = root_sector as usize * SECTOR;
        image[root_offset..root_offset + root_table.len()].copy_from_slice(&root_table);
        let mut volume = vec![0u8; SECTOR];
        volume[0..20].copy_from_slice(xdvdfs::layout::VOLUME_HEADER_MAGIC.as_slice());
        volume[20..24].copy_from_slice(&root_sector.to_le_bytes());
        volume[24..28].copy_from_slice(&root_table_len.to_le_bytes());
        volume[2028..2048].copy_from_slice(xdvdfs::layout::VOLUME_HEADER_MAGIC.as_slice());
        image[32 * SECTOR..33 * SECTOR].copy_from_slice(&volume);

        let mut file = file_with_bytes_at(0, &image);
        let entry = find_path_in_disc_image(&mut file, "default.xbe")
            .unwrap()
            .unwrap();
        assert!(entry.is_directory);
        assert_eq!(
            read_file_prefix_in_disc_image(&mut file, "default.xbe", 4096),
            Err(XdvdfsTraversalError::NotAFile)
        );
    }

    #[test]
    fn disc_image_huge_overflow_data_sector_is_refused_not_panicked() {
        // DEFAULT.XBE's data_sector field is patched to u32::MAX, a sector
        // number whose byte offset (far beyond the small backing file) can
        // never be satisfied - must fail closed with a real error, never a
        // panic or a silently truncated/garbage read.
        let mut image = synthetic_xdvdfs_image(b"XBEH payload");
        // Locate DEFAULT.XBE's dirent within the root table by scanning for
        // its own name bytes, then overwrite its data_sector field (the 4
        // bytes immediately preceding the name-length/name in the fixed
        // dirent layout: left(2) + right(2) + data_sector(4) + data_size(4)
        // + attrs(1) + name_len(1) + name).
        let needle = b"DEFAULT.XBE";
        let name_pos = image
            .windows(needle.len())
            .position(|window| window == needle)
            .expect("DEFAULT.XBE name bytes must be present in the synthetic image");
        // name is preceded by: name_len(1) + attrs(1) + data_size(4) +
        // data_sector(4) = 10 bytes.
        let data_sector_offset = name_pos - 10;
        image[data_sector_offset..data_sector_offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());

        let mut file = file_with_bytes_at(0, &image);
        let result = read_file_prefix_in_disc_image(&mut file, "default.xbe", 4096);
        assert_eq!(result, Err(XdvdfsTraversalError::OutOfBounds));
    }

    #[test]
    fn disc_image_never_materializes_the_whole_image() {
        // A multi-gigabyte sparse file (a hole - no real disk I/O for the
        // padding) with a real, valid XDVDFS volume placed only at the real
        // XGD1 offset. If this code ever tried to read the whole file into
        // memory, this would be a multi-gigabyte allocation; instead it
        // must resolve default.xbe using only the small number of bounded
        // reads the traversal genuinely needs.
        let mut file = file_with_bytes_at(XGD1_OFFSET, &synthetic_xdvdfs_image(b"XBEH payload"));
        // Extend well past any real Xbox disc (XGD3 alone is ~6.8 GB) to
        // make a whole-image read practically infeasible in a unit test if
        // this code ever regressed to doing one.
        let huge_len = XGD1_OFFSET + 8 * 1024 * 1024 * 1024;
        file.set_len(huge_len).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();

        let entry = find_path_in_disc_image(&mut file, "default.xbe")
            .unwrap()
            .unwrap();
        assert_eq!(entry.name, "DEFAULT.XBE");
        let data = read_file_prefix_in_disc_image(&mut file, "default.xbe", 4096)
            .unwrap()
            .unwrap();
        assert_eq!(data, b"XBEH payload");
    }

    /// A minimal valid XDVDFS image with exactly one root file - a thin
    /// local alias for [`test_support::synthetic_single_root_file_image`]
    /// (already `pub(crate)`, shared with `xbox_boot_evidence`/
    /// `xbox360_boot_evidence`'s own tests) so this module's disc-image
    /// tests can use it without a second, competing implementation.
    fn synthetic_single_root_file_image_for_test(filename: &str, content: &[u8]) -> Vec<u8> {
        test_support::synthetic_single_root_file_image(filename, content)
    }
}
