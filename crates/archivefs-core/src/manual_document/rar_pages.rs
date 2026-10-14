//! CBR: bounded inspection and on-demand page reading over the in-process
//! `rars` reader (RAR 1.3 through RAR 7). Nothing is ever extracted to disk and
//! no external tool is spawned: a page is decoded one member at a time into
//! memory under explicit output, workspace and dictionary budgets, size-checked,
//! then handed to the same image decoder CBZ uses.
//!
//! Policy mirrors `zip_pages`: the whole member table is validated before any
//! page is trusted, and a member that is unsafe, encrypted, nested, duplicated
//! or implausibly compressed refuses the document even if it is not a page.

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use rars::{ArchiveReadOptions, ArchiveReader, ExtractionDecision, ReadCancellation};

use super::detect::ManualFormatEvidence;
use super::order::{compare_page_names, page_group};
use super::zip_pages::{
    Classified, S_IFDIR, S_IFLNK, S_IFMT, S_IFREG, classify, sanitise_for_error,
    validate_member_name, within_expansion_ratio,
};
use super::{
    ArchiveLimitation, ManualDocumentId, ManualDocumentKind, ManualInspection, ManualLimits,
    ManualMetadata, ManualPage, ManualReadiness, ManualViewerError, ManualWarning,
    UnsafeMemberReason, open_regular,
};

/// Plaintext header bytes admitted for one parse, regardless of member count.
const MAX_HEADER_BYTES: u64 = 32 * 1024 * 1024;
/// Header slots beyond the member limit: main, end, service and comment headers.
const HEADER_SLACK: u64 = 32;
/// Decoder workspace (model state, filters, staging) for one page read.
const MAX_WORKSPACE_BYTES: u64 = 512 * 1024 * 1024;
/// Largest declared RAR 5/7 dictionary. Real comic archives use 4-128 MiB.
const MAX_DICTIONARY_BYTES: u64 = 256 * 1024 * 1024;

/// A one-way signal that stops an in-flight page read at the next decoder
/// checkpoint. Clone it to cancel from another thread; it cannot be reset.
#[derive(Clone, Debug, Default)]
pub struct ManualCancellation(ReadCancellation);

impl ManualCancellation {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.cancel();
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}

fn parse_options<'a>(
    limits: &ManualLimits,
    cancellation: Option<&'a ReadCancellation>,
) -> ArchiveReadOptions<'a> {
    let name_budget = (limits.max_name_bytes as u64).saturating_add(512);
    let header_bytes = name_budget
        .saturating_mul(limits.max_members as u64)
        .clamp(1024 * 1024, MAX_HEADER_BYTES);
    let mut options = ArchiveReadOptions::new()
        .with_max_header_count((limits.max_members as u64).saturating_add(HEADER_SLACK))
        .with_max_header_bytes(header_bytes);
    if let Some(token) = cancellation {
        options = options.with_cancellation(token);
    }
    options
}

fn read_options<'a>(
    limits: &ManualLimits,
    cancellation: Option<&'a ReadCancellation>,
) -> ArchiveReadOptions<'a> {
    // The per-member ceiling also governs solid predecessors that are decoded
    // and discarded, so it is the document-wide cap, not the selected page's
    // size. The returned page is checked against its inspected size below.
    let mut options = ArchiveReadOptions::new()
        .with_max_member_output_bytes(limits.max_member_bytes)
        // Solid predecessors are decoded and discarded; they count here.
        .with_max_total_output_bytes(limits.max_total_uncompressed_bytes)
        .with_max_reader_workspace_bytes(MAX_WORKSPACE_BYTES)
        .with_rar50_dictionary_size_limit(MAX_DICTIONARY_BYTES)
        .with_rar50_buffered_decode_limit(limits.max_member_bytes);
    if let Some(token) = cancellation {
        options = options.with_cancellation(token);
    }
    options
}

