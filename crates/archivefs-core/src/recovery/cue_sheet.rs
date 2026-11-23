//! A strict, bounded reader for the cue sheets found beside CD images.
//!
//! The recovery engine uses it for one question only: which files does this
//! sheet name, and are they plain file names it is safe to place beside the
//! sheet? It is deliberately narrower than what emulators accept: a sheet
//! with anything it does not understand is refused, never guessed at.

use serde::Serialize;

/// Larger than any real sheet (99 tracks); a bigger "cue" is not one.
pub const MAX_CUE_SHEET_BYTES: usize = 64 * 1024;
const MAX_CUE_FILES: usize = 99;
const MAX_CUE_TRACKS: usize = 99;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CueFileKind {
    /// Raw sector data (`BINARY`).
    Binary,
    /// RIFF/WAVE audio (`WAVE`).
    Wave,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CueTrackMode {
    Audio,
    /// 2048-byte user-data sectors.
    Mode1Cooked,
    /// 2352-byte raw Mode 1 sectors.
    Mode1Raw,
    /// 2352-byte raw Mode 2 sectors.
    Mode2Raw,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CueTrack {
    pub number: u8,
    pub mode: CueTrackMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CueFile {
    /// Exactly as written in the sheet; a single plain file name.
    pub name: String,
    pub kind: CueFileKind,
    pub tracks: Vec<CueTrack>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CueSheet {
    pub files: Vec<CueFile>,
}

impl CueSheet {
    pub fn track_count(&self) -> usize {
        self.files.iter().map(|file| file.tracks.len()).sum()
    }

    /// The first data track's file and mode, if the disc has one.
    pub fn first_data_track(&self) -> Option<(&CueFile, CueTrackMode)> {
        self.files.iter().find_map(|file| {
            file.tracks
                .iter()
                .find(|track| track.mode != CueTrackMode::Audio)
                .map(|track| (file, track.mode))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CueRefusal {
    TooLarge,
    NotText,
    /// A line the reader does not understand (1-based line number).
    UnknownLine(usize),
    /// A `FILE` name that is not one plain file name.
    UnsafeFileName(usize),
    /// A `FILE` type other than `BINARY` or `WAVE`.
    UnsupportedFileType(usize, String),
    /// A `TRACK` mode the reader does not know.
    UnsupportedTrackMode(usize, String),
    /// A `TRACK` or `INDEX` before any `FILE`.
    TrackWithoutFile(usize),
    /// Track numbers must rise by one from 1.
    TrackOrder(usize),
    /// The same file is named twice.
    DuplicateFile(usize),
    /// A `FILE` with no `TRACK`.
    FileWithoutTrack(String),
    TooManyEntries,
    Empty,
}

impl std::fmt::Display for CueRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => f.write_str("the cue sheet is larger than any real one"),
            Self::NotText => f.write_str("the cue sheet is not plain text"),
            Self::UnknownLine(line) => {
                write!(f, "line {line} of the cue sheet is not understood")
            }
            Self::UnsafeFileName(line) => write!(
                f,
                "line {line} of the cue sheet names a file that is not a plain file name"
            ),
            Self::UnsupportedFileType(line, kind) => {
                write!(
                    f,
                    "line {line} uses the file type {kind}, which is not supported"
                )
            }
            Self::UnsupportedTrackMode(line, mode) => {
                write!(
                    f,
                    "line {line} uses the track mode {mode}, which is not supported"
                )
            }
            Self::TrackWithoutFile(line) => {
                write!(f, "line {line} describes a track before any file is named")
            }
            Self::TrackOrder(line) => {
                write!(f, "line {line}: track numbers do not rise by one from 1")
            }
            Self::DuplicateFile(line) => write!(f, "line {line} names a file a second time"),
            Self::FileWithoutTrack(name) => write!(f, "the file {name} has no track"),
            Self::TooManyEntries => {
                f.write_str("the cue sheet has more entries than a CD can hold")
            }
            Self::Empty => f.write_str("the cue sheet names no files"),
        }
    }
}

/// One plain file name: no separators, no traversal, no control characters,
/// nothing a shell or an archive tool could read as an option or a pattern.
pub fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.starts_with('-')
        && !name.contains(['/', '\\', '\0', '*', '?', ':'])
        && !name.chars().any(char::is_control)
        && name.trim() == name
}

pub fn parse_cue_sheet(bytes: &[u8]) -> Result<CueSheet, CueRefusal> {
    if bytes.len() > MAX_CUE_SHEET_BYTES {
        return Err(CueRefusal::TooLarge);
    }
    // A UTF-8 byte-order mark is common in sheets written on Windows.
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|_| CueRefusal::NotText)?;
    // A lone carriage return is a line break to some readers and not to this
    // one; a sheet two readers could read differently is refused. So is any
    // other control character.
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        let allowed = match character {
            '\n' | '\t' => true,
            '\r' => characters.peek() == Some(&'\n'),
            other => !other.is_control(),
        };
        if !allowed {
            return Err(CueRefusal::NotText);
        }
    }

    let mut files: Vec<CueFile> = Vec::new();
    let mut next_track: u8 = 1;
    for (index, raw_line) in text.lines().enumerate() {
        let number = index + 1;
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let (keyword, rest) = match line.split_once(char::is_whitespace) {
            Some((keyword, rest)) => (keyword, rest.trim()),
            None => (line, ""),
        };
        match keyword.to_ascii_uppercase().as_str() {
            "REM" | "CATALOG" | "PERFORMER" | "SONGWRITER" | "TITLE" | "ISRC" | "FLAGS"
            | "PREGAP" | "POSTGAP" | "INDEX" => {
                // Timing and metadata do not change which files are needed.
                if matches!(
                    keyword.to_ascii_uppercase().as_str(),
                    "INDEX" | "PREGAP" | "POSTGAP" | "FLAGS" | "ISRC"
                ) && files.last().is_none_or(|file| file.tracks.is_empty())
                {
                    return Err(CueRefusal::TrackWithoutFile(number));
                }
            }
            "FILE" => {
                let (name, kind) = parse_file_line(rest).ok_or(CueRefusal::UnknownLine(number))?;
                if !is_plain_file_name(&name) {
                    return Err(CueRefusal::UnsafeFileName(number));
                }
                let kind = match kind.to_ascii_uppercase().as_str() {
                    "BINARY" => CueFileKind::Binary,
                    "WAVE" => CueFileKind::Wave,
                    other => {
                        return Err(CueRefusal::UnsupportedFileType(number, other.to_string()));
                    }
                };
                if files
                    .iter()
                    .any(|file| file.name.eq_ignore_ascii_case(&name))
                {
                    return Err(CueRefusal::DuplicateFile(number));
                }
                if files.len() >= MAX_CUE_FILES {
                    return Err(CueRefusal::TooManyEntries);
                }
                files.push(CueFile {
                    name,
                    kind,
                    tracks: Vec::new(),
                });
            }
            "TRACK" => {
                let mut parts = rest.split_whitespace();
                let (Some(track), Some(mode), None) = (parts.next(), parts.next(), parts.next())
                else {
                    return Err(CueRefusal::UnknownLine(number));
                };
                let track: u8 = track.parse().map_err(|_| CueRefusal::UnknownLine(number))?;
                let mode = match mode.to_ascii_uppercase().as_str() {
                    "AUDIO" => CueTrackMode::Audio,
                    "MODE1/2048" => CueTrackMode::Mode1Cooked,
                    "MODE1/2352" => CueTrackMode::Mode1Raw,
                    "MODE2/2352" => CueTrackMode::Mode2Raw,
                    other => {
                        return Err(CueRefusal::UnsupportedTrackMode(number, other.to_string()));
                    }
                };
                let Some(file) = files.last_mut() else {
                    return Err(CueRefusal::TrackWithoutFile(number));
                };
                if track != next_track {
                    return Err(CueRefusal::TrackOrder(number));
                }
                if usize::from(next_track) > MAX_CUE_TRACKS {
                    return Err(CueRefusal::TooManyEntries);
                }
                next_track += 1;
                file.tracks.push(CueTrack {
                    number: track,
                    mode,
                });
            }
            _ => return Err(CueRefusal::UnknownLine(number)),
        }
    }
    if files.is_empty() {
        return Err(CueRefusal::Empty);
    }
    if let Some(file) = files.iter().find(|file| file.tracks.is_empty()) {
        return Err(CueRefusal::FileWithoutTrack(file.name.clone()));
    }
    Ok(CueSheet { files })
}

/// `"name with spaces" TYPE` or `name TYPE`.
fn parse_file_line(rest: &str) -> Option<(String, String)> {
    if let Some(after_quote) = rest.strip_prefix('"') {
        let (name, tail) = after_quote.split_once('"')?;
        let mut parts = tail.split_whitespace();
        let kind = parts.next()?;
        if parts.next().is_some() {
            return None;
        }
        Some((name.to_string(), kind.to_string()))
    } else {
        let mut parts = rest.split_whitespace();
        let (name, kind) = (parts.next()?, parts.next()?);
        if parts.next().is_some() {
            return None;
        }
        Some((name.to_string(), kind.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_data_plus_audio_sheet_lists_every_file_in_order() {
        let sheet = parse_cue_sheet(
            b"REM made by a test\r\nFILE \"Game (1994)[!].iso\" BINARY\r\n  TRACK 01 MODE1/2048\r\n    INDEX 01 00:00:00\r\nFILE \"Game (Track 2 of 2).wav\" WAVE\r\n  TRACK 02 AUDIO\r\n    PREGAP 00:02:00\r\n    INDEX 01 00:00:00\r\n",
        )
        .unwrap();
        assert_eq!(sheet.files.len(), 2);
        assert_eq!(sheet.files[0].name, "Game (1994)[!].iso");
        assert_eq!(sheet.files[0].kind, CueFileKind::Binary);
        assert_eq!(sheet.files[1].kind, CueFileKind::Wave);
        assert_eq!(sheet.track_count(), 2);
        let (file, mode) = sheet.first_data_track().unwrap();
        assert_eq!(file.name, "Game (1994)[!].iso");
        assert_eq!(mode, CueTrackMode::Mode1Cooked);
    }

    #[test]
    fn names_that_leave_the_folder_are_refused() {
        for name in [
            "../escape.iso",
            "/etc/passwd",
            "sub/dir.iso",
            "C:\\x.iso",
            "..",
            "-rf",
            "a*.iso",
            " padded.iso",
        ] {
            let text =
                format!("FILE \"{name}\" BINARY\n TRACK 01 MODE1/2048\n INDEX 01 00:00:00\n");
            assert!(
                matches!(
                    parse_cue_sheet(text.as_bytes()),
                    Err(CueRefusal::UnsafeFileName(1))
                ),
                "{name:?}"
            );
        }
    }

    #[test]
    fn anything_not_understood_is_refused_rather_than_guessed() {
        let cases: &[(&[u8], CueRefusal)] = &[
            (b"", CueRefusal::Empty),
            (b"TRACK 01 AUDIO\n", CueRefusal::TrackWithoutFile(1)),
            (
                b"FILE \"a.iso\" BINARY\n",
                CueRefusal::FileWithoutTrack("a.iso".into()),
            ),
            (
                b"FILE \"a.mp3\" MP3\n TRACK 01 AUDIO\n",
                CueRefusal::UnsupportedFileType(1, "MP3".into()),
            ),
            (
                b"FILE \"a.bin\" BINARY\n TRACK 01 CDG\n",
                CueRefusal::UnsupportedTrackMode(2, "CDG".into()),
            ),
            (
                b"FILE \"a.bin\" BINARY\n TRACK 02 AUDIO\n",
                CueRefusal::TrackOrder(2),
            ),
            (
                b"FILE \"a.bin\" BINARY\n TRACK 01 AUDIO\nFILE \"A.BIN\" BINARY\n TRACK 02 AUDIO\n",
                CueRefusal::DuplicateFile(3),
            ),
            (b"CDTEXTFILE \"x.cdt\"\n", CueRefusal::UnknownLine(1)),
            (b"FILE \"a.bin\" BINARY extra\n", CueRefusal::UnknownLine(1)),
            (b"\xff\xfe", CueRefusal::NotText),
            (
                b"REM x\rFILE \"/abs/other\" BINARY\nFILE \"a.bin\" BINARY\n TRACK 01 AUDIO\n",
                CueRefusal::NotText,
            ),
            (
                b"REM bell\x07\nFILE \"a.bin\" BINARY\n TRACK 01 AUDIO\n",
                CueRefusal::NotText,
            ),
        ];
        for (bytes, expected) in cases {
            assert_eq!(parse_cue_sheet(bytes).as_ref(), Err(expected), "{bytes:?}");
        }
        assert_eq!(
            parse_cue_sheet(&vec![b' '; MAX_CUE_SHEET_BYTES + 1]),
            Err(CueRefusal::TooLarge)
        );
    }

    #[test]
    fn a_byte_order_mark_and_an_unquoted_name_are_accepted() {
        let sheet = parse_cue_sheet(
            b"\xef\xbb\xbfFILE game.bin BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
        )
        .unwrap();
        assert_eq!(sheet.files[0].name, "game.bin");
        assert_eq!(sheet.first_data_track().unwrap().1, CueTrackMode::Mode1Raw);
    }
}
