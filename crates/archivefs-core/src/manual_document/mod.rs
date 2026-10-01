//! Bounded, read-only inspection of manuals and strategy guides (PDF, CBZ, CBR).
//!
//! This is the canonical *content* layer behind an internal viewer. It answers
//! "what is this file, is it safe, how many pages, and can EmuWiz show them",
//! and never executes, extracts, rewrites or follows anything inside a document.
//! Discovery and game association live elsewhere (`gui_v2::documents`); this
//! module is what that layer asks about a file once it has one.
//!
//! * Format is decided from content signatures, never from the extension alone.
//! * Every size, count and dimension is bounded by [`ManualLimits`] with
//!   checked arithmetic, and refused before memory or disk can be exhausted.
//! * CBZ pages are read on demand, one member at a time; nothing is extracted.
//! * PDF is inspected structurally (page count, metadata, encryption, active
//!   content flags). There is no PDF renderer in the workspace, so rendering is
//!   an explicit capability gap ([`ManualReadiness::InspectOnly`]).
//! * CBR is recognised by signature but reported as unsupported: reading RAR
//!   needs a decompressor that is not a dependency, and no unsupervised tool is
//!   spawned.

mod detect;
mod order;
mod pdf;
mod viewer_state;
mod zip_pages;

#[cfg(test)]
mod tests;

use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

pub use detect::{ManualFormatEvidence, ManualSignature};
pub use order::{PageGroup, compare_page_names, page_group};
pub use viewer_state::{ManualViewerAction, ManualViewerState, ManualZoom};
pub use zip_pages::ManualPageImage;

/// The formats the viewer foundation understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ManualDocumentKind {
    Pdf,
    Cbz,
    Cbr,
}

impl ManualDocumentKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pdf => "PDF",
            Self::Cbz => "CBZ",
            Self::Cbr => "CBR",
        }
    }
}

/// Every bound the module enforces. The defaults are the safe ceilings; a
/// caller may only tighten them (see [`ManualLimits::clamped_to_defaults`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualLimits {
    /// Largest container file inspected at all.
    pub max_file_bytes: u64,
    /// Archive members counted (every member, page or not).
    pub max_members: usize,
    /// Declared and actual uncompressed size of one member.
    pub max_member_bytes: u64,
    /// Sum of declared uncompressed sizes of all members.
    pub max_total_uncompressed_bytes: u64,
    /// A member larger than `ratio_floor_bytes` whose expansion ratio exceeds
    /// this is treated as a decompression bomb.
    pub max_expansion_ratio: u64,
    pub ratio_floor_bytes: u64,
    /// Pages listed or reported.
    pub max_pages: usize,
    /// Image width/height, and total pixels, checked before pixels are decoded.
    pub max_image_dimension: u32,
    pub max_image_pixels: u64,
    /// Upper bound on a decoded RGBA page (`pixels * 4`).
    pub max_decoded_bytes: u64,
    /// Member path length in bytes.
    pub max_name_bytes: usize,
    /// Metadata strings are cut to this many characters.
    pub max_metadata_chars: usize,
    /// PDF structure bounds.
    pub pdf_max_xref_sections: usize,
    /// Trailing NUL/whitespace padding skipped when looking for `startxref`.
    pub pdf_max_tail_padding: u64,
    pub pdf_max_objects: usize,
    pub pdf_max_stream_bytes: u64,
    pub pdf_max_object_window: u64,
    pub pdf_max_nesting: usize,
}

impl Default for ManualLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 4 * 1024 * 1024 * 1024,
            max_members: 10_000,
            max_member_bytes: 64 * 1024 * 1024,
            max_total_uncompressed_bytes: 2 * 1024 * 1024 * 1024,
            max_expansion_ratio: 200,
            ratio_floor_bytes: 1024 * 1024,
            max_pages: 10_000,
            max_image_dimension: 16_384,
            max_image_pixels: 100_000_000,
            max_decoded_bytes: 400_000_000,
            max_name_bytes: 512,
            max_metadata_chars: 256,
            pdf_max_xref_sections: 256,
            pdf_max_tail_padding: 16 * 1024 * 1024,
            pdf_max_objects: 500_000,
            pdf_max_stream_bytes: 16 * 1024 * 1024,
            pdf_max_object_window: 4 * 1024 * 1024,
            pdf_max_nesting: 32,
        }
    }
}