/// Translate a decoder error into the viewer's vocabulary. No decoder text is
/// shown to the user; the technical detail stays in `Malformed`/`Io`.
fn map_error(error: &rars::Error, name: Option<&str>) -> ManualViewerError {
    use rars::Error as E;
    use rars::ErrorKind as K;
    let name = || name.map(sanitise_for_error).unwrap_or_default();
    match error {
        E::MemberOutputLimitExceeded { .. } => {
            return ManualViewerError::SuspiciousCompression { name: name() };
        }
        E::TotalOutputLimitExceeded {
            limit, required, ..
        } => {
            return ManualViewerError::ArchiveTooLarge {
                bytes: *required,
                max: *limit,
            };
        }
        E::HeaderCountLimitExceeded { limit, required } => {
            return ManualViewerError::TooManyMembers {
                count: usize::try_from(*required).unwrap_or(usize::MAX),
                max: usize::try_from(*limit).unwrap_or(usize::MAX),
            };
        }
        E::FeatureDisabled { feature, .. } if *feature == "encryption" => {
            return ManualViewerError::UnsupportedArchive(ArchiveLimitation::Encrypted);
        }
        E::UnsupportedEncryption { .. } => {
            return ManualViewerError::UnsupportedArchive(ArchiveLimitation::Encrypted);
        }
        E::UnsupportedCompression { .. } => {
            return ManualViewerError::UnsupportedArchive(ArchiveLimitation::Compression);
        }
        E::HeaderBytesLimitExceeded { .. }
        | E::Rar50DictionaryLimitExceeded { .. }
        | E::Rar50BufferedDecodeLimitExceeded { .. }
        | E::Rar50FilterMemoryLimitExceeded { .. }
        | E::MemoryLimitExceeded { .. } => {
            return ManualViewerError::UnsupportedArchive(ArchiveLimitation::Resources);
        }
        _ => {}
    }
    match error.kind() {
        K::Cancelled => ManualViewerError::Cancelled,
        K::SourceChanged => ManualViewerError::SourceChanged,
        K::PasswordRequired | K::BadPassword => {
            ManualViewerError::UnsupportedArchive(ArchiveLimitation::Encrypted)
        }
        K::UnsupportedFormat => {
            ManualViewerError::UnsupportedArchive(ArchiveLimitation::FormatVersion)
        }
        K::UnsupportedFeature => ManualViewerError::UnsupportedArchive(ArchiveLimitation::Feature),
        K::ResourceLimit => ManualViewerError::UnsupportedArchive(ArchiveLimitation::Resources),
        K::UnsafePath => ManualViewerError::UnsafeMember {
            name: name(),
            reason: UnsafeMemberReason::Traversal,
        },
        K::Io => ManualViewerError::Io(error.to_string()),
        _ => ManualViewerError::Malformed(format!("not a readable RAR archive: {error}")),
    }
}

/// Windows reparse-point attribute: how DOS-host RAR archives mark links.
const DOS_REPARSE_POINT: u64 = 0x400;

/// How the member's attributes should be read, using the decoder's own host
/// grouping (RAR 1.5-4: hosts 3 and 5 are Unix; RAR 5: host 1 is Unix).
enum MemberKind {
    Plain,
    Link,
    Special,
}

fn member_kind(meta: &rars::ArchiveMemberMeta) -> MemberKind {
    match meta.attr_source() {
        rars::AttrSource::Unix => {
            let bits = (meta.file_attr & u64::from(u32::MAX)) as u32 & S_IFMT;
            match bits {
                0 | S_IFREG | S_IFDIR => MemberKind::Plain,
                S_IFLNK => MemberKind::Link,
                _ => MemberKind::Special,
            }
        }
        rars::AttrSource::Dos if meta.file_attr & DOS_REPARSE_POINT != 0 => MemberKind::Link,
        // Unknown hosts carry no usable attributes; nothing is ever extracted.
        _ => MemberKind::Plain,
    }
}

fn archive_flags(archive: &rars::Archive) -> (bool, bool) {
    // (multi-volume, encrypted headers)
    match archive {
        rars::Archive::Rar13(a) => (a.main.is_volume(), false),
        rars::Archive::Rar15To40(a) => (a.main.is_volume(), a.main.has_encrypted_headers()),
        rars::Archive::Rar50Plus(a) => (a.main.is_volume(), false),
        _ => (false, false),
    }
}

