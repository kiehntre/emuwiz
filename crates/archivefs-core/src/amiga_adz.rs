//! Bounded gzip decompression of `.adz` (compressed Amiga floppy image)
//! containers.
//!
//! `.adz` is not a new filesystem and not a new disk format: it is a plain
//! gzip wrapper around an ordinary `.adf` (raw Amiga floppy) image. This
//! module adds no ADF/AmigaDOS parsing of its own - it bounded-decompresses
//! the gzip stream to a private, unlinked temporary file and reuses
//! [`crate::amiga_disk::inspect_amiga_floppy`] for media evidence, exactly
//! as a raw `.adf` would be inspected.
//!
//! The decompressed logical image and the compressed container are kept as
//! separate evidence throughout: [`AdzInspection::container_path`] names the
//! `.adz` file itself, while every filesystem/identity fact comes from
//! [`AdzInspection::floppy`], whose own hashes/observations are the ADF
//! parser's, never the gzip container's bytes.

use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use flate2::read::MultiGzDecoder;

use crate::amiga_disk::{self, AmigaFloppyError, AmigaFloppyInspection};

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// Minimum input for the existing image reader: one 512-byte block. This
/// bound alone supplies no floppy evidence. Smaller output is refused before
/// the decompressed bytes are handed to the ADF parser.
const MIN_ADF_BYTES: u64 = 512;

/// Largest decompressed size accepted from a `.adz`. Comfortably above the
/// largest standard Amiga floppy the ADF parser supports (HD, 1_802_240
/// bytes) with headroom for a slightly oversized/nonstandard dump, while
/// firmly refusing a decompression-bomb-shaped stream. This is "a plausible
/// Amiga floppy image", not "any size an attacker could construct".
pub const MAX_ADZ_DECOMPRESSED_BYTES: u64 = 2 * 1024 * 1024;

/// Largest compressed `.adz` input this module will read into memory before
/// attempting decompression. Enforced on actual bytes read, independently
/// of filesystem metadata and the decompressed-output bound above.
pub const MAX_ADZ_COMPRESSED_BYTES: u64 = 8 * MAX_ADZ_DECOMPRESSED_BYTES;

/// Why a `.adz` could not be confirmed as a compressed Amiga floppy image.
/// Every variant means "not trusted as ADZ/ADF content", never "probably
/// fine".
#[derive(Debug)]
pub enum AdzError {
    /// The path could not be read.
    Io(String),
    /// A FIFO, device or directory is not an inspectable disk-image file.
    NotRegularFile,
    /// The opened object or original pathname changed during inspection;
    /// retry a stable copy.
    ChangedWhileReading,
    /// The container's own size exceeds [`MAX_ADZ_COMPRESSED_BYTES`] -
    /// refused before any byte is decompressed.
    ContainerTooLarge { limit: u64 },
    /// The first two bytes are not the gzip magic number - a plain, honest
    /// refusal rather than guessing at some other format.
    NotGzip,
    /// The gzip stream did not decode cleanly (truncated or corrupt).
    /// Fail-soft: this is never treated as a hard crash or as evidence the
    /// underlying `.adf` (if any) is itself invalid.
    MalformedGzip(String),
    /// A gzip header exceeds the decoder's implementation limits. This is
    /// unsupported inspection, not evidence of an RFC 1952 violation.
    UnsupportedGzip(String),
    /// Decompressed output exceeded [`MAX_ADZ_DECOMPRESSED_BYTES`] - refused
    /// as a decompression-bomb shape, not decoded further.
    DecompressedTooLarge { limit: u64 },
    /// Decompressed output is smaller than one Amiga boot block; cannot be
    /// a real ADF.
    DecompressedTooSmall,
    /// A private, unlinked temporary file (required by the existing
    /// path-based ADF parser) could not be created or unlinked.
    TempFileUnavailable(String),
    /// The decompressed bytes are not a valid Amiga floppy image -
    /// "compressed data is not a supported ADF", not a guess at what it
    /// might be instead. Carries the existing ADF parser's own refusal.
    NotAdf(AmigaFloppyError),
}

