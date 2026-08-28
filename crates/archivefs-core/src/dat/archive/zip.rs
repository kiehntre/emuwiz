//! Bounded, read-only ZIP-member hashing.

use std::fs::{File, Metadata};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use zip::ZipArchive;

use crate::safe_read::{TrustedRoots, open_bounded_read};

use super::hash::{MemberStreamError, hash_member_stream};
use super::limits::ArchiveLimits;
use super::zip_preflight::{ZipPreflightError, ZipPreflightInfo, preflight_zip};
use super::{
    ArchiveMemberEvidence, ArchiveMemberSource, ArchiveMemberSourceError, ArchiveMemberStatus,
    ArchivePassCompletion, ArchivePassOutcome, ArchivePassStopReason, ArchiveRunBudget,
};

const NESTED_ARCHIVE_EXTENSIONS: &[&[u8]] =
    &[b"zip", b"7z", b"rar", b"tar", b"gz", b"bz2", b"xz", b"zst"];

#[cfg(test)]
type AfterMemberHook = Box<dyn FnMut(&Path)>;

#[derive(Debug, Clone, PartialEq, Eq)]
struct OuterIdentity {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanoseconds: i64,
}

impl OuterIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            changed_seconds: metadata.ctime(),
            #[cfg(unix)]
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }
}

/// ZIP source opened under the normal trusted-root read policy.
pub struct ZipArchiveSource {
    archive_path: PathBuf,
    file: File,
    preflight: ZipPreflightInfo,
    limits: ArchiveLimits,
    identity: OuterIdentity,
    member_count: usize,
    #[cfg(test)]
    after_member: Option<AfterMemberHook>,
}

impl std::fmt::Debug for ZipArchiveSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ZipArchiveSource")
            .field("archive_path", &self.archive_path)
            .field("preflight", &self.preflight)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl ZipArchiveSource {
    pub fn open(
        path: &Path,
        trusted: &TrustedRoots,
        limits: ArchiveLimits,
        cancel: &AtomicBool,
    ) -> Result<Self, ArchiveMemberSourceError> {
        let safe =
            open_bounded_read(path, trusted).map_err(|error| ArchiveMemberSourceError::Open {
                detail: format!("read policy refused the ZIP: {error:?}"),
            })?;
        let len = safe.len();
        let mut file = safe.into_file();
        let identity = OuterIdentity::from_metadata(&file.metadata().map_err(|error| {
            ArchiveMemberSourceError::Open {
                detail: format!("could not identify ZIP: {error}"),
            }
        })?);

        let preflight = preflight_zip(&mut file, len, &limits, cancel).map_err(map_preflight)?;
        let archive =
            construct_archive(file).map_err(|error| ArchiveMemberSourceError::Corrupt {
                detail: format!("ZIP parser refused archive after preflight: {error}"),
            })?;
        if archive.len() > preflight.entry_count
            || archive.central_directory_start() != preflight.central_directory_offset
        {
            return Err(ArchiveMemberSourceError::Corrupt {
                detail: "ZIP parser and preflight disagree on the central directory".to_string(),
            });
        }
        let member_count = preflight
            .entries
            .iter()
            .filter(|entry| !entry.is_directory)
            .count();
        let file = archive.into_inner();

        Ok(Self {
            archive_path: path.to_path_buf(),
            file,
            preflight,
            limits,
            identity,
            member_count,
            #[cfg(test)]
            after_member: None,
        })
    }

    fn outer_identity_unchanged(&self) -> bool {
        // Match the identity of the object the trusted-root policy opened.
        // `metadata` intentionally follows an allowed symlink just as
        // `open_bounded_read` did; comparing symlink metadata here would make
        // every permitted symlink look changed after an otherwise clean pass.
        std::fs::metadata(&self.archive_path)
            .map(|metadata| OuterIdentity::from_metadata(&metadata) == self.identity)
            .unwrap_or(false)
    }

    fn member_metadata(&self, index: usize) -> MemberMetadata {
        let member = &self.preflight.entries[index];
        let raw = member.name_raw.clone();
        MemberMetadata {
            display: display_name(&raw),
            nested: is_nested_name(&raw),
            raw,
            logical_size: member.logical_size,
            compressed_size: member.compressed_size,
            expected_crc32: member.crc32,
            encrypted: member.flags & ((1 << 0) | (1 << 6) | (1 << 13)) != 0,
            unsupported_flags: member.flags & ((1 << 4) | (1 << 5)) != 0,
            method: member.method,
            data_start: member.data_start,
        }
    }

    fn evidence(
        &self,
        index: usize,
        metadata: &MemberMetadata,
        status: ArchiveMemberStatus,
        hashes: Option<super::ArchiveMemberHashes>,
    ) -> ArchiveMemberEvidence {
        ArchiveMemberEvidence {
            archive_path: self.archive_path.clone(),
            member_name_raw: metadata.raw.clone(),
            member_name_display: metadata.display.clone(),
            index,
            logical_size: metadata.logical_size,
            is_nested_archive: metadata.nested,
            status,
            hashes,
        }
    }
}

#[derive(Debug)]
struct MemberMetadata {
    raw: Vec<u8>,
    display: String,
    logical_size: u64,
    compressed_size: u64,
    expected_crc32: u32,
    encrypted: bool,
    unsupported_flags: bool,
    nested: bool,
    method: u16,
    data_start: u64,
}