pub(super) fn inspect(
    file: File,
    id: ManualDocumentId,
    evidence: ManualFormatEvidence,
    limits: &ManualLimits,
) -> Result<ManualInspection, ManualViewerError> {
    let archive = ArchiveReader::read_reader_with_options(file, parse_options(limits, None))
        .map_err(|e| map_error(&e, None))?;
    let (volume, encrypted_headers) = archive_flags(&archive);
    if encrypted_headers {
        return Err(ManualViewerError::UnsupportedArchive(
            ArchiveLimitation::Encrypted,
        ));
    }
    if volume {
        return Err(ManualViewerError::UnsupportedArchive(
            ArchiveLimitation::MultiVolume,
        ));
    }
    if archive.sfx_offset() != 0 {
        return Err(ManualViewerError::UnsupportedArchive(
            ArchiveLimitation::SelfExtracting,
        ));
    }

    let mut seen = HashSet::new();
    let mut total: u64 = 0;
    let mut ignored = 0usize;
    let mut unsupported = 0usize;
    let mut first_unsupported: Option<String> = None;
    let mut pages: Vec<(String, super::ManualPageFormat, usize, u64, u64)> = Vec::new();

    for (index, member) in archive.members().enumerate() {
        if index >= limits.max_members {
            return Err(ManualViewerError::TooManyMembers {
                count: index + 1,
                max: limits.max_members,
            });
        }
        let meta = &member.meta;
        let raw_name = String::from_utf8_lossy(&meta.name).into_owned();
        let name = validate_member_name(&raw_name, limits).map_err(|reason| {
            ManualViewerError::UnsafeMember {
                name: sanitise_for_error(&raw_name),
                reason,
            }
        })?;
        if !seen.insert(name.clone()) {
            return Err(ManualViewerError::DuplicateMember { name });
        }
        if meta.is_redirection {
            return Err(ManualViewerError::UnsafeMember {
                name,
                reason: UnsafeMemberReason::Symlink,
            });
        }
        match member_kind(meta) {
            MemberKind::Plain => {}
            MemberKind::Link => {
                return Err(ManualViewerError::UnsafeMember {
                    name,
                    reason: UnsafeMemberReason::Symlink,
                });
            }
            MemberKind::Special => {
                return Err(ManualViewerError::UnsafeMember {
                    name,
                    reason: UnsafeMemberReason::SpecialFile,
                });
            }
        }
        if meta.is_encrypted {
            return Err(ManualViewerError::EncryptedMember { name });
        }
        if meta.is_split_before || meta.is_split_after {
            return Err(ManualViewerError::UnsupportedArchive(
                ArchiveLimitation::MultiVolume,
            ));
        }
        let size = meta.unpacked_size;
        let packed = meta.packed_size;
        if size > limits.max_member_bytes {
            return Err(ManualViewerError::MemberTooLarge {
                name,
                bytes: size,
                max: limits.max_member_bytes,
            });
        }
        total = total
            .checked_add(size)
            .filter(|t| *t <= limits.max_total_uncompressed_bytes)
            .ok_or(ManualViewerError::ArchiveTooLarge {
                bytes: total.saturating_add(size),
                max: limits.max_total_uncompressed_bytes,
            })?;
        if !within_expansion_ratio(size, packed, limits.max_expansion_ratio) {
            return Err(ManualViewerError::SuspiciousCompression { name });
        }
        if meta.is_directory {
            continue;
        }
        match classify(&name) {
            Classified::Page(format) => pages.push((name, format, index, packed, size)),
            Classified::UnsupportedImage => {
                unsupported += 1;
                first_unsupported.get_or_insert(name);
            }
            Classified::NestedArchive => return Err(ManualViewerError::NestedArchive { name }),
            Classified::Ignored => ignored += 1,
        }
        if pages.len() > limits.max_pages {
            return Err(ManualViewerError::TooManyPages {
                count: pages.len(),
                max: limits.max_pages,
            });
        }
    }
    if pages.is_empty() {
        return Err(match first_unsupported {
            Some(name) => ManualViewerError::UnsupportedImage { name },
            None => ManualViewerError::NoSupportedPages,
        });
    }
    pages.sort_by(|a, b| compare_page_names(&a.0, &b.0));
    let pages: Vec<ManualPage> = pages
        .into_iter()
        .enumerate()
        .map(
            |(position, (name, format, member_index, packed, size))| ManualPage {
                index: position,
                group: page_group(&name),
                name,
                format,
                member_index,
                compressed_bytes: packed,
                uncompressed_bytes: size,
            },
        )
        .collect();
    let mut warnings = Vec::new();
    if ignored > 0 {
        warnings.push(ManualWarning::IgnoredMembers(ignored));
    }
    if unsupported > 0 {
        warnings.push(ManualWarning::UnsupportedImageMembers(unsupported));
    }
    Ok(ManualInspection {
        id,
        kind: ManualDocumentKind::Cbr,
        evidence,
        readiness: ManualReadiness::Viewable,
        page_count: Some(pages.len()),
        pages,
        metadata: ManualMetadata::default(),
        active_content: Vec::new(),
        warnings,
    })
}

/// Collects the selected page and refuses the first byte past its inspected
/// size, so a header that understates the real output is caught while decoding
/// rather than after the whole member has been produced.
struct PageSink {
    bytes: Vec<u8>,
    cap: u64,
    overflowed: bool,
}