impl ManualLimits {
    /// Never exceed the defaults, whatever a caller asked for.
    #[must_use]
    pub fn clamped_to_defaults(self) -> Self {
        let d = Self::default();
        Self {
            max_file_bytes: self.max_file_bytes.min(d.max_file_bytes),
            max_members: self.max_members.min(d.max_members),
            max_member_bytes: self.max_member_bytes.min(d.max_member_bytes),
            max_total_uncompressed_bytes: self
                .max_total_uncompressed_bytes
                .min(d.max_total_uncompressed_bytes),
            max_expansion_ratio: self.max_expansion_ratio.min(d.max_expansion_ratio),
            ratio_floor_bytes: self.ratio_floor_bytes.min(d.ratio_floor_bytes),
            max_pages: self.max_pages.min(d.max_pages),
            max_image_dimension: self.max_image_dimension.min(d.max_image_dimension),
            max_image_pixels: self.max_image_pixels.min(d.max_image_pixels),
            max_decoded_bytes: self.max_decoded_bytes.min(d.max_decoded_bytes),
            max_name_bytes: self.max_name_bytes.min(d.max_name_bytes),
            max_metadata_chars: self.max_metadata_chars.min(d.max_metadata_chars),
            pdf_max_xref_sections: self.pdf_max_xref_sections.min(d.pdf_max_xref_sections),
            pdf_max_tail_padding: self.pdf_max_tail_padding.min(d.pdf_max_tail_padding),
            pdf_max_objects: self.pdf_max_objects.min(d.pdf_max_objects),
            pdf_max_stream_bytes: self.pdf_max_stream_bytes.min(d.pdf_max_stream_bytes),
            pdf_max_object_window: self.pdf_max_object_window.min(d.pdf_max_object_window),
            pdf_max_nesting: self.pdf_max_nesting.min(d.pdf_max_nesting),
        }
    }
}

/// Why a document is refused. Plain text via [`ManualViewerError::user_message`];
/// technical detail stays in the variant fields / `Display`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManualViewerError {
    Io(String),
    NotARegularFile,
    TooLarge {
        bytes: u64,
        max: u64,
    },
    UnrecognisedFormat,
    /// An encrypted PDF. Never decrypted; many such files have only an owner
    /// password and open freely elsewhere, so this does not claim a password
    /// is required.
    Encrypted,
    Malformed(String),
    TooManyMembers {
        count: usize,
        max: usize,
    },
    MemberTooLarge {
        name: String,
        bytes: u64,
        max: u64,
    },
    ArchiveTooLarge {
        bytes: u64,
        max: u64,
    },
    SuspiciousCompression {
        name: String,
    },
    UnsafeMember {
        name: String,
        reason: UnsafeMemberReason,
    },
    DuplicateMember {
        name: String,
    },
    NestedArchive {
        name: String,
    },
    EncryptedMember {
        name: String,
    },
    NoSupportedPages,
    TooManyPages {
        count: usize,
        max: usize,
    },
    UnsupportedImage {
        name: String,
    },
    ImageTooLarge {
        width: u32,
        height: u32,
    },
    MalformedImage(String),
    PageOutOfRange {
        index: usize,
        count: usize,
    },
    /// The file changed after it was inspected.
    SourceChanged,
    /// The format is recognised but cannot be read (see [`ManualCapabilityGap`]).
    CapabilityUnavailable(ManualCapabilityGap),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsafeMemberReason {
    Traversal,
    AbsolutePath,
    DriveOrUnc,
    ControlCharacter,
    NameTooLong,
    EmptyName,
    Symlink,
    SpecialFile,
}