impl std::fmt::Display for AdzError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(detail) => write!(f, "{detail}"),
            Self::NotRegularFile => f.write_str("ADZ inspection requires a regular file"),
            Self::ChangedWhileReading => f.write_str("ADZ input changed while it was read"),
            Self::ContainerTooLarge { limit } => {
                write!(
                    f,
                    "compressed .adz exceeds the {limit}-byte container limit"
                )
            }
            Self::NotGzip => f.write_str("not a gzip-wrapped file"),
            Self::MalformedGzip(detail) => write!(f, "malformed gzip stream: {detail}"),
            Self::UnsupportedGzip(detail) => write!(f, "unsupported gzip header: {detail}"),
            Self::DecompressedTooLarge { limit } => write!(
                f,
                "decompressed output exceeds the {limit}-byte plausible-ADF limit"
            ),
            Self::DecompressedTooSmall => {
                f.write_str("decompressed output is smaller than one Amiga boot block")
            }
            Self::TempFileUnavailable(detail) => {
                write!(f, "could not stage decompressed bytes: {detail}")
            }
            Self::NotAdf(error) => write!(f, "compressed data is not a supported ADF: {error}"),
        }
    }
}
impl std::error::Error for AdzError {}

/// The complete, read-only result of inspecting one `.adz`. `container_path`
/// and `decompressed_bytes` describe the compressed wrapper; `floppy` is
/// exactly what [`amiga_disk::inspect_amiga_floppy`] would have produced
/// for the equivalent raw `.adf` - the same type, the same evidence
/// contract, nothing ADZ-specific added to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdzInspection {
    pub container_path: PathBuf,
    pub decompressed_bytes: u64,
    pub floppy: AmigaFloppyInspection,
}

/// Inspect `path` as a gzip-wrapped Amiga floppy candidate: bounded-decompress,
/// then reuse [`amiga_disk::inspect_amiga_floppy`] on the result.
/// Stages bytes only after obtaining an unlinked file, never mutates `path`, and never
/// claims a decompressed stream is an ADF beyond what the existing parser
/// itself validates. The descriptor and original container pathname are
/// rechecked after logical-image inspection; a subsequent replacement remains
/// possible. This is not a source lock or an atomic filesystem snapshot.
pub fn inspect_adz(path: &Path) -> Result<AdzInspection, AdzError> {
    inspect_adz_in_temp_dir(path, &std::env::temp_dir())
}

/// Reuse the ADF validator for an already bounded, complete in-memory member.
/// The staging inode is private and unlinked; no archive name becomes a path.
pub(crate) fn inspect_adf_member_bytes(bytes: &[u8]) -> Result<AmigaFloppyInspection, AdzError> {
    if bytes.len() as u64 > MAX_ADZ_DECOMPRESSED_BYTES {
        return Err(AdzError::DecompressedTooLarge {
            limit: MAX_ADZ_DECOMPRESSED_BYTES,
        });
    }
    let mut temp = open_private_temp_file(&std::env::temp_dir())
        .map_err(|error| AdzError::TempFileUnavailable(error.to_string()))?;
    temp.write_all(bytes)
        .map_err(|error| AdzError::Io(error.to_string()))?;
    temp.flush()
        .map_err(|error| AdzError::Io(error.to_string()))?;
    amiga_disk::inspect_amiga_floppy(&proc_self_fd_path(temp.as_raw_fd())).map_err(AdzError::NotAdf)
}

pub(crate) fn inspect_adz_in_temp_dir(
    path: &Path,
    temp_dir: &Path,
) -> Result<AdzInspection, AdzError> {
    // Nonblocking open also protects the type check against a path swapped
    // to a FIFO. Inspect and read the same descriptor, never reopen the path.
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| AdzError::Io(error.to_string()))?;
    let metadata = file
        .metadata()
        .map_err(|error| AdzError::Io(error.to_string()))?;
    if !metadata.is_file() {
        return Err(AdzError::NotRegularFile);
    }
    let source = amiga_disk::source_identity::OpenedSource::capture(&metadata);
    #[cfg(test)]
    amiga_disk::source_identity::test_support::checkpoint(
        path,
        amiga_disk::source_identity::test_support::Checkpoint::Captured,
    );
    let result = (|| {
        let compressed = read_compressed_file(&mut file, &metadata)?;
        if compressed.len() < GZIP_MAGIC.len() || compressed[..2] != GZIP_MAGIC {
            return Err(AdzError::NotGzip);
        }

        // RFC 1952 defines a gzip file as concatenated members. This decoder
        // checks every header, CRC and ISIZE, and rejects non-member suffixes.
        let decoder = MultiGzDecoder::new(compressed.as_slice());
        let mut decoded = Vec::new();
        // The extra byte bounds aggregate output across all members. Accepted
        // streams reach EOF, including the final member's trailer validation.
        if let Err(error) = decoder
            .take(MAX_ADZ_DECOMPRESSED_BYTES + 1)
            .read_to_end(&mut decoded)
        {
            // flate2 1.1.9 bounds optional filename/comment fields to 65535 bytes.
            // RFC 1952 has no such length restriction; do not call this corruption.
            if error.kind() == io::ErrorKind::InvalidInput
                && error.to_string() == "gzip header field too long"
            {
                return Err(AdzError::UnsupportedGzip(error.to_string()));
            }
            return Err(AdzError::MalformedGzip(error.to_string()));
        }
        if decoded.len() as u64 > MAX_ADZ_DECOMPRESSED_BYTES {
            return Err(AdzError::DecompressedTooLarge {
                limit: MAX_ADZ_DECOMPRESSED_BYTES,
            });
        }
        if (decoded.len() as u64) < MIN_ADF_BYTES {
            return Err(AdzError::DecompressedTooSmall);
        }

        let mut temp = open_private_temp_file(temp_dir)
            .map_err(|error| AdzError::TempFileUnavailable(error.to_string()))?;
        temp.write_all(&decoded)
            .map_err(|error| AdzError::Io(error.to_string()))?;
        temp.flush()
            .map_err(|error| AdzError::Io(error.to_string()))?;
        let temp_path = proc_self_fd_path(temp.as_raw_fd());
        let floppy = amiga_disk::inspect_amiga_floppy(&temp_path).map_err(AdzError::NotAdf)?;
        Ok(AdzInspection {
            container_path: path.to_path_buf(),
            decompressed_bytes: decoded.len() as u64,
            floppy,
        })
    })();
    #[cfg(test)]
    amiga_disk::source_identity::test_support::checkpoint(
        path,
        amiga_disk::source_identity::test_support::Checkpoint::BeforePublish,
    );
    if !source
        .unchanged(&file, path)
        .map_err(|error| AdzError::Io(error.to_string()))?
    {
        return Err(AdzError::ChangedWhileReading);
    }
    result
}