struct CappedPage(Arc<Mutex<PageSink>>);

impl Write for CappedPage {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut sink = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if sink.bytes.len() as u64 + buf.len() as u64 > sink.cap {
            sink.overflowed = true;
            return Err(io::Error::other("page output exceeds its declared size"));
        }
        sink.bytes.extend_from_slice(buf);
        #[cfg(test)]
        PEAK_ACCEPTED.with(|peak| peak.set(peak.get().max(sink.bytes.len() as u64)));
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
thread_local! {
    /// Most bytes any page sink accepted on this thread (test observation only).
    static PEAK_ACCEPTED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Decode member `index`, discarding solid predecessors, into a sink that holds
/// at most `cap` bytes. Mirrors `Archive::read_member_at_with_options` but with
/// a bounded destination for the selected member.
fn read_member_capped(
    archive: &rars::Archive,
    index: usize,
    cap: u64,
    name: &str,
    options: ArchiveReadOptions<'_>,
) -> Result<Option<Vec<u8>>, ManualViewerError> {
    let sink = Arc::new(Mutex::new(PageSink {
        bytes: Vec::new(),
        cap,
        overflowed: false,
    }));
    let solid = archive.is_solid();
    let mut current = 0usize;
    let result = archive.extract_with_control(options, |member| {
        let this = current;
        current += 1;
        if this > index {
            return Ok(ExtractionDecision::Stop);
        }
        if this != index {
            return Ok(
                if solid && !member.meta.is_directory && !member.meta.is_redirection {
                    ExtractionDecision::Extract(Box::new(io::sink()))
                } else {
                    ExtractionDecision::Skip
                },
            );
        }
        if member.meta.is_directory || member.meta.is_redirection {
            return Ok(ExtractionDecision::Stop);
        }
        Ok(ExtractionDecision::Extract(Box::new(CappedPage(
            Arc::clone(&sink),
        ))))
    });
    let mut sink = sink.lock().unwrap_or_else(|e| e.into_inner());
    if sink.overflowed {
        return Err(ManualViewerError::SuspiciousCompression {
            name: sanitise_for_error(name),
        });
    }
    result.map_err(|e| map_error(&e, Some(name)))?;
    Ok((current > index).then(|| std::mem::take(&mut sink.bytes)))
}

pub(super) fn read_page_bytes(
    inspection: &ManualInspection,
    index: usize,
    limits: &ManualLimits,
    cancellation: Option<&ManualCancellation>,
) -> Result<Vec<u8>, ManualViewerError> {
    let page = inspection
        .pages
        .get(index)
        .ok_or(ManualViewerError::PageOutOfRange {
            index,
            count: inspection.pages.len(),
        })?;
    let token = cancellation.map(|c| &c.0);
    if token.is_some_and(ReadCancellation::is_cancelled) {
        return Err(ManualViewerError::Cancelled);
    }
    let (file, metadata) = open_regular(&inspection.id.path)?;
    if !inspection.id.still_matches(&metadata) {
        return Err(ManualViewerError::SourceChanged);
    }
    let archive = ArchiveReader::read_reader_with_options(file, parse_options(limits, token))
        .map_err(|e| map_error(&e, None))?;
    // The member at this position must still be the one that was inspected.
    let member = archive
        .members()
        .nth(page.member_index)
        .ok_or(ManualViewerError::SourceChanged)?;
    let current = validate_member_name(&String::from_utf8_lossy(&member.meta.name), limits)
        .map_err(|_| ManualViewerError::SourceChanged)?;
    if current != page.name
        || member.meta.unpacked_size != page.uncompressed_bytes
        || member.meta.is_directory
        || member.meta.is_encrypted
        || member.meta.is_redirection
    {
        return Err(ManualViewerError::SourceChanged);
    }
    let bytes = read_member_capped(
        &archive,
        page.member_index,
        page.uncompressed_bytes,
        &page.name,
        read_options(limits, token),
    )?
    .ok_or(ManualViewerError::SourceChanged)?;
    if bytes.len() as u64 != page.uncompressed_bytes {
        return Err(ManualViewerError::SuspiciousCompression {
            name: sanitise_for_error(&page.name),
        });
    }
    // The file must not have been replaced while it was being decoded.
    let (_, after) = open_regular(&inspection.id.path)?;
    if !inspection.id.still_matches(&after) {
        return Err(ManualViewerError::SourceChanged);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