impl fmt::Display for ManualViewerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(m) => write!(f, "could not read the file: {m}"),
            Self::NotARegularFile => f.write_str("not a regular file"),
            Self::TooLarge { bytes, max } => write!(f, "file is {bytes} bytes; limit is {max}"),
            Self::UnrecognisedFormat => f.write_str("not a recognised PDF, CBZ or CBR file"),
            Self::Encrypted => f.write_str("the PDF is encrypted (EmuWiz does not decrypt PDFs)"),
            Self::Malformed(m) => write!(f, "malformed document: {m}"),
            Self::TooManyMembers { count, max } => {
                write!(f, "archive has {count} members; limit is {max}")
            }
            Self::MemberTooLarge { name, bytes, max } => {
                write!(f, "member {name:?} is {bytes} bytes; limit is {max}")
            }
            Self::ArchiveTooLarge { bytes, max } => {
                write!(f, "archive expands to {bytes} bytes; limit is {max}")
            }
            Self::SuspiciousCompression { name } => {
                write!(
                    f,
                    "member {name:?} expands suspiciously (possible decompression bomb)"
                )
            }
            Self::UnsafeMember { name, reason } => {
                write!(f, "unsafe archive member {name:?}: {reason:?}")
            }
            Self::DuplicateMember { name } => write!(f, "duplicate archive member {name:?}"),
            Self::NestedArchive { name } => write!(f, "nested archive {name:?} is not supported"),
            Self::EncryptedMember { name } => write!(f, "member {name:?} is encrypted"),
            Self::NoSupportedPages => f.write_str("no supported page images were found"),
            Self::TooManyPages { count, max } => write!(f, "{count} pages; limit is {max}"),
            Self::UnsupportedImage { name } => write!(f, "unsupported image format: {name:?}"),
            Self::ImageTooLarge { width, height } => {
                write!(f, "page image is {width}x{height}, over the safe limit")
            }
            Self::MalformedImage(m) => write!(f, "page image refused: {m}"),
            Self::PageOutOfRange { index, count } => {
                write!(f, "page {index} is outside 0..{count}")
            }
            Self::SourceChanged => f.write_str("the file changed after it was opened"),
            Self::CapabilityUnavailable(gap) => write!(f, "unavailable: {}", gap.detail()),
        }
    }
}

impl std::error::Error for ManualViewerError {}

impl ManualViewerError {
    /// Short wording for a normal screen.
    #[must_use]
    pub fn user_message(&self) -> &'static str {
        match self {
            Self::Io(_) | Self::NotARegularFile => "EmuWiz could not read this file.",
            Self::TooLarge { .. } | Self::ArchiveTooLarge { .. } | Self::TooManyMembers { .. } => {
                "This document is larger than EmuWiz opens safely."
            }
            Self::UnrecognisedFormat => "This is not a PDF, CBZ or CBR file.",
            Self::Encrypted => {
                "This PDF is encrypted, so EmuWiz cannot show it in its own viewer. It may still open in your PDF viewer."
            }
            Self::Malformed(_) | Self::MalformedImage(_) | Self::DuplicateMember { .. } => {
                "This document is damaged or not valid."
            }
            Self::MemberTooLarge { .. }
            | Self::SuspiciousCompression { .. }
            | Self::ImageTooLarge { .. }
            | Self::TooManyPages { .. } => {
                "This document contains oversized content, so it was refused."
            }
            Self::UnsafeMember { .. } | Self::NestedArchive { .. } => {
                "This archive contains unsafe or unsupported entries, so it was refused."
            }
            Self::EncryptedMember { .. } => "This archive is encrypted, so it cannot be shown.",
            Self::NoSupportedPages | Self::UnsupportedImage { .. } => {
                "This archive has no pages EmuWiz can show."
            }
            Self::PageOutOfRange { .. } => "That page does not exist.",
            Self::SourceChanged => "This file changed while it was open. Open it again.",
            Self::CapabilityUnavailable(_) => "EmuWiz cannot show this kind of document yet.",
        }
    }
}

/// What is missing for a format that is recognised but cannot be shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManualCapabilityGap {
    /// No PDF page renderer is a workspace dependency.
    PdfRenderer,
    /// No RAR reader/decompressor is a workspace dependency, and no tool is
    /// spawned without a supervised EmuWiz execution primitive.
    RarReader,
}

impl ManualCapabilityGap {
    #[must_use]
    pub const fn detail(self) -> &'static str {
        match self {
            Self::PdfRenderer => {
                "rendering PDF pages needs a PDF rendering library, which is not a dependency of this workspace"
            }
            Self::RarReader => {
                "reading RAR archives needs a RAR decompressor (a vetted crate or a supervised unrar tool), which is not available"
            }
        }
    }
}