impl ArchiveMemberSource for ZipArchiveSource {
    fn archive_format(&self) -> &'static str {
        "zip"
    }

    fn member_count(&self) -> usize {
        self.member_count
    }

    fn verify_all(
        &mut self,
        cancel: &AtomicBool,
        run_budget: &mut ArchiveRunBudget,
    ) -> ArchivePassOutcome {
        let mut members = Vec::with_capacity(self.member_count);
        let mut archive_logical = 0_u64;
        let mut completion = ArchivePassCompletion::Complete;

        for index in 0..self.preflight.entry_count {
            if self.preflight.entries[index].is_directory {
                continue;
            }
            if cancel.load(Ordering::Relaxed) {
                completion = ArchivePassCompletion::Incomplete {
                    reason: ArchivePassStopReason::Cancelled,
                };
                break;
            }
            let metadata = self.member_metadata(index);

            let immediate_status = if metadata.nested {
                Some(ArchiveMemberStatus::NestedArchive)
            } else if metadata.encrypted {
                Some(ArchiveMemberStatus::Encrypted)
            } else if metadata.unsupported_flags {
                Some(ArchiveMemberStatus::UnsupportedCodec {
                    method: "unsupported ZIP feature flags".to_string(),
                })
            } else if !matches!(metadata.method, 0 | 8) {
                Some(ArchiveMemberStatus::UnsupportedCodec {
                    method: format!("ZIP method {}", metadata.method),
                })
            } else if metadata.logical_size > self.limits.max_member_logical_bytes {
                Some(ArchiveMemberStatus::RefusedLimits {
                    reason: "member size",
                })
            } else if ratio_exceeded(
                metadata.logical_size,
                metadata.compressed_size,
                self.limits.max_compression_ratio,
            ) {
                Some(ArchiveMemberStatus::RefusedLimits {
                    reason: "compression ratio",
                })
            } else {
                None
            };
            if let Some(status) = immediate_status {
                members.push(self.evidence(index, &metadata, status, None));
                continue;
            }

            let Some(archive_after) = archive_logical.checked_add(metadata.logical_size) else {
                members.push(self.evidence(
                    index,
                    &metadata,
                    ArchiveMemberStatus::RefusedLimits {
                        reason: "archive logical budget",
                    },
                    None,
                ));
                continue;
            };
            if archive_after > self.limits.max_archive_logical_bytes {
                members.push(self.evidence(
                    index,
                    &metadata,
                    ArchiveMemberStatus::RefusedLimits {
                        reason: "archive logical budget",
                    },
                    None,
                ));
                continue;
            }
            if !run_budget.try_charge(metadata.logical_size) {
                members.push(self.evidence(
                    index,
                    &metadata,
                    ArchiveMemberStatus::RefusedLimits {
                        reason: "run logical budget",
                    },
                    None,
                ));
                completion = ArchivePassCompletion::Incomplete {
                    reason: ArchivePassStopReason::RunLogicalBudget,
                };
                break;
            }
            archive_logical = archive_after;

            let decoded = decode_and_hash_member(&mut self.file, &metadata, cancel);
            match decoded {
                Ok(hashed)
                    if hashed.bytes_read == 0
                        && metadata.logical_size == 0
                        && hashed.hashes.crc32 == format!("{:08x}", metadata.expected_crc32) =>
                {
                    // Opening and reading even an empty accepted member to EOF
                    // makes the ZIP reader validate its CRC. Empty evidence is
                    // intentionally not offered as a DAT hash in this slice.
                    members.push(self.evidence(
                        index,
                        &metadata,
                        ArchiveMemberStatus::EmptyFile,
                        None,
                    ));
                }
                Ok(hashed)
                    if hashed.bytes_read == metadata.logical_size
                        && hashed.hashes.crc32 == format!("{:08x}", metadata.expected_crc32) =>
                {
                    members.push(self.evidence(
                        index,
                        &metadata,
                        ArchiveMemberStatus::HashComplete,
                        Some(hashed.hashes),
                    ));
                }
                Ok(hashed) => members.push(self.evidence(
                    index,
                    &metadata,
                    ArchiveMemberStatus::Corrupt {
                        detail: format!(
                            "decoded {} bytes of the {} declared, or CRC32 disagreed",
                            hashed.bytes_read, metadata.logical_size,
                        ),
                    },
                    None,
                )),
                Err(MemberStreamError::Cancelled) => {
                    completion = ArchivePassCompletion::Incomplete {
                        reason: ArchivePassStopReason::Cancelled,
                    };
                    break;
                }
                Err(MemberStreamError::TooLarge { .. }) => members.push(self.evidence(
                    index,
                    &metadata,
                    ArchiveMemberStatus::Corrupt {
                        detail: "decoded bytes exceed declared size".to_string(),
                    },
                    None,
                )),
                Err(MemberStreamError::Io(detail)) => members.push(self.evidence(
                    index,
                    &metadata,
                    ArchiveMemberStatus::Corrupt { detail },
                    None,
                )),
            }
            #[cfg(test)]
            if let Some(after_member) = self.after_member.as_mut() {
                after_member(&self.archive_path);
            }
        }

        if !self.outer_identity_unchanged() {
            completion = ArchivePassCompletion::Incomplete {
                reason: ArchivePassStopReason::OuterFileChanged,
            };
        }
        ArchivePassOutcome {
            members,
            total_members: self.member_count,
            completion,
        }
    }
}

/// Why [`extract_sole_zip_member`] refused to return bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZipExtractError {
    Open(String),
    /// The preflight scan itself refused or found the ZIP corrupt - see
    /// [`super::zip_preflight::ZipPreflightError`].
    Preflight(ZipPreflightError),
    /// Zero, or more than one, non-directory member - this function is
    /// deliberately narrow (see its own doc comment) and never guesses which
    /// member a caller wants.
    MemberCountNotOne(usize),
    Refused(&'static str),
    Corrupt(String),
    Cancelled,
}

/// One safely materialised ZIP member.  The path is relative to the caller's
/// staging directory; no archive member is ever allowed to choose an
/// absolute or escaping destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipExtractedMember {
    pub relative_path: PathBuf,
    pub logical_size: u64,
}

