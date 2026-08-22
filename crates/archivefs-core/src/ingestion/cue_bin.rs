//! CUE/BIN pairing: the `.cue` sheet is the anchor for a disc-image
//! candidate; a `.bin` is only ever resolved through a `.cue` that names
//! it. A lone `.bin` with no matching `.cue` is never guessed at here -
//! see [`super::discovery`]'s `SkipReason::MissingPairedFile`.
//!
//! Parsing is read-only and bounded: CUE sheets are always small plain
//! text, so a file above [`MAX_CUE_BYTES`] is refused rather than read.

use std::path::{Path, PathBuf};

/// CUE sheets are a few hundred bytes to a few KiB. Refuse anything larger
/// as not a genuine CUE sheet rather than reading it.
const MAX_CUE_BYTES: u64 = 256 * 1024;

/// The maximum number of `FILE` references resolved from one CUE sheet
/// (multi-track/multi-session discs may reference more than one).
const MAX_CUE_FILE_REFERENCES: usize = 99;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CueError {
    Io(String),
    TooLarge,
    NoFileReferences,
}

/// One resolved CUE sheet: the sheet itself plus every `.bin` (or other
/// data file) it references, resolved relative to the sheet's own
/// directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueSheet {
    pub cue_path: PathBuf,
    pub referenced_paths: Vec<PathBuf>,
}

/// Parse a `.cue` sheet and resolve every `FILE "..." <TYPE>` reference it
/// contains relative to the sheet's directory. Read-only; the referenced
/// files are never opened here, only named.
pub fn resolve_cue(cue_path: &Path) -> Result<CueSheet, CueError> {
    let metadata = std::fs::metadata(cue_path).map_err(|error| CueError::Io(error.to_string()))?;
    if metadata.len() > MAX_CUE_BYTES {
        return Err(CueError::TooLarge);
    }
    let contents =
        std::fs::read_to_string(cue_path).map_err(|error| CueError::Io(error.to_string()))?;
    let parent = cue_path.parent().unwrap_or_else(|| Path::new("."));
    let mut referenced_paths = Vec::new();
    for line in contents.lines() {
        let trimmed = line.trim_start();
        if !trimmed.to_ascii_uppercase().starts_with("FILE") {
            continue;
        }
        if let Some(name) = extract_quoted_filename(trimmed) {
            if referenced_paths.len() >= MAX_CUE_FILE_REFERENCES {
                break;
            }
            referenced_paths.push(parent.join(name));
        }
    }
    if referenced_paths.is_empty() {
        return Err(CueError::NoFileReferences);
    }
    Ok(CueSheet {
        cue_path: cue_path.to_path_buf(),
        referenced_paths,
    })
}

/// Extracts the quoted filename from a CUE `FILE "name.bin" BINARY` line.
fn extract_quoted_filename(line: &str) -> Option<&str> {
    let start = line.find('"')? + 1;
    let end = start + line[start..].find('"')?;
    Some(&line[start..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp(name: &str, contents: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "archivefs-cue-bin-test-{}-{}",
            std::process::id(),
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn resolves_a_single_file_reference() {
        let cue = write_temp(
            "game.cue",
            "FILE \"game.bin\" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n",
        );
        let sheet = resolve_cue(&cue).unwrap();
        assert_eq!(sheet.referenced_paths.len(), 1);
        assert_eq!(
            sheet.referenced_paths[0].file_name().unwrap().to_str(),
            Some("game.bin")
        );
    }

    #[test]
    fn resolves_multiple_file_references() {
        let cue = write_temp(
            "multi.cue",
            "FILE \"multi (Track 1).bin\" BINARY\n\
             FILE \"multi (Track 2).bin\" BINARY\n",
        );
        let sheet = resolve_cue(&cue).unwrap();
        assert_eq!(sheet.referenced_paths.len(), 2);
    }

    #[test]
    fn a_cue_with_no_file_reference_is_an_error() {
        let cue = write_temp("empty.cue", "REM just a comment\n");
        assert_eq!(resolve_cue(&cue), Err(CueError::NoFileReferences));
    }
}
