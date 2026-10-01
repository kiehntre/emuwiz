//! CBZ: bounded inspection and on-demand page reading over the workspace's
//! existing `zip` dependency. Nothing is ever extracted to disk; a page is read
//! one member at a time into memory, size-checked, then decoded.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom};

use super::detect::ManualFormatEvidence;
use super::order::{compare_page_names, page_group};
use super::{
    ManualDocumentId, ManualDocumentKind, ManualInspection, ManualLimits, ManualMetadata,
    ManualPage, ManualPageFormat, ManualReadiness, ManualViewerError, ManualWarning,
    UnsafeMemberReason, open_regular, read_bounded,
};

const S_IFMT: u32 = 0o170_000;
const S_IFREG: u32 = 0o100_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFLNK: u32 = 0o120_000;

/// A decoded page, ready for a texture upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualPageImage {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, RGBA8.
    pub rgba: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
enum Classified {
    Page(ManualPageFormat),
    UnsupportedImage,
    NestedArchive,
    Ignored,
}

/// Validate and normalise an archive member path. Returns forward-slash form.
pub(super) fn validate_member_name(
    raw: &str,
    limits: &ManualLimits,
) -> Result<String, UnsafeMemberReason> {
    if raw.is_empty() {
        return Err(UnsafeMemberReason::EmptyName);
    }
    if raw.len() > limits.max_name_bytes {
        return Err(UnsafeMemberReason::NameTooLong);
    }
    if raw.chars().any(|c| c.is_control()) {
        return Err(UnsafeMemberReason::ControlCharacter);
    }
    let normalised = raw.replace('\\', "/");
    if normalised.starts_with('/') {
        return Err(if normalised.starts_with("//") {
            UnsafeMemberReason::DriveOrUnc
        } else {
            UnsafeMemberReason::AbsolutePath
        });
    }
    let bytes = normalised.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(UnsafeMemberReason::DriveOrUnc);
    }
    let mut kept = Vec::new();
    for component in normalised.split('/') {
        match component {
            ".." => return Err(UnsafeMemberReason::Traversal),
            "." | "" => {}
            other => kept.push(other),
        }
    }
    if kept.is_empty() {
        return Err(UnsafeMemberReason::EmptyName);
    }
    Ok(kept.join("/"))
}

fn classify(name: &str) -> Classified {
    let file = name.rsplit('/').next().unwrap_or(name);
    // macOS resource-fork debris carries image extensions but is not an image.
    if name.starts_with("__MACOSX/") || file.starts_with('.') {
        return Classified::Ignored;
    }
    let ext = file
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => Classified::Page(ManualPageFormat::Png),
        "jpg" | "jpeg" => Classified::Page(ManualPageFormat::Jpeg),
        "webp" => Classified::Page(ManualPageFormat::Webp),
        "gif" | "bmp" | "tif" | "tiff" | "avif" | "heic" | "heif" | "jxl" | "jp2" | "ico" => {
            Classified::UnsupportedImage
        }
        "zip" | "cbz" | "rar" | "cbr" | "7z" | "cb7" | "tar" | "gz" | "tgz" | "bz2" | "xz" => {
            Classified::NestedArchive
        }
        _ => Classified::Ignored,
    }
}