/// Extracts every regular, non-directory member of a ZIP into an already
/// dedicated staging directory.  It reuses the same central-directory
/// preflight and bounded decoder as the archive audit reader.  The caller
/// remains responsible for deciding which extracted members are meaningful;
/// this function only provides the format-neutral path and resource safety
/// boundary.
pub fn extract_zip_members_to(
    path: &Path,
    trusted: &crate::safe_read::TrustedRoots,
    limits: &ArchiveLimits,
    cancel: &AtomicBool,
    destination: &Path,
) -> Result<Vec<ZipExtractedMember>, ZipExtractError> {
    let safe = open_bounded_read(path, trusted).map_err(|error| {
        ZipExtractError::Open(format!("read policy refused the ZIP: {error:?}"))
    })?;
    let file_len = safe.len();
    let mut file = safe.into_file();
    let preflight =
        preflight_zip(&mut file, file_len, limits, cancel).map_err(ZipExtractError::Preflight)?;

    std::fs::create_dir_all(destination)
        .map_err(|error| ZipExtractError::Open(error.to_string()))?;
    let mut names = std::collections::BTreeSet::new();
    let mut members = Vec::new();
    let mut total = 0_u64;
    for entry in &preflight.entries {
        let raw = &entry.name_raw;
        let name = std::str::from_utf8(raw)
            .map_err(|_| ZipExtractError::Refused("member name is not UTF-8"))?;
        let relative = PathBuf::from(name.trim_end_matches('/'));
        if relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::CurDir | std::path::Component::ParentDir
                )
            })
        {
            return Err(ZipExtractError::Refused("unsafe member path"));
        }
        if !names.insert(name.to_lowercase()) {
            return Err(ZipExtractError::Refused(
                "duplicate or case-colliding member path",
            ));
        }
        if entry.is_directory {
            continue;
        }
        // Unix creator metadata identifies symbolic links by the file-type
        // bits in the high half of external attributes.  Refuse them even if
        // a ZIP library would otherwise expose them as regular bytes.
        if (entry.version_made_by >> 8) == 3
            && ((entry.external_attributes >> 16) & 0xf000) == 0xa000
        {
            return Err(ZipExtractError::Refused("symbolic-link member"));
        }
        if entry.logical_size > limits.max_member_logical_bytes {
            return Err(ZipExtractError::Refused("member size"));
        }
        total = total
            .checked_add(entry.logical_size)
            .ok_or(ZipExtractError::Refused("archive logical size"))?;
        if total > limits.max_archive_logical_bytes {
            return Err(ZipExtractError::Refused("archive logical size"));
        }
        if entry.flags & ((1 << 0) | (1 << 6) | (1 << 13)) != 0 {
            return Err(ZipExtractError::Refused("encrypted member"));
        }
        if entry.flags & ((1 << 4) | (1 << 5)) != 0 || !matches!(entry.method, 0 | 8) {
            return Err(ZipExtractError::Refused("unsupported ZIP feature"));
        }
        if ratio_exceeded(
            entry.logical_size,
            entry.compressed_size,
            limits.max_compression_ratio,
        ) {
            return Err(ZipExtractError::Refused("compression ratio"));
        }
        members.push(ZipExtractedMember {
            relative_path: relative,
            logical_size: entry.logical_size,
        });
    }

    // All metadata and safety checks complete before the first output write.
    // A fresh staging directory can therefore be removed wholesale by the
    // caller on any decode or write error.
    let mut output = Vec::with_capacity(members.len());
    for entry in preflight.entries.iter().filter(|entry| !entry.is_directory) {
        check_cancel_extract(cancel)?;
        let name = std::str::from_utf8(&entry.name_raw)
            .map_err(|_| ZipExtractError::Refused("member name is not UTF-8"))?;
        let relative = PathBuf::from(name.trim_end_matches('/'));
        let target = destination.join(&relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| ZipExtractError::Open(error.to_string()))?;
        }
        file.seek(std::io::SeekFrom::Start(entry.data_start))
            .map_err(|error| ZipExtractError::Corrupt(error.to_string()))?;
        let packed = (&mut file).take(entry.compressed_size);
        let (bytes, read, crc) = if entry.method == 0 {
            read_bounded_with_crc(packed, entry.logical_size, cancel)?
        } else {
            let decoder = flate2::read::DeflateDecoder::new(packed);
            read_bounded_with_crc(decoder, entry.logical_size, cancel)?
        };
        if read != entry.logical_size || crc != entry.crc32 {
            return Err(ZipExtractError::Corrupt(
                "decoded size or CRC32 disagreed".to_string(),
            ));
        }
        let mut created = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|error| ZipExtractError::Open(error.to_string()))?;
        created
            .write_all(&bytes)
            .and_then(|_| created.sync_all())
            .map_err(|error| ZipExtractError::Open(error.to_string()))?;
        output.push(ZipExtractedMember {
            relative_path: relative,
            logical_size: entry.logical_size,
        });
    }
    Ok(output)
}

fn check_cancel_extract(cancel: &AtomicBool) -> Result<(), ZipExtractError> {
    if cancel.load(Ordering::Relaxed) {
        Err(ZipExtractError::Cancelled)
    } else {
        Ok(())
    }
}