/// Whether the viewer can actually show this document's pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManualReadiness {
    /// Pages can be read and decoded.
    Viewable,
    /// Structure and page count are known but pages cannot be rendered.
    InspectOnly { missing: ManualCapabilityGap },
    /// Recognised, but nothing about the pages can be read.
    Unsupported { missing: ManualCapabilityGap },
}

impl ManualReadiness {
    #[must_use]
    pub const fn can_view(self) -> bool {
        matches!(self, Self::Viewable)
    }
}

/// Identity of the exact file that was inspected, used to detect it changing
/// before a later read and as the viewer's document identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualDocumentId {
    pub path: PathBuf,
    pub len: u64,
    pub modified_nanos: Option<u128>,
    pub device_inode: Option<(u64, u64)>,
}

impl ManualDocumentId {
    fn capture(path: &Path, metadata: &fs::Metadata) -> Self {
        Self {
            path: path.to_path_buf(),
            len: metadata.len(),
            modified_nanos: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos()),
            #[cfg(unix)]
            device_inode: {
                use std::os::unix::fs::MetadataExt;
                Some((metadata.dev(), metadata.ino()))
            },
            #[cfg(not(unix))]
            device_inode: None,
        }
    }

    fn still_matches(&self, metadata: &fs::Metadata) -> bool {
        let now = Self::capture(&self.path, metadata);
        now.len == self.len
            && now.modified_nanos == self.modified_nanos
            && now.device_inode == self.device_inode
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManualPageFormat {
    Png,
    Jpeg,
    Webp,
}

/// One viewable page of an archive document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualPage {
    pub index: usize,
    /// Position of the member in the archive's central directory. Valid only
    /// for the exact file that was inspected (its identity is re-checked on
    /// every read).
    pub member_index: usize,
    /// Normalised member path (forward slashes).
    pub name: String,
    pub format: ManualPageFormat,
    pub compressed_bytes: u64,
    pub uncompressed_bytes: u64,
    pub group: PageGroup,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManualMetadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub producer: Option<String>,
    pub created: Option<String>,
}

/// PDF features that would be *active* in a full reader. They are only ever
/// reported, never run: no script, embedded file, form or link is executed or
/// opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ManualActiveContent {
    OpenAction,
    AdditionalActions,
    JavaScript,
    EmbeddedFiles,
    InteractiveForm,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManualWarning {
    /// The file extension disagreed with the content; the content was trusted.
    ExtensionMismatch {
        claimed: String,
        detected: ManualDocumentKind,
    },
    /// Members that are not pages (metadata, thumbnails, notes) were ignored.
    IgnoredMembers(usize),
    /// Image members in a format EmuWiz cannot decode were left out.
    UnsupportedImageMembers(usize),
    ActiveContentIgnored,
    /// A PDF's declared page count is metadata only; the page tree is not walked.
    PageCountIsDeclared,
}

/// Everything learned from one bounded, read-only look at a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualInspection {
    pub id: ManualDocumentId,
    pub kind: ManualDocumentKind,
    pub evidence: ManualFormatEvidence,
    pub readiness: ManualReadiness,
    pub page_count: Option<usize>,
    /// Pages in reading order. Filled for CBZ; empty for PDF and CBR.
    pub pages: Vec<ManualPage>,
    pub metadata: ManualMetadata,
    pub active_content: Vec<ManualActiveContent>,
    pub warnings: Vec<ManualWarning>,
}

impl ManualInspection {
    /// A display title: embedded title, else the file stem.
    #[must_use]
    pub fn display_title(&self) -> String {
        self.metadata
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .or_else(|| {
                self.id
                    .path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.replace(['_', '-'], " "))
            })
            .unwrap_or_else(|| "Untitled document".into())
    }
}

