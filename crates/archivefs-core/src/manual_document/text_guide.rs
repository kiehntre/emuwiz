//! Bounded, read-only plain-text guides (FAQs, walkthroughs, hint files).
//!
//! Text guides are not paged documents, so they stay outside
//! [`super::ManualDocumentKind`] and the page viewer. They share the same
//! file-safety rules: regular files only (a final symlink is refused), a hard
//! size ceiling, an identity fingerprint re-checked before the read, and
//! nothing is ever executed, rewritten or extracted.
//!
//! The content is untrusted. Binary data is refused, control and
//! bidirectional-override characters are removed before display, invalid
//! UTF-8 is replaced visibly, and very long lines are shortened for display.

use std::io::Read;
use std::path::Path;

use super::{ManualDocumentId, ManualViewerError, open_regular, read_bounded};

/// Largest text guide read at all. Real guides are well under 2 MiB.
pub const MAX_TEXT_GUIDE_BYTES: u64 = 8 * 1024 * 1024;
/// Lines kept for display; a longer file is refused rather than cut silently.
pub const MAX_TEXT_GUIDE_LINES: usize = 200_000;
/// Characters shown per line; the rest of an over-long line is replaced by `…`.
pub const MAX_TEXT_GUIDE_LINE_CHARS: usize = 2_000;
/// Bytes sniffed during discovery to tell text from binary.
const SNIFF_BYTES: u64 = 8 * 1024;

/// A text guide ready for display. Holds no file handle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextGuide {
    pub id: ManualDocumentId,
    pub lines: Vec<String>,
    /// Invalid UTF-8 was shown as U+FFFD.
    pub replaced_invalid_bytes: bool,
    /// Control or bidirectional-override characters were removed.
    pub removed_control_characters: bool,
    /// Lines over [`MAX_TEXT_GUIDE_LINE_CHARS`] that were shortened.
    pub shortened_lines: usize,
}

/// Cheap discovery check: a regular file under the size limit whose first
/// bytes look like text. Returns the identity a later read must still match.
pub fn inspect_text_guide(path: &Path) -> Result<ManualDocumentId, ManualViewerError> {
    let (file, metadata) = open_regular(path)?;
    check_size(metadata.len())?;
    let mut head = Vec::new();
    file.take(SNIFF_BYTES)
        .read_to_end(&mut head)
        .map_err(|e| ManualViewerError::Io(e.to_string()))?;
    refuse_binary(&head)?;
    Ok(ManualDocumentId::capture(path, &metadata))
}

/// Read a whole guide. With `expected`, the file must still be the one that
/// was inspected; otherwise [`ManualViewerError::SourceChanged`].
pub fn read_text_guide(
    path: &Path,
    expected: Option<&ManualDocumentId>,
) -> Result<TextGuide, ManualViewerError> {
    if let Some(expected) = expected {
        if expected.path != path {
            return Err(ManualViewerError::SourceChanged);
        }
        expected.check_path()?;
    }
    let (file, metadata) = open_regular(path)?;
    check_size(metadata.len())?;
    let id = ManualDocumentId::capture(path, &metadata);
    if expected.is_some_and(|expected| !expected.still_matches(&metadata)) {
        return Err(ManualViewerError::SourceChanged);
    }
    let bytes =
        read_bounded(file, MAX_TEXT_GUIDE_BYTES).map_err(|_| ManualViewerError::TooLarge {
            bytes: MAX_TEXT_GUIDE_BYTES.saturating_add(1),
            max: MAX_TEXT_GUIDE_BYTES,
        })?;
    if bytes.len() as u64 != metadata.len() {
        return Err(ManualViewerError::SourceChanged);
    }
    refuse_binary(&bytes)?;
    let mut guide = decode(&bytes)?;
    guide.id = id;
    Ok(guide)
}

fn check_size(len: u64) -> Result<(), ManualViewerError> {
    if len > MAX_TEXT_GUIDE_BYTES {
        return Err(ManualViewerError::TooLarge {
            bytes: len,
            max: MAX_TEXT_GUIDE_BYTES,
        });
    }
    Ok(())
}

/// NUL bytes never occur in plain-text guides but always occur in UTF-16,
/// executables, images and archives, so one is enough to refuse.
fn refuse_binary(bytes: &[u8]) -> Result<(), ManualViewerError> {
    if bytes.contains(&0) {
        return Err(ManualViewerError::Malformed(
            "the file contains binary data and is not a plain-text guide".into(),
        ));
    }
    Ok(())
}

fn is_removed(character: char) -> bool {
    (character.is_control() && character != '\t')
        || matches!(
            character,
            '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
        )
}