/// Extracts the raw decoded bytes of the single non-directory member in the
/// ZIP archive at `path`, reusing exactly the same bounded metadata scan
/// ([`preflight_zip`]) and member-safety checks (`ArchiveLimits`, encrypted/
/// unsupported-codec refusal, compression-ratio refusal) that
/// [`ZipArchiveSource::verify_all`] already applies to every member it
/// hashes - the only difference is that this keeps the decoded bytes instead
/// of only their hash, and requires the archive to contain exactly one real
/// member.
///
/// This exists for `dat::updates`'s managed Redump game-DAT provider, which
/// must unwrap a single DAT/XML file from a ZIP-wrapped download without a
/// second, independent (and therefore independently-unsafe) ZIP-parsing
/// path. It is deliberately not a general extraction API: a ZIP with zero or
/// more than one real member is refused outright rather than guessing which
/// member is wanted.
pub fn extract_sole_zip_member(
    path: &Path,
    limits: &ArchiveLimits,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, ZipExtractError> {
    let mut file = File::open(path).map_err(|error| ZipExtractError::Open(error.to_string()))?;
    let file_len = file
        .metadata()
        .map_err(|error| ZipExtractError::Open(error.to_string()))?
        .len();
    let preflight =
        preflight_zip(&mut file, file_len, limits, cancel).map_err(ZipExtractError::Preflight)?;

    let real_entries: Vec<_> = preflight
        .entries
        .iter()
        .filter(|entry| !entry.is_directory)
        .collect();
    if real_entries.len() != 1 {
        return Err(ZipExtractError::MemberCountNotOne(real_entries.len()));
    }
    let entry = real_entries[0];

    let encrypted = entry.flags & ((1 << 0) | (1 << 6) | (1 << 13)) != 0;
    let unsupported_flags = entry.flags & ((1 << 4) | (1 << 5)) != 0;
    if encrypted {
        return Err(ZipExtractError::Refused("encrypted member"));
    }
    if unsupported_flags || !matches!(entry.method, 0 | 8) {
        return Err(ZipExtractError::Refused("unsupported ZIP feature"));
    }
    if entry.logical_size > limits.max_member_logical_bytes {
        return Err(ZipExtractError::Refused("member size"));
    }
    if ratio_exceeded(
        entry.logical_size,
        entry.compressed_size,
        limits.max_compression_ratio,
    ) {
        return Err(ZipExtractError::Refused("compression ratio"));
    }

    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(entry.data_start))
        .map_err(|error| ZipExtractError::Corrupt(error.to_string()))?;
    let packed = (&mut file).take(entry.compressed_size);
    let (bytes, bytes_read, crc32) = if entry.method == 0 {
        read_bounded_with_crc(packed, entry.logical_size, cancel)?
    } else {
        let decoder = flate2::read::DeflateDecoder::new(packed);
        read_bounded_with_crc(decoder, entry.logical_size, cancel)?
    };
    if bytes_read != entry.logical_size || crc32 != entry.crc32 {
        return Err(ZipExtractError::Corrupt(format!(
            "decoded {bytes_read} bytes of the {} declared, or CRC32 disagreed",
            entry.logical_size
        )));
    }
    Ok(bytes)
}

/// Reads `reader` fully, bounded to `max_bytes` (refused, not silently
/// truncated, the instant more would be read), returning the bytes, how many
/// were read, and their CRC32 - the caller compares this against the
/// preflighted central-directory value, exactly like every other member this
/// module decodes.
fn read_bounded_with_crc<R: Read>(
    mut reader: R,
    max_bytes: u64,
    cancel: &AtomicBool,
) -> Result<(Vec<u8>, u64, u32), ZipExtractError> {
    let mut buffer = Vec::new();
    let mut crc = crate::identity_source::hashing::Crc32::new();
    let mut chunk = [0_u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ZipExtractError::Cancelled);
        }
        let read = reader
            .read(&mut chunk)
            .map_err(|error| ZipExtractError::Corrupt(error.to_string()))?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > max_bytes {
            return Err(ZipExtractError::Refused("member size"));
        }
        crc.update(&chunk[..read]);
        buffer.extend_from_slice(&chunk[..read]);
    }
    Ok((buffer, total, crc.finish()))
}

fn construct_archive(file: File) -> zip::result::ZipResult<ZipArchive<File>> {
    #[cfg(test)]
    ZIP_ARCHIVE_NEW_CALLS.with(|calls| calls.set(calls.get() + 1));
    ZipArchive::new(file)
}

fn decode_and_hash_member(
    file: &mut File,
    metadata: &MemberMetadata,
    cancel: &AtomicBool,
) -> Result<super::hash::HashedMember, MemberStreamError> {
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(metadata.data_start))
        .map_err(|error| MemberStreamError::Io(error.to_string()))?;
    let packed = file.take(metadata.compressed_size);
    if metadata.method == 0 {
        hash_member_stream(packed, metadata.logical_size, cancel)
    } else {
        let mut decoder = flate2::read::DeflateDecoder::new(packed);
        let hashed = hash_member_stream(&mut decoder, metadata.logical_size, cancel)?;
        if decoder.total_in() != metadata.compressed_size {
            return Err(MemberStreamError::Io(
                "Deflate stream did not consume its declared packed range".to_string(),
            ));
        }
        Ok(hashed)
    }
}