/// Inspect a file without executing or extracting anything.
pub fn inspect_manual(
    path: &Path,
    limits: &ManualLimits,
) -> Result<ManualInspection, ManualViewerError> {
    let limits = limits.clone().clamped_to_defaults();
    let (mut file, metadata) = open_regular(path)?;
    if metadata.len() > limits.max_file_bytes {
        return Err(ManualViewerError::TooLarge {
            bytes: metadata.len(),
            max: limits.max_file_bytes,
        });
    }
    let id = ManualDocumentId::capture(path, &metadata);
    let evidence = detect::detect(&mut file, path)?;
    let mut warnings = Vec::new();
    if let Some(claimed) = evidence.extension_mismatch() {
        warnings.push(ManualWarning::ExtensionMismatch {
            claimed,
            detected: evidence.kind,
        });
    }
    let kind = evidence.kind;
    let mut inspection = match kind {
        ManualDocumentKind::Pdf => pdf::inspect(&mut file, id, evidence, &limits)?,
        ManualDocumentKind::Cbz => zip_pages::inspect(file, id, evidence, &limits)?,
        ManualDocumentKind::Cbr => ManualInspection {
            id,
            kind,
            evidence,
            readiness: ManualReadiness::Unsupported {
                missing: ManualCapabilityGap::RarReader,
            },
            page_count: None,
            pages: Vec::new(),
            metadata: ManualMetadata::default(),
            active_content: Vec::new(),
            warnings: Vec::new(),
        },
    };
    warnings.append(&mut inspection.warnings);
    inspection.warnings = warnings;
    Ok(inspection)
}

/// An opened document whose pages can be read on demand. Holds no file handle
/// between calls and re-checks the file's identity before every read.
#[derive(Clone, Debug)]
pub struct ManualDocument {
    inspection: ManualInspection,
    limits: ManualLimits,
}

impl ManualDocument {
    pub fn open(path: &Path, limits: &ManualLimits) -> Result<Self, ManualViewerError> {
        let limits = limits.clone().clamped_to_defaults();
        let inspection = inspect_manual(path, &limits)?;
        Ok(Self { inspection, limits })
    }

    #[must_use]
    pub fn inspection(&self) -> &ManualInspection {
        &self.inspection
    }

    #[must_use]
    pub fn id(&self) -> &ManualDocumentId {
        &self.inspection.id
    }

    /// Raw bytes of one page image, bounded and size-checked.
    pub fn read_page_bytes(&self, index: usize) -> Result<Vec<u8>, ManualViewerError> {
        self.require_viewable()?;
        zip_pages::read_page_bytes(&self.inspection, index, &self.limits)
    }

    /// Decode one page to RGBA, refusing absurd dimensions before decoding.
    pub fn decode_page(&self, index: usize) -> Result<ManualPageImage, ManualViewerError> {
        let bytes = self.read_page_bytes(index)?;
        zip_pages::decode_image(&bytes, &self.limits)
    }

    fn require_viewable(&self) -> Result<(), ManualViewerError> {
        match self.inspection.readiness {
            ManualReadiness::Viewable => Ok(()),
            ManualReadiness::InspectOnly { missing } | ManualReadiness::Unsupported { missing } => {
                Err(ManualViewerError::CapabilityUnavailable(missing))
            }
        }
    }
}

pub(crate) fn open_regular(path: &Path) -> Result<(File, fs::Metadata), ManualViewerError> {
    // Refuse a symlink at the final component instead of silently following it.
    let link = fs::symlink_metadata(path).map_err(|e| ManualViewerError::Io(e.to_string()))?;
    if link.file_type().is_symlink() || !link.is_file() {
        return Err(ManualViewerError::NotARegularFile);
    }
    let file = File::open(path).map_err(|e| ManualViewerError::Io(e.to_string()))?;
    let metadata = file
        .metadata()
        .map_err(|e| ManualViewerError::Io(e.to_string()))?;
    if !metadata.is_file() {
        return Err(ManualViewerError::NotARegularFile);
    }
    Ok((file, metadata))
}

/// Read at most `max` bytes, failing rather than returning more.
pub(crate) fn read_bounded<R: Read>(mut reader: R, max: u64) -> Result<Vec<u8>, ManualViewerError> {
    let mut out = Vec::new();
    (&mut reader)
        .take(max.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(|e| ManualViewerError::Io(e.to_string()))?;
    if out.len() as u64 > max {
        return Err(ManualViewerError::MemberTooLarge {
            name: String::new(),
            bytes: out.len() as u64,
            max,
        });
    }
    Ok(out)
}