fn decode(bytes: &[u8]) -> Result<TextGuide, ManualViewerError> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = String::from_utf8_lossy(bytes);
    let replaced_invalid_bytes = matches!(text, std::borrow::Cow::Owned(_));
    let mut lines = Vec::new();
    let mut removed_control_characters = false;
    let mut shortened_lines = 0;
    // CRLF, LF and lone CR (classic Mac) all end a line.
    for raw in text.split('\n') {
        for raw in raw.strip_suffix('\r').unwrap_or(raw).split('\r') {
            if lines.len() == MAX_TEXT_GUIDE_LINES {
                return Err(ManualViewerError::TooManyPages {
                    count: MAX_TEXT_GUIDE_LINES.saturating_add(1),
                    max: MAX_TEXT_GUIDE_LINES,
                });
            }
            let mut line = String::with_capacity(raw.len().min(MAX_TEXT_GUIDE_LINE_CHARS));
            let mut kept = 0;
            let mut shortened = false;
            for character in raw.chars() {
                if is_removed(character) {
                    removed_control_characters = true;
                    continue;
                }
                if kept == MAX_TEXT_GUIDE_LINE_CHARS {
                    shortened = true;
                    break;
                }
                line.push(character);
                kept += 1;
            }
            if shortened {
                line.push('…');
                shortened_lines += 1;
            }
            lines.push(line);
        }
    }
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    Ok(TextGuide {
        id: ManualDocumentId {
            path: Default::default(),
            len: 0,
            modified_nanos: None,
            device_inode: None,
        },
        lines,
        replaced_invalid_bytes,
        removed_control_characters,
        shortened_lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn reads_guide_lines_with_any_line_ending() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "guide.txt",
            b"\xEF\xBB\xBFWalkthrough\r\nPart 1\rPart 2\n\tBoss\n",
        );
        let id = inspect_text_guide(&path).unwrap();
        let guide = read_text_guide(&path, Some(&id)).unwrap();
        assert_eq!(guide.lines, ["Walkthrough", "Part 1", "Part 2", "\tBoss"]);
        assert!(!guide.replaced_invalid_bytes && !guide.removed_control_characters);
        assert_eq!(guide.id, id);
    }

    #[test]
    fn untrusted_characters_are_removed_and_invalid_bytes_visible() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "guide.txt",
            "a\u{1b}[31mred\u{202E}txt.exe\u{7}\n".as_bytes(),
        );
        let guide = read_text_guide(&path, None).unwrap();
        assert_eq!(guide.lines, ["a[31mredtxt.exe"]);
        assert!(guide.removed_control_characters);
        let latin1 = write(dir.path(), "latin1.txt", b"caf\xE9\n");
        let guide = read_text_guide(&latin1, None).unwrap();
        assert_eq!(guide.lines, ["caf\u{FFFD}"]);
        assert!(guide.replaced_invalid_bytes);
    }

    #[test]
    fn binary_oversized_and_symlinked_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let binary = write(dir.path(), "game.txt", b"MZ\x90\x00\x03");
        assert!(matches!(
            inspect_text_guide(&binary),
            Err(ManualViewerError::Malformed(_))
        ));
        let utf16 = write(dir.path(), "utf16.txt", b"\xFF\xFEG\x00");
        assert!(read_text_guide(&utf16, None).is_err());
        let big = dir.path().join("big.txt");
        fs::File::create(&big)
            .unwrap()
            .set_len(MAX_TEXT_GUIDE_BYTES + 1)
            .unwrap();
        assert!(matches!(
            inspect_text_guide(&big),
            Err(ManualViewerError::TooLarge { .. })
        ));
        #[cfg(unix)]
        {
            let target = write(dir.path(), "real.txt", b"text\n");
            let link = dir.path().join("link.txt");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert_eq!(
                inspect_text_guide(&link),
                Err(ManualViewerError::NotARegularFile)
            );
            assert_eq!(
                read_text_guide(&link, None),
                Err(ManualViewerError::NotARegularFile)
            );
        }
        assert_eq!(
            inspect_text_guide(dir.path()),
            Err(ManualViewerError::NotARegularFile)
        );
    }

    #[test]
    fn long_lines_are_shortened_and_too_many_lines_refused() {
        let dir = tempfile::tempdir().unwrap();
        let long = "x".repeat(MAX_TEXT_GUIDE_LINE_CHARS + 10);
        let path = write(dir.path(), "long.txt", long.as_bytes());
        let guide = read_text_guide(&path, None).unwrap();
        assert_eq!(guide.shortened_lines, 1);
        assert_eq!(
            guide.lines[0].chars().count(),
            MAX_TEXT_GUIDE_LINE_CHARS + 1
        );
        let many = "\n".repeat(MAX_TEXT_GUIDE_LINES + 1);
        let path = write(dir.path(), "many.txt", many.as_bytes());
        assert!(matches!(
            read_text_guide(&path, None),
            Err(ManualViewerError::TooManyPages { .. })
        ));
    }

    #[test]
    fn a_changed_file_is_refused_after_inspection() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "guide.txt", b"first\n");
        let id = inspect_text_guide(&path).unwrap();
        fs::write(&path, b"replaced with longer text\n").unwrap();
        assert_eq!(
            read_text_guide(&path, Some(&id)),
            Err(ManualViewerError::SourceChanged)
        );
        let other = write(dir.path(), "other.txt", b"first\n");
        assert_eq!(
            read_text_guide(&other, Some(&id)),
            Err(ManualViewerError::SourceChanged)
        );
    }
}
