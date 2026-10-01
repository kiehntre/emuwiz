//! CBZ: bounded inspection and on-demand page reading over the workspace's
//! existing `zip` dependency. Nothing is ever extracted to disk; a page is read
//! one member at a time into memory, size-checked, then decoded.

use std::collections::HashSet;
use std::fs::File;
use std::io::Cursor;

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

pub(super) fn inspect(
    mut file: File,
    id: ManualDocumentId,
    evidence: ManualFormatEvidence,
    limits: &ManualLimits,
) -> Result<ManualInspection, ManualViewerError> {
    // Canonical physical enumeration precedes the name-deduplicating ZIP index.
    // Its complete central-directory check also refuses trailing junk, rather
    // than letting a missing end record disable the bounds.
    use crate::dat::archive::{
        limits::ArchiveLimits,
        zip_preflight::{ZipPreflightError, preflight_zip},
    };
    let archive_limits = ArchiveLimits {
        max_members: limits.max_members,
        ..ArchiveLimits::default()
    };
    let physical = preflight_zip(
        &mut file,
        id.len,
        &archive_limits,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .map_err(|e| match e {
        ZipPreflightError::Refused("member count") => ManualViewerError::TooManyMembers {
            count: limits.max_members.saturating_add(1),
            max: limits.max_members,
        },
        other => ManualViewerError::Malformed(format!("unsafe ZIP structure: {other:?}")),
    })?;
    let mut seen = HashSet::new();
    let mut total: u64 = 0;
    let mut ignored = 0usize;
    let mut unsupported = 0usize;
    let mut first_unsupported: Option<String> = None;
    let mut pages: Vec<(String, ManualPageFormat, usize, u64, u64)> = Vec::new();

    for (index, entry) in physical.entries.iter().enumerate() {
        if index >= limits.max_members {
            return Err(ManualViewerError::TooManyMembers {
                count: index + 1,
                max: limits.max_members,
            });
        }
        let raw_name = String::from_utf8_lossy(&entry.name_raw);
        let name = validate_member_name(&raw_name, limits).map_err(|reason| {
            ManualViewerError::UnsafeMember {
                name: sanitise_for_error(&raw_name),
                reason,
            }
        })?;
        if !seen.insert(name.clone()) {
            return Err(ManualViewerError::DuplicateMember { name });
        }
        {
            let mode = entry.external_attributes >> 16;
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
        if entry.flags & (1 | (1 << 6)) != 0 {
            return Err(ManualViewerError::EncryptedMember { name });
        }
        let size = entry.logical_size;
        let packed = entry.compressed_size;
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
        if entry.is_directory || entry.external_attributes >> 16 & S_IFMT == S_IFDIR {
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
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| ManualViewerError::Malformed(format!("not a readable ZIP: {e}")))?;
    if archive.len() != physical.entries.len() {
        return Err(ManualViewerError::DuplicateMember {
            name: "(duplicate ZIP name)".into(),
        });
    }
    // Keep page ordinals tied to the decoder's interpretation of each name.
    for (name, _, index, _, _) in &pages {
        let entry = archive
            .by_index_raw(*index)
            .map_err(|e| ManualViewerError::Malformed(e.to_string()))?;
        if validate_member_name(entry.name(), limits).as_ref() != Ok(name) {
            return Err(ManualViewerError::Malformed(
                "ambiguous ZIP member name encoding".into(),
            ));
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

/// Exact cross multiplication in a wider integer. Empty stored entries are
/// allowed; zero packed bytes can never justify a nonempty logical member.
pub(super) fn within_expansion_ratio(size: u64, packed: u64, ratio: u64) -> bool {
    u128::from(size) <= u128::from(packed) * u128::from(ratio)
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
