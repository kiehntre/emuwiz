//! Bounded, sanitised handling of untrusted text.
//!
//! Emulator logs and stderr can contain anything: a game can print through
//! its emulator, and a file name is whatever someone called the file. This
//! module makes such text safe to *carry and display* and nothing more. It
//! never interprets it. Two rules follow from that:
//!
//! - every limit here is a hard cap applied before any other work, so a
//!   huge or hostile input costs a bounded amount of time and memory;
//! - sanitised text contains no control characters, terminal escapes or
//!   direction overrides, so it cannot repaint a terminal, hide part of
//!   itself, or pass for something EmuWiz wrote.

/// Most log bytes examined from one stream. Text beyond this is not read
/// and the coverage record says so.
pub const LOG_BYTES_LIMIT: usize = 4 * 1024 * 1024;
/// Most log lines examined from one stream.
pub const LOG_LINES_LIMIT: usize = 50_000;
/// Longest line considered for signature matching. A longer line is
/// counted as ignored rather than matched on a prefix.
pub const LOG_LINE_LIMIT: usize = 4_096;
/// Most matched events kept from one stream.
pub const LOG_EVENTS_LIMIT: usize = 200;
/// Longest excerpt of a matched line kept for display.
pub const EXCERPT_LIMIT: usize = 240;
/// Longest value a signature may extract from a line.
pub const VALUE_LIMIT: usize = 200;
/// Longest path kept in evidence.
pub const PATH_LIMIT: usize = 1_024;
/// Longest source description or reason kept in evidence.
pub const SOURCE_LIMIT: usize = 300;
/// Longest one-line summary of an evidence item.
pub const SUMMARY_LIMIT: usize = 600;

/// Marker appended when text was shortened.
const ELLIPSIS: char = '\u{2026}';

fn is_unsafe(character: char) -> bool {
    character.is_control()
        // Direction overrides and isolates can reorder how text is shown.
        || matches!(character, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
        // Zero-width and invisible formatting characters can hide content.
        || matches!(character, '\u{200B}'..='\u{200F}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}')
        // Line and paragraph separators would split one line into several.
        || matches!(character, '\u{2028}' | '\u{2029}')
}

/// Make text safe to display: unsafe characters become a visible
/// replacement character and the result is at most `limit` characters.
pub fn sanitise(input: &str, limit: usize) -> String {
    let mut output = String::with_capacity(input.len().min(limit * 4));
    let mut count = 0usize;
    for character in input.chars() {
        if count == limit {
            output.push(ELLIPSIS);
            break;
        }
        output.push(if is_unsafe(character) {
            char::REPLACEMENT_CHARACTER
        } else {
            character
        });
        count += 1;
    }
    output
}

/// One line of examined text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line<'a> {
    /// One-based line number.
    pub number: u32,
    pub text: &'a str,
}

/// A bounded prefix of some untrusted text, split into lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Examined {
    text: String,
    spans: Vec<(u32, std::ops::Range<usize>)>,
    pub bytes_examined: u64,
    /// The input continued past a limit and the rest was not examined.
    pub truncated: bool,
    /// Lines skipped because they were over-long.
    pub lines_ignored: u32,
}

impl Examined {
    pub fn lines(&self) -> impl Iterator<Item = Line<'_>> {
        self.spans.iter().map(|(number, range)| Line {
            number: *number,
            text: &self.text[range.clone()],
        })
    }

    pub fn line_count(&self) -> u32 {
        self.spans.len() as u32
    }
}

/// Examine at most [`LOG_BYTES_LIMIT`] bytes of captured output as at most
/// [`LOG_LINES_LIMIT`] lines.
///
/// Invalid UTF-8 becomes replacement characters rather than an error: a log
/// that cannot be decoded is still evidence that a log exists. Lines end at
/// `\n`; a trailing `\r` is dropped. A line over [`LOG_LINE_LIMIT`] bytes
/// is skipped and counted, and a final line cut by the byte limit is
/// dropped, so a signature is never matched against part of a line.
pub fn examine(bytes: &[u8]) -> Examined {
    let cut = bytes.len() > LOG_BYTES_LIMIT;
    let window = &bytes[..bytes.len().min(LOG_BYTES_LIMIT)];
    // When the input was cut, drop the partial last line before decoding.
    let window = if cut {
        match window.iter().rposition(|&byte| byte == b'\n') {
            Some(index) => &window[..=index],
            None => &[][..],
        }
    } else {
        window
    };
    let text = String::from_utf8_lossy(window).into_owned();
    let mut spans = Vec::new();
    let mut lines_ignored = 0u32;
    let mut truncated = cut;
    let mut number = 0u32;
    let mut offset = 0usize;
    for raw in text.split_inclusive('\n') {
        if number as usize == LOG_LINES_LIMIT {
            truncated = true;
            break;
        }
        number += 1;
        let start = offset;
        offset += raw.len();
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.len() > LOG_LINE_LIMIT {
            lines_ignored += 1;
            continue;
        }
        spans.push((number, start..start + line.len()));
    }
    Examined {
        text,
        spans,
        bytes_examined: offset as u64,
        truncated,
        lines_ignored,
    }
}

/// The text between the first `"` after `prefix` and the next `"`, when
/// `line` starts with `prefix`. Used by signatures that quote a value.
pub fn quoted_after<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(prefix)?;
    let start = rest.find('"')? + 1;
    let length = rest[start..].find('"')?;
    Some(&rest[start..start + length])
}

#[cfg(test)]
#[path = "text/tests.rs"]
mod tests;