#[cfg(test)]
thread_local! {
    static ZIP_ARCHIVE_NEW_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn map_preflight(error: ZipPreflightError) -> ArchiveMemberSourceError {
    match error {
        ZipPreflightError::Cancelled => ArchiveMemberSourceError::Cancelled,
        ZipPreflightError::Refused(reason) => ArchiveMemberSourceError::RefusedLimits { reason },
        ZipPreflightError::Corrupt(detail) => ArchiveMemberSourceError::Corrupt { detail },
    }
}

fn ratio_exceeded(logical: u64, compressed: u64, maximum: u64) -> bool {
    logical > 0
        && (compressed == 0
            || compressed
                .checked_mul(maximum)
                .is_none_or(|maximum_logical| logical > maximum_logical))
}

fn is_nested_name(raw: &[u8]) -> bool {
    let Some(dot) = raw.iter().rposition(|byte| *byte == b'.') else {
        return false;
    };
    let extension: Vec<_> = raw[dot + 1..].iter().map(u8::to_ascii_lowercase).collect();
    NESTED_ARCHIVE_EXTENSIONS.contains(&extension.as_slice())
}

fn display_name(raw: &[u8]) -> String {
    let mut display = String::new();
    for byte in raw {
        for escaped in byte.escape_ascii() {
            display.push(char::from(escaped));
        }
    }
    display
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::{AesMode, CompressionMethod, ZipWriter};

    fn trusted(root: &Path) -> TrustedRoots {
        TrustedRoots::from_paths(std::iter::once(root))
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8], CompressionMethod)]) {
        let file = File::create(path).unwrap();
        let mut writer = ZipWriter::new(file);
        for (name, bytes, method) in entries {
            writer
                .start_file(
                    *name,
                    SimpleFileOptions::default().compression_method(*method),
                )
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    fn verify(path: &Path, limits: ArchiveLimits) -> ArchivePassOutcome {
        let cancel = AtomicBool::new(false);
        let mut source =
            ZipArchiveSource::open(path, &trusted(path.parent().unwrap()), limits, &cancel)
                .unwrap();
        let mut budget = ArchiveRunBudget::new(u64::MAX);
        source.verify_all(&cancel, &mut budget)
    }

    fn bytes(path: &Path) -> Vec<u8> {
        std::fs::read(path).unwrap()
    }

    fn eocd_offset(data: &[u8]) -> usize {
        data.windows(4)
            .rposition(|bytes| bytes == b"PK\x05\x06")
            .unwrap()
    }

    fn central_offsets(data: &[u8]) -> Vec<usize> {
        data.windows(4)
            .enumerate()
            .filter_map(|(offset, signature)| (signature == b"PK\x01\x02").then_some(offset))
            .collect()
    }

    fn local_offsets(data: &[u8]) -> Vec<usize> {
        data.windows(4)
            .enumerate()
            .filter_map(|(offset, signature)| (signature == b"PK\x03\x04").then_some(offset))
            .collect()
    }

    #[test]
    fn stored_and_deflated_members_hash_to_eof() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("games.zip");
        write_zip(
            &path,
            &[
                ("stored.rom", b"stored bytes", CompressionMethod::Stored),
                (
                    "deflated.rom",
                    b"deflated bytes",
                    CompressionMethod::Deflated,
                ),
            ],
        );
        let outcome = verify(&path, ArchiveLimits::default());
        assert!(outcome.is_complete());
        assert_eq!(outcome.total_members, 2);
        assert_eq!(outcome.members.len(), 2);
        assert!(
            outcome
                .members
                .iter()
                .all(|member| member.is_hash_complete())
        );
        assert_eq!(
            outcome.members[0].hashes.as_ref().unwrap().md5,
            "ead0eb0586ff4f57deffee2548fd7960"
        );
    }

    #[test]
    fn zip64_entry_metadata_is_preflighted_and_hashed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zip64.zip");
        let mut writer = ZipWriter::new(File::create(&path).unwrap());
        writer
            .start_file(
                "large-marked.rom",
                SimpleFileOptions::default()
                    .compression_method(CompressionMethod::Stored)
                    .large_file(true),
            )
            .unwrap();
        writer.write_all(b"small fixture").unwrap();
        writer.finish().unwrap();
        let outcome = verify(&path, ArchiveLimits::default());
        assert_eq!(outcome.members[0].status, ArchiveMemberStatus::HashComplete);
    }

    #[test]
    fn zip64_end_record_is_bounded_before_parser_construction() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zip64-eocd.zip");
        write_zip(&path, &[("game.rom", b"rom", CompressionMethod::Stored)]);
        let data = bytes(&path);
        let eocd = eocd_offset(&data);
        let classic = &data[eocd..];
        let central_size = u32::from_le_bytes(data[eocd + 12..eocd + 16].try_into().unwrap());
        let central_offset = u32::from_le_bytes(data[eocd + 16..eocd + 20].try_into().unwrap());
        let record_offset = eocd as u64;
        let mut rebuilt = data[..eocd].to_vec();
        rebuilt.extend_from_slice(b"PK\x06\x06");
        rebuilt.extend_from_slice(&44_u64.to_le_bytes());
        rebuilt.extend_from_slice(&45_u16.to_le_bytes());
        rebuilt.extend_from_slice(&45_u16.to_le_bytes());
        rebuilt.extend_from_slice(&0_u32.to_le_bytes());
        rebuilt.extend_from_slice(&0_u32.to_le_bytes());
        rebuilt.extend_from_slice(&1_u64.to_le_bytes());
        rebuilt.extend_from_slice(&1_u64.to_le_bytes());
        rebuilt.extend_from_slice(&u64::from(central_size).to_le_bytes());
        rebuilt.extend_from_slice(&u64::from(central_offset).to_le_bytes());
        rebuilt.extend_from_slice(b"PK\x06\x07");
        rebuilt.extend_from_slice(&0_u32.to_le_bytes());
        rebuilt.extend_from_slice(&record_offset.to_le_bytes());
        rebuilt.extend_from_slice(&1_u32.to_le_bytes());
        let classic_start = rebuilt.len();
        rebuilt.extend_from_slice(classic);
        rebuilt[classic_start + 8..classic_start + 12].fill(0xff);
        rebuilt[classic_start + 12..classic_start + 20].fill(0xff);
        std::fs::write(&path, rebuilt).unwrap();

        let outcome = verify(&path, ArchiveLimits::default());
        assert_eq!(outcome.members[0].status, ArchiveMemberStatus::HashComplete);
    }

    #[test]
    fn duplicate_names_are_distinguished_by_index_and_order_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("duplicates.zip");
        write_zip(
            &path,
            &[
                ("same1.rom", b"first", CompressionMethod::Stored),
                ("same2.rom", b"second", CompressionMethod::Deflated),
            ],
        );
        let mut data = bytes(&path);
        let second_local = local_offsets(&data)[1];
        let second_central = central_offsets(&data)[1];
        data[second_local + 30 + 4] = b'1';
        data[second_central + 46 + 4] = b'1';
        std::fs::write(&path, data).unwrap();
        let first = verify(&path, ArchiveLimits::default());
        let second = verify(&path, ArchiveLimits::default());
        assert_eq!(first, second);
        assert_eq!(
            first.members[0].member_name_raw,
            first.members[1].member_name_raw
        );
        assert_eq!(
            first
                .members
                .iter()
                .map(|member| member.index)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_ne!(first.members[0].hashes, first.members[1].hashes);
    }

    #[test]
    fn raw_non_utf8_name_is_lossless_and_display_is_separate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("raw.zip");
        write_zip(&path, &[("badx.rom", b"rom", CompressionMethod::Stored)]);
        let mut data = bytes(&path);
        for offset in local_offsets(&data) {
            data[offset + 30 + 3] = 0xff;
            data[offset + 6] &= !8;
        }
        for offset in central_offsets(&data) {
            data[offset + 46 + 3] = 0xff;
            data[offset + 8] &= !8;
        }
        std::fs::write(&path, data).unwrap();
        let outcome = verify(&path, ArchiveLimits::default());
        assert_eq!(outcome.members[0].member_name_raw, b"bad\xff.rom");
        assert_eq!(outcome.members[0].member_name_display, "bad\\xff.rom");
    }

    #[test]
    fn encrypted_member_is_refused_without_decryption() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("encrypted.zip");
        let mut writer = ZipWriter::new(File::create(&path).unwrap());
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .with_aes_encryption(AesMode::Aes256, "secret");
        writer.start_file("game.rom", options).unwrap();
        writer.write_all(b"payload").unwrap();
        writer.finish().unwrap();
        let outcome = verify(&path, ArchiveLimits::default());
        assert_eq!(outcome.members[0].status, ArchiveMemberStatus::Encrypted);
        assert!(outcome.members[0].hashes.is_none());
    }

    #[test]
    fn unsupported_codec_and_nested_archive_are_refused_independently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refusals.zip");
        write_zip(
            &path,
            &[
                ("codec.rom", b"payload", CompressionMethod::Stored),
                ("inner.zip", b"nested", CompressionMethod::Stored),
                ("good.rom", b"good", CompressionMethod::Stored),
            ],
        );
        let mut data = bytes(&path);
        let local = local_offsets(&data)[0];
        let central = central_offsets(&data)[0];
        data[local + 8..local + 10].copy_from_slice(&12_u16.to_le_bytes());
        data[central + 10..central + 12].copy_from_slice(&12_u16.to_le_bytes());
        std::fs::write(&path, data).unwrap();
        let outcome = verify(&path, ArchiveLimits::default());
        assert!(
            outcome.is_complete(),
            "independent ZIP members should continue"
        );
        assert!(matches!(
            outcome.members[0].status,
            ArchiveMemberStatus::UnsupportedCodec { .. }
        ));
        assert_eq!(
            outcome.members[1].status,
            ArchiveMemberStatus::NestedArchive
        );
        assert_eq!(outcome.members[2].status, ArchiveMemberStatus::HashComplete);
    }

    #[test]
    fn corrupt_crc_has_no_hashes_and_later_member_remains_visible() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crc.zip");
        write_zip(
            &path,
            &[
                ("bad.rom", b"bad", CompressionMethod::Stored),
                ("good.rom", b"good", CompressionMethod::Stored),
            ],
        );
        let mut data = bytes(&path);
        let local = local_offsets(&data)[0];
        let central = central_offsets(&data)[0];
        data[local + 14..local + 18].copy_from_slice(&0_u32.to_le_bytes());
        data[central + 16..central + 20].copy_from_slice(&0_u32.to_le_bytes());
        std::fs::write(&path, data).unwrap();
        let outcome = verify(&path, ArchiveLimits::default());
        assert!(outcome.is_complete());
        assert!(matches!(
            outcome.members[0].status,
            ArchiveMemberStatus::Corrupt { .. }
        ));
        assert!(outcome.members[0].hashes.is_none());
        assert_eq!(outcome.members[1].status, ArchiveMemberStatus::HashComplete);
    }

    #[test]
    fn member_size_exact_limit_passes_and_just_over_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sizes.zip");
        write_zip(
            &path,
            &[
                ("exact.rom", b"1234", CompressionMethod::Stored),
                ("over.rom", b"12345", CompressionMethod::Stored),
            ],
        );
        let limits = ArchiveLimits {
            max_member_logical_bytes: 4,
            ..ArchiveLimits::default()
        };
        let outcome = verify(&path, limits);
        assert_eq!(outcome.members[0].status, ArchiveMemberStatus::HashComplete);
        assert_eq!(
            outcome.members[1].status,
            ArchiveMemberStatus::RefusedLimits {
                reason: "member size"
            }
        );
    }

    #[test]
    fn archive_logical_budget_accepts_exact_total_and_refuses_just_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive-budget.zip");
        write_zip(
            &path,
            &[
                ("one.rom", b"1234", CompressionMethod::Stored),
                ("two.rom", b"5678", CompressionMethod::Stored),
            ],
        );
        let exact = verify(
            &path,
            ArchiveLimits {
                max_archive_logical_bytes: 8,
                ..ArchiveLimits::default()
            },
        );
        assert!(exact.members.iter().all(|member| member.is_hash_complete()));

        let over = verify(
            &path,
            ArchiveLimits {
                max_archive_logical_bytes: 4,
                ..ArchiveLimits::default()
            },
        );
        assert_eq!(over.members[0].status, ArchiveMemberStatus::HashComplete);
        assert_eq!(
            over.members[1].status,
            ArchiveMemberStatus::RefusedLimits {
                reason: "archive logical budget"
            }
        );
        assert!(
            over.is_complete(),
            "ZIP can inspect later independent metadata"
        );
    }

    #[test]
    fn short_decode_is_corrupt_and_never_exposes_prefix_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("short.zip");
        write_zip(&path, &[("short.rom", b"abc", CompressionMethod::Stored)]);
        let mut data = bytes(&path);
        let local = local_offsets(&data)[0];
        let central = central_offsets(&data)[0];
        data[local + 22..local + 26].copy_from_slice(&4_u32.to_le_bytes());
        data[central + 24..central + 28].copy_from_slice(&4_u32.to_le_bytes());
        std::fs::write(&path, data).unwrap();
        let outcome = verify(&path, ArchiveLimits::default());
        assert!(matches!(
            outcome.members[0].status,
            ArchiveMemberStatus::Corrupt { .. }
        ));
        assert!(outcome.members[0].hashes.is_none());
    }

    #[test]
    fn compression_ratio_and_zero_packed_size_are_refused_before_decode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ratio.zip");
        write_zip(
            &path,
            &[("bomb.rom", &[0_u8; 100], CompressionMethod::Deflated)],
        );
        let limits = ArchiveLimits {
            max_compression_ratio: 1,
            ..ArchiveLimits::default()
        };
        let outcome = verify(&path, limits);
        assert_eq!(
            outcome.members[0].status,
            ArchiveMemberStatus::RefusedLimits {
                reason: "compression ratio"
            }
        );

        let exact_path = dir.path().join("ratio-exact.zip");
        write_zip(
            &exact_path,
            &[("exact.rom", b"1234", CompressionMethod::Stored)],
        );
        let exact = verify(
            &exact_path,
            ArchiveLimits {
                max_compression_ratio: 1,
                ..ArchiveLimits::default()
            },
        );
        assert_eq!(exact.members[0].status, ArchiveMemberStatus::HashComplete);

        let zero_path = dir.path().join("zero-pack.zip");
        write_zip(
            &zero_path,
            &[("declared.rom", b"", CompressionMethod::Stored)],
        );
        let mut data = bytes(&zero_path);
        let local = local_offsets(&data)[0];
        let central = central_offsets(&data)[0];
        data[local + 22..local + 26].copy_from_slice(&1_u32.to_le_bytes());
        data[central + 24..central + 28].copy_from_slice(&1_u32.to_le_bytes());
        std::fs::write(&zero_path, data).unwrap();
        let outcome = verify(&zero_path, ArchiveLimits::default());
        assert_eq!(
            outcome.members[0].status,
            ArchiveMemberStatus::RefusedLimits {
                reason: "compression ratio"
            }
        );
    }

    #[test]
    fn archive_and_run_budgets_keep_partial_results_visible() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("budget.zip");
        write_zip(
            &path,
            &[
                ("one.rom", b"1234", CompressionMethod::Stored),
                ("two.rom", b"5678", CompressionMethod::Stored),
            ],
        );
        let cancel = AtomicBool::new(false);
        let mut source = ZipArchiveSource::open(
            &path,
            &trusted(dir.path()),
            ArchiveLimits::default(),
            &cancel,
        )
        .unwrap();
        let mut run = ArchiveRunBudget::new(4);
        let outcome = source.verify_all(&cancel, &mut run);
        assert_eq!(outcome.members.len(), 2);
        assert_eq!(outcome.members[0].status, ArchiveMemberStatus::HashComplete);
        assert_eq!(
            outcome.members[1].status,
            ArchiveMemberStatus::RefusedLimits {
                reason: "run logical budget"
            }
        );
        assert_eq!(
            outcome.completion,
            ArchivePassCompletion::Incomplete {
                reason: ArchivePassStopReason::RunLogicalBudget
            }
        );
    }

    #[test]
    fn preflight_rejects_hostile_count_and_central_bounds_before_zip_parser() {
        let dir = tempfile::tempdir().unwrap();
        for (name, mutate) in [("count.zip", 0_u8), ("bounds.zip", 1_u8)] {
            let path = dir.path().join(name);
            write_zip(&path, &[("a.rom", b"a", CompressionMethod::Stored)]);
            let mut data = bytes(&path);
            let eocd = eocd_offset(&data);
            if mutate == 0 {
                data[eocd + 8..eocd + 10].copy_from_slice(&5000_u16.to_le_bytes());
                data[eocd + 10..eocd + 12].copy_from_slice(&5000_u16.to_le_bytes());
            } else {
                let past_eof = data.len() as u32;
                data[eocd + 16..eocd + 20].copy_from_slice(&past_eof.to_le_bytes());
            }
            std::fs::write(&path, data).unwrap();
            let before = ZIP_ARCHIVE_NEW_CALLS.with(std::cell::Cell::get);
            let result = ZipArchiveSource::open(
                &path,
                &trusted(dir.path()),
                ArchiveLimits::default(),
                &AtomicBool::new(false),
            );
            assert!(result.is_err());
            assert_eq!(ZIP_ARCHIVE_NEW_CALLS.with(std::cell::Cell::get), before);
        }
    }

    #[test]
    fn truncated_zip_and_overlapping_packed_range_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let truncated = dir.path().join("truncated.zip");
        std::fs::write(&truncated, b"PK\x03\x04short").unwrap();
        assert!(
            ZipArchiveSource::open(
                &truncated,
                &trusted(dir.path()),
                ArchiveLimits::default(),
                &AtomicBool::new(false)
            )
            .is_err()
        );

        let path = dir.path().join("overlap.zip");
        write_zip(&path, &[("a.rom", b"abc", CompressionMethod::Stored)]);
        let mut data = bytes(&path);
        let central = central_offsets(&data)[0];
        data[central + 20..central + 24].copy_from_slice(&(central as u32).to_le_bytes());
        std::fs::write(&path, data).unwrap();
        assert!(
            ZipArchiveSource::open(
                &path,
                &trusted(dir.path()),
                ArchiveLimits::default(),
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }

    #[test]
    fn preset_cancellation_produces_incomplete_pass_without_members() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cancel.zip");
        write_zip(&path, &[("a.rom", b"abc", CompressionMethod::Stored)]);
        let mut source = ZipArchiveSource::open(
            &path,
            &trusted(dir.path()),
            ArchiveLimits::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let cancel = AtomicBool::new(true);
        let outcome = source.verify_all(&cancel, &mut ArchiveRunBudget::new(u64::MAX));
        assert!(outcome.members.is_empty());
        assert_eq!(
            outcome.completion,
            ArchivePassCompletion::Incomplete {
                reason: ArchivePassStopReason::Cancelled
            }
        );
    }

    #[test]
    fn active_cancellation_keeps_completed_member_evidence_visible() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cancel-after-one.zip");
        write_zip(
            &path,
            &[
                ("one.rom", b"first", CompressionMethod::Stored),
                ("two.rom", b"second", CompressionMethod::Deflated),
            ],
        );
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let mut source = ZipArchiveSource::open(
            &path,
            &trusted(dir.path()),
            ArchiveLimits::default(),
            &cancel,
        )
        .unwrap();
        let set_cancel = cancel.clone();
        source.after_member = Some(Box::new(move |_| {
            set_cancel.store(true, Ordering::Relaxed);
        }));
        let outcome = source.verify_all(&cancel, &mut ArchiveRunBudget::new(u64::MAX));
        assert_eq!(outcome.members.len(), 1);
        assert_eq!(outcome.members[0].status, ArchiveMemberStatus::HashComplete);
        assert_eq!(
            outcome.completion,
            ArchivePassCompletion::Incomplete {
                reason: ArchivePassStopReason::Cancelled
            }
        );
    }

    #[test]
    fn replaced_outer_archive_invalidates_the_whole_pass() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("replace.zip");
        write_zip(&path, &[("a.rom", b"abc", CompressionMethod::Stored)]);
        let cancel = AtomicBool::new(false);
        let mut source = ZipArchiveSource::open(
            &path,
            &trusted(dir.path()),
            ArchiveLimits::default(),
            &cancel,
        )
        .unwrap();
        let replacement = dir.path().join("replacement.zip");
        write_zip(
            &replacement,
            &[("b.rom", b"xyz", CompressionMethod::Stored)],
        );
        source.after_member = Some(Box::new(move |archive_path| {
            std::fs::rename(&replacement, archive_path).unwrap();
        }));
        let outcome = source.verify_all(&cancel, &mut ArchiveRunBudget::new(u64::MAX));
        assert_eq!(
            outcome.completion,
            ArchivePassCompletion::Incomplete {
                reason: ArchivePassStopReason::OuterFileChanged
            }
        );
    }

    #[test]
    fn verification_never_writes_or_rewrites_the_archive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("readonly.zip");
        write_zip(&path, &[("a.rom", b"abc", CompressionMethod::Deflated)]);
        let before = bytes(&path);
        let names_before: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        let _ = verify(&path, ArchiveLimits::default());
        let names_after: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(bytes(&path), before);
        assert_eq!(names_after, names_before);
    }

    // --- extract_sole_zip_member ---------------------------------------------------------------

    #[test]
    fn extract_sole_zip_member_returns_the_one_members_exact_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sole.zip");
        write_zip(
            &path,
            &[("only.dat", b"the dat content", CompressionMethod::Stored)],
        );
        let extracted =
            extract_sole_zip_member(&path, &ArchiveLimits::default(), &AtomicBool::new(false))
                .unwrap();
        assert_eq!(extracted, b"the dat content");
    }

    #[test]
    fn extract_sole_zip_member_decodes_deflated_content_too() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deflated.zip");
        let content = b"deflated dat content repeated repeated repeated".to_vec();
        write_zip(
            &path,
            &[("only.dat", &content, CompressionMethod::Deflated)],
        );
        let extracted =
            extract_sole_zip_member(&path, &ArchiveLimits::default(), &AtomicBool::new(false))
                .unwrap();
        assert_eq!(extracted, content);
    }

    #[test]
    fn extract_sole_zip_member_refuses_zero_members() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.zip");
        write_zip(&path, &[]);
        let error =
            extract_sole_zip_member(&path, &ArchiveLimits::default(), &AtomicBool::new(false))
                .unwrap_err();
        assert_eq!(error, ZipExtractError::MemberCountNotOne(0));
    }

    #[test]
    fn extract_sole_zip_member_refuses_more_than_one_member_rather_than_guessing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("two.zip");
        write_zip(
            &path,
            &[
                ("first.dat", b"first", CompressionMethod::Stored),
                ("second.dat", b"second", CompressionMethod::Stored),
            ],
        );
        let error =
            extract_sole_zip_member(&path, &ArchiveLimits::default(), &AtomicBool::new(false))
                .unwrap_err();
        assert_eq!(error, ZipExtractError::MemberCountNotOne(2));
    }

    #[test]
    fn extract_sole_zip_member_refuses_an_encrypted_member() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("encrypted.zip");
        let mut writer = ZipWriter::new(File::create(&path).unwrap());
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .with_aes_encryption(AesMode::Aes256, "secret");
        writer.start_file("only.dat", options).unwrap();
        writer.write_all(b"payload").unwrap();
        writer.finish().unwrap();
        let error =
            extract_sole_zip_member(&path, &ArchiveLimits::default(), &AtomicBool::new(false))
                .unwrap_err();
        assert_eq!(error, ZipExtractError::Refused("encrypted member"));
    }

    #[test]
    fn extract_sole_zip_member_refuses_a_member_over_the_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oversized.zip");
        write_zip(&path, &[("only.dat", b"12345", CompressionMethod::Stored)]);
        let limits = ArchiveLimits {
            max_member_logical_bytes: 4,
            ..ArchiveLimits::default()
        };
        let error = extract_sole_zip_member(&path, &limits, &AtomicBool::new(false)).unwrap_err();
        assert_eq!(error, ZipExtractError::Refused("member size"));
    }

    #[test]
    fn extract_sole_zip_member_refuses_an_excessive_compression_ratio() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bomb.zip");
        write_zip(
            &path,
            &[("only.dat", &[0_u8; 100], CompressionMethod::Deflated)],
        );
        let limits = ArchiveLimits {
            max_compression_ratio: 1,
            ..ArchiveLimits::default()
        };
        let error = extract_sole_zip_member(&path, &limits, &AtomicBool::new(false)).unwrap_err();
        assert_eq!(error, ZipExtractError::Refused("compression ratio"));
    }

    #[test]
    fn extract_sole_zip_member_rejects_a_corrupt_crc() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("badcrc.zip");
        write_zip(&path, &[("only.dat", b"good", CompressionMethod::Stored)]);
        let mut data = bytes(&path);
        let local = local_offsets(&data)[0];
        let central = central_offsets(&data)[0];
        data[local + 14..local + 18].copy_from_slice(&0_u32.to_le_bytes());
        data[central + 16..central + 20].copy_from_slice(&0_u32.to_le_bytes());
        std::fs::write(&path, data).unwrap();
        let error =
            extract_sole_zip_member(&path, &ArchiveLimits::default(), &AtomicBool::new(false))
                .unwrap_err();
        assert!(matches!(error, ZipExtractError::Corrupt(_)));
    }

    #[test]
    fn extract_sole_zip_member_rejects_a_truncated_zip_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("truncated.zip");
        std::fs::write(&path, b"PK\x03\x04short").unwrap();
        let error =
            extract_sole_zip_member(&path, &ArchiveLimits::default(), &AtomicBool::new(false))
                .unwrap_err();
        assert!(matches!(error, ZipExtractError::Preflight(_)));
    }

    #[test]
    fn extract_sole_zip_member_never_writes_or_rewrites_the_archive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("readonly.zip");
        write_zip(&path, &[("only.dat", b"abc", CompressionMethod::Stored)]);
        let before = bytes(&path);
        let _ = extract_sole_zip_member(&path, &ArchiveLimits::default(), &AtomicBool::new(false));
        assert_eq!(bytes(&path), before);
    }
}