fn read_compressed_file(
    file: &mut File,
    metadata: &std::fs::Metadata,
) -> Result<Vec<u8>, AdzError> {
    if metadata.len() > MAX_ADZ_COMPRESSED_BYTES {
        return Err(AdzError::ContainerTooLarge {
            limit: MAX_ADZ_COMPRESSED_BYTES,
        });
    }
    let mut compressed = Vec::new();
    (&mut *file)
        .take(MAX_ADZ_COMPRESSED_BYTES + 1)
        .read_to_end(&mut compressed)
        .map_err(|error| AdzError::Io(error.to_string()))?;
    if compressed.len() as u64 > MAX_ADZ_COMPRESSED_BYTES {
        return Err(AdzError::ContainerTooLarge {
            limit: MAX_ADZ_COMPRESSED_BYTES,
        });
    }
    let after = file
        .metadata()
        .map_err(|error| AdzError::Io(error.to_string()))?;
    if metadata.len() != compressed.len() as u64
        || metadata.len() != after.len()
        || metadata.modified().ok() != after.modified().ok()
    {
        return Err(AdzError::ChangedWhileReading);
    }
    Ok(compressed)
}

fn proc_self_fd_path(fd: RawFd) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{fd}"))
}

/// Opens a private, unlinked read/write temporary file. Tries Linux `O_TMPFILE` first (the file is
/// anonymous from the instant it is created - there is no window in which
/// a named entry exists, and no separate delete step that could be skipped
/// on an error path); falls back to a create-then-immediately-unlink named
/// file only if the target filesystem does not support `O_TMPFILE`.
fn open_private_temp_file(dir: &Path) -> io::Result<File> {
    if let Ok(file) = open_tmpfile(dir) {
        return Ok(file);
    }
    open_unlink_immediately(dir)
}

fn open_tmpfile(dir: &Path) -> io::Result<File> {
    let dir_c = CString::new(dir.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "temp dir path has a NUL byte"))?;
    // SAFETY: `open` with `O_TMPFILE` on a directory path creates an
    // unnamed, already-unlinked inode inside that directory; the returned
    // fd is exclusively owned by the `File` constructed immediately below.
    let fd = unsafe {
        libc::open(
            dir_c.as_ptr(),
            libc::O_TMPFILE | libc::O_RDWR | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just returned by the successful `open` above.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn open_unlink_immediately(dir: &Path) -> io::Result<File> {
    for _ in 0..8 {
        let candidate = dir.join(format!(
            ".archivefs-adz-{}-{}.tmp",
            std::process::id(),
            random_u64()
        ));
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&candidate)
        {
            Ok(file) => {
                // Unlinked immediately after opening: the fd keeps the
                // inode alive, but no name ever remains for anything else
                // to see or race against past this point.
                std::fs::remove_file(&candidate)?;
                return Ok(file);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other(
        "could not create a unique private temp file",
    ))
}

fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
}

#[cfg(test)]
mod tests;
