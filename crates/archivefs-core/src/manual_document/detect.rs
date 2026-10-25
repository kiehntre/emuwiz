//! Evidence-based format detection. The content signature decides the kind; the
//! extension is only recorded so a mismatch can be reported.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use super::{ManualDocumentKind, ManualViewerError};

/// How far into a file a PDF header may appear (some producers prepend bytes).
const PDF_HEADER_WINDOW: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManualSignature {
    PdfHeader,
    Zip,
    /// RAR 1.5-4.x (`Rar!\x1A\x07\x00`).
    Rar4,
    /// RAR 5.0+ (`Rar!\x1A\x07\x01\x00`).
    Rar5,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualFormatEvidence {
    pub kind: ManualDocumentKind,
    pub signature: ManualSignature,
    /// Lower-case extension as found, if any (not trusted).
    pub extension: Option<String>,
}

impl ManualFormatEvidence {
    /// The claimed extension when it names a document/archive type that does
    /// not match what the content actually is.
    #[must_use]
    pub fn extension_mismatch(&self) -> Option<String> {
        let ext = self.extension.as_deref()?;
        let expected: &[&str] = match self.kind {
            ManualDocumentKind::Pdf => &["pdf"],
            ManualDocumentKind::Cbz => &["cbz", "zip"],
            ManualDocumentKind::Cbr => &["cbr", "rar"],
        };
        let known = ["pdf", "cbz", "cbr", "zip", "rar"];
        (known.contains(&ext) && !expected.contains(&ext)).then(|| ext.to_string())
    }
}

pub(super) fn detect(
    file: &mut File,
    path: &Path,
) -> Result<ManualFormatEvidence, ManualViewerError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|e| ManualViewerError::Io(e.to_string()))?;
    let mut head = Vec::with_capacity(PDF_HEADER_WINDOW);
    file.by_ref()
        .take(PDF_HEADER_WINDOW as u64)
        .read_to_end(&mut head)
        .map_err(|e| ManualViewerError::Io(e.to_string()))?;
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let (kind, signature) = classify(&head).ok_or(ManualViewerError::UnrecognisedFormat)?;
    Ok(ManualFormatEvidence {
        kind,
        signature,
        extension,
    })
}

/// Pure signature check over the first bytes of a file.
pub(super) fn classify(head: &[u8]) -> Option<(ManualDocumentKind, ManualSignature)> {
    // Container magics are anchored at offset 0, so they are checked before the
    // looser PDF header search: a ZIP that merely mentions "%PDF-" is a ZIP.
    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        return Some((ManualDocumentKind::Cbz, ManualSignature::Zip));
    }
    if head.starts_with(b"Rar!\x1A\x07\x01\x00") {
        return Some((ManualDocumentKind::Cbr, ManualSignature::Rar5));
    }
    if head.starts_with(b"Rar!\x1A\x07\x00") {
        return Some((ManualDocumentKind::Cbr, ManualSignature::Rar4));
    }
    head.windows(5)
        .take(PDF_HEADER_WINDOW)
        .any(|window| window == b"%PDF-")
        .then_some((ManualDocumentKind::Pdf, ManualSignature::PdfHeader))
}