/// The member count the archive's own end-of-central-directory record
/// declares, including ZIP64. The `zip` crate indexes members by name, so it
/// silently collapses duplicates: its `len()` can be smaller than what the file
/// actually lists. Comparing the two exposes hidden duplicates and keeps the
/// member-count bound honest.
fn declared_entry_count(file: &mut File) -> Result<Option<u64>, ManualViewerError> {
    const EOCD_LEN: usize = 22;
    const MAX_COMMENT: u64 = 65_535;
    let io = |e: std::io::Error| ManualViewerError::Io(e.to_string());
    let len = file.seek(SeekFrom::End(0)).map_err(io)?;
    let span = len.min(EOCD_LEN as u64 + MAX_COMMENT);
    let start = len - span;
    file.seek(SeekFrom::Start(start)).map_err(io)?;
    let mut tail = Vec::with_capacity(span as usize);
    (&mut *file).take(span).read_to_end(&mut tail).map_err(io)?;
    let Some(at) = (0..=tail.len().saturating_sub(EOCD_LEN)).rev().find(|&i| {
        tail[i..].starts_with(b"PK\x05\x06")
            && tail.len() >= i + EOCD_LEN
            && i + EOCD_LEN + usize::from(u16::from_le_bytes([tail[i + 20], tail[i + 21]]))
                == tail.len()
    }) else {
        return Ok(None);
    };
    let total = u64::from(u16::from_le_bytes([tail[at + 10], tail[at + 11]]));
    if total != 0xFFFF {
        return Ok(Some(total));
    }
    // ZIP64: the locator sits just before the EOCD and points at the real record.
    if at < 20 || !tail[at - 20..].starts_with(b"PK\x06\x07") {
        return Ok(None);
    }
    let record_at = u64::from_le_bytes(tail[at - 12..at - 4].try_into().expect("8 bytes"));
    if record_at.checked_add(56).is_none_or(|end| end > len) {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(record_at)).map_err(io)?;
    let mut record = [0u8; 56];
    file.read_exact(&mut record).map_err(io)?;
    if !record.starts_with(b"PK\x06\x06") {
        return Ok(None);
    }
    Ok(Some(u64::from_le_bytes(
        record[32..40].try_into().expect("8 bytes"),
    )))
}

pub(super) fn inspect(
    mut file: File,
    id: ManualDocumentId,
    evidence: ManualFormatEvidence,
    limits: &ManualLimits,
) -> Result<ManualInspection, ManualViewerError> {
    let declared = declared_entry_count(&mut file)?;
    // The bound applies to what the file *declares*, before anything is indexed.
    if let Some(count) = declared.filter(|c| *c > limits.max_members as u64) {
        return Err(ManualViewerError::TooManyMembers {
            count: usize::try_from(count).unwrap_or(usize::MAX),
            max: limits.max_members,
        });
    }
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| ManualViewerError::Malformed(format!("not a readable ZIP: {e}")))?;
    if archive.len() > limits.max_members {
        return Err(ManualViewerError::TooManyMembers {
            count: archive.len(),
            max: limits.max_members,
        });
    }
    if declared.is_some_and(|count| count > archive.len() as u64) {
        return Err(ManualViewerError::DuplicateMember {
            name: "(the archive lists a name more than once)".into(),
        });
    }
    let mut seen = HashSet::new();
    let mut total: u64 = 0;
    let mut ignored = 0usize;
    let mut unsupported = 0usize;
    let mut first_unsupported: Option<String> = None;
    let mut pages: Vec<(String, ManualPageFormat, usize, u64, u64)> = Vec::new();

    for index in 0..archive.len() {
        let entry = archive
            .by_index_raw(index)
            .map_err(|e| ManualViewerError::Malformed(format!("bad ZIP member: {e}")))?;
        let raw_name = entry.name().to_string();
        let name = validate_member_name(&raw_name, limits).map_err(|reason| {
            ManualViewerError::UnsafeMember {
                name: sanitise_for_error(&raw_name),
                reason,
            }
        })?;
        if let Some(mode) = entry.unix_mode() {
            let kind = mode & S_IFMT;
            if kind == S_IFLNK {
                return Err(ManualViewerError::UnsafeMember {
                    name,
                    reason: UnsafeMemberReason::Symlink,
                });
            }
            if kind != 0 && kind != S_IFREG && kind != S_IFDIR {
                return Err(ManualViewerError::UnsafeMember {
                    name,
                    reason: UnsafeMemberReason::SpecialFile,
                });
            }
        }
        if entry.is_dir() {
            continue;
        }
        if entry.encrypted() {
            return Err(ManualViewerError::EncryptedMember { name });
        }
        if !seen.insert(name.clone()) {
            return Err(ManualViewerError::DuplicateMember { name });
        }
        let size = entry.size();
        let packed = entry.compressed_size();
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
        if size > limits.ratio_floor_bytes
            && (packed == 0 || size / packed > limits.max_expansion_ratio)
        {
            return Err(ManualViewerError::SuspiciousCompression { name });
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
        kind: ManualDocumentKind::Cbz,
        evidence,
        readiness: ManualReadiness::Viewable,
        page_count: Some(pages.len()),
        pages,
        metadata: ManualMetadata::default(),
        active_content: Vec::new(),
        warnings,
    })
}

/// Names go into error text; keep them printable and short.
fn sanitise_for_error(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .take(120)
        .collect()
}

pub(super) fn read_page_bytes(
    inspection: &ManualInspection,
    index: usize,
    limits: &ManualLimits,
) -> Result<Vec<u8>, ManualViewerError> {
    let page = inspection
        .pages
        .get(index)
        .ok_or(ManualViewerError::PageOutOfRange {
            index,
            count: inspection.pages.len(),
        })?;
    let (file, metadata) = open_regular(&inspection.id.path)?;
    if !inspection.id.still_matches(&metadata) {
        return Err(ManualViewerError::SourceChanged);
    }
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| ManualViewerError::Malformed(format!("not a readable ZIP: {e}")))?;
    let entry = archive
        .by_index(page.member_index)
        .map_err(|e| ManualViewerError::Malformed(format!("bad ZIP member: {e}")))?;
    // The member at this position must still be the one that was inspected.
    let current =
        validate_member_name(entry.name(), limits).map_err(|_| ManualViewerError::SourceChanged)?;
    if current != page.name || entry.size() != page.uncompressed_bytes {
        return Err(ManualViewerError::SourceChanged);
    }
    let declared = page.uncompressed_bytes.min(limits.max_member_bytes);
    read_bounded(entry, declared).map_err(|error| match error {
        // Produced more bytes than the header declared: a lying header.
        ManualViewerError::MemberTooLarge { .. } => ManualViewerError::SuspiciousCompression {
            name: page.name.clone(),
        },
        ManualViewerError::Io(message) => ManualViewerError::Malformed(message),
        other => other,
    })
}

/// Decode a page. Dimensions are read from the header and checked against the
/// limits *before* any pixel buffer is allocated.
pub(super) fn decode_image(
    bytes: &[u8],
    limits: &ManualLimits,
) -> Result<ManualPageImage, ManualViewerError> {
    use image::{ImageFormat, ImageReader};
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| ManualViewerError::MalformedImage(e.to_string()))?;
    match reader.format() {
        Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP) => {}
        _ => {
            return Err(ManualViewerError::UnsupportedImage {
                name: String::new(),
            });
        }
    }
    let (width, height) = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| ManualViewerError::MalformedImage(e.to_string()))?
        .into_dimensions()
        .map_err(|e| ManualViewerError::MalformedImage(e.to_string()))?;
    let pixels = u64::from(width) * u64::from(height);
    let decoded_bytes = pixels.checked_mul(4);
    if width == 0
        || height == 0
        || width > limits.max_image_dimension
        || height > limits.max_image_dimension
        || pixels > limits.max_image_pixels
        || decoded_bytes.is_none_or(|b| b > limits.max_decoded_bytes)
    {
        return Err(ManualViewerError::ImageTooLarge { width, height });
    }
    let mut reader = reader;
    let mut image_limits = image::Limits::default();
    image_limits.max_image_width = Some(limits.max_image_dimension);
    image_limits.max_image_height = Some(limits.max_image_dimension);
    image_limits.max_alloc = Some(limits.max_decoded_bytes);
    reader.limits(image_limits);
    let decoded = reader
        .decode()
        .map_err(|e| ManualViewerError::MalformedImage(e.to_string()))?;
    let rgba = decoded.to_rgba8();
    Ok(ManualPageImage {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}
