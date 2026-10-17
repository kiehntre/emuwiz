//! One loose PlayStation CUE/BIN disc as one library entry.
//!
//! The `.cue` is the entry; the files it references are its companions and
//! never become entries of their own. A sheet is accepted only when the
//! existing strict parsers accept it: every `FILE` reference is safe (no
//! absolute path, no `..`, stays inside the sheet's folder), present, a
//! regular file and not a symlink; every file is `BINARY`; track 1 is the
//! disc's only data track, raw 2352-byte sectors; any other tracks are audio;
//! and INDEX 00 / PREGAP placement is what `resolve_data_track` understands.
//! A sheet sharing a referenced file with another sheet in the same folder is
//! refused: one disc must never appear as two games. Nothing is modified.
use super::cue_bin::{
    CueDataTrackMode, CueError, CueTrackMode, resolve_cue, resolve_cue_all_files_lenient,
    resolve_cue_layout, resolve_data_track,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Sibling sheets examined for shared companions. Beyond this a folder is too
/// crowded to prove a disc is not claimed twice, so its sheets are refused.
const MAX_SIBLING_SHEETS: usize = 64;

/// An accepted disc: the sheet and its companions in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ps1CueDisc {
    pub cue: PathBuf,
    pub companions: Vec<PathBuf>,
    pub track_count: usize,
}

/// Why a sheet is not a PlayStation disc entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ps1CueRefusal {
    /// The sheet itself is not a readable regular file, or does not parse.
    Malformed(String),
    /// A referenced file does not exist.
    MissingCompanion(PathBuf),
    /// A reference escapes the folder, or names a symlink or non-file.
    UnsafeCompanion(String),
    /// The track layout is not a PlayStation layout this build understands.
    UnsupportedLayout(String),
    /// Another sheet in the folder references the same file, or the folder
    /// holds too many sheets to tell.
    SharedCompanion(PathBuf),
}

impl std::fmt::Display for Ps1CueRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(detail) => write!(formatter, "the CUE sheet is malformed: {detail}"),
            Self::MissingCompanion(path) => write!(
                formatter,
                "a file the CUE sheet needs is missing: {}",
                path.display()
            ),
            Self::UnsafeCompanion(detail) => {
                write!(
                    formatter,
                    "the CUE sheet references an unsafe file: {detail}"
                )
            }
            Self::UnsupportedLayout(detail) => {
                write!(formatter, "the disc layout is not supported: {detail}")
            }
            Self::SharedCompanion(path) => write!(
                formatter,
                "another CUE sheet in this folder uses the same file, so which game it belongs to is ambiguous: {}",
                path.display()
            ),
        }
    }
}

fn is_cue(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("cue"))
}

fn refusal(error: CueError) -> Ps1CueRefusal {
    match error {
        CueError::MissingDataFile(path) => Ps1CueRefusal::MissingCompanion(path),
        CueError::UnsafeReference => {
            Ps1CueRefusal::UnsafeCompanion("a reference leaves the sheet's folder".into())
        }
        CueError::AmbiguousDataTracks | CueError::UnsupportedTrackMode(_) => {
            Ps1CueRefusal::UnsupportedLayout(error.to_string())
        }
        other => Ps1CueRefusal::Malformed(other.to_string()),
    }
}

/// The companions `cue` references, in the sheet's own spelling, or why they
/// are not all usable. The strict resolver proves each reference stays in the
/// sheet's folder; the spelled path is then checked itself, so a reference
/// that is a symlink is refused rather than silently followed.
fn companions(cue: &Path) -> Result<Vec<PathBuf>, Ps1CueRefusal> {
    let mut canonical = Vec::new();
    for reference in resolve_cue_all_files_lenient(cue).map_err(refusal)? {
        canonical.push(reference.map_err(refusal)?);
    }
    let spelled = resolve_cue(cue).map_err(refusal)?.referenced_paths;
    if spelled.len() != canonical.len() {
        return Err(Ps1CueRefusal::Malformed(
            "a FILE reference is not a quoted file name".into(),
        ));
    }
    let mut files: Vec<PathBuf> = Vec::new();
    for (spelled, canonical) in spelled.into_iter().zip(canonical) {
        match std::fs::symlink_metadata(&spelled) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(Ps1CueRefusal::UnsafeCompanion(format!(
                    "{} is a symlink",
                    spelled.display()
                )));
            }
            Ok(metadata) if !metadata.is_file() => {
                return Err(Ps1CueRefusal::UnsafeCompanion(format!(
                    "{} is not a regular file",
                    spelled.display()
                )));
            }
            Ok(_) => {}
            Err(_) => return Err(Ps1CueRefusal::MissingCompanion(spelled)),
        }
        if std::fs::canonicalize(&spelled).ok().as_ref() != Some(&canonical) {
            return Err(Ps1CueRefusal::UnsafeCompanion(format!(
                "{} does not resolve to the file the sheet names",
                spelled.display()
            )));
        }
        if !files.contains(&spelled) {
            files.push(spelled);
        }
    }
    Ok(files)
}

/// Inspects one loose sheet. Reads only the sheet, its siblings' sheets and
/// file metadata; never a track's contents.
pub fn inspect_ps1_cue_disc(cue: &Path) -> Result<Ps1CueDisc, Ps1CueRefusal> {
    match std::fs::symlink_metadata(cue) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        _ => {
            return Err(Ps1CueRefusal::Malformed(
                "the sheet is not a regular file".into(),
            ));
        }
    }
    let companions = companions(cue)?;
    let layout = resolve_cue_layout(cue).map_err(refusal)?;
    for track in &layout.tracks {
        if track.file_type != "BINARY" {
            return Err(Ps1CueRefusal::UnsupportedLayout(format!(
                "track {} is stored as {}, not BINARY",
                track.number, track.file_type
            )));
        }
        let first = track.number == layout.tracks[0].number;
        match (&track.mode, first) {
            (CueTrackMode::Data(CueDataTrackMode::Mode2_2352), true)
            | (CueTrackMode::Data(CueDataTrackMode::Mode1_2352), true)
            | (CueTrackMode::Audio, false) => {}
            (CueTrackMode::Data(_), true) => {
                return Err(Ps1CueRefusal::UnsupportedLayout(
                    "the data track is not stored as raw 2352-byte sectors".into(),
                ));
            }
            _ => {
                return Err(Ps1CueRefusal::UnsupportedLayout(format!(
                    "track {} is not where a PlayStation disc keeps that kind of track",
                    track.number
                )));
            }
        }
    }
    // The data track's sector size, extent and INDEX 00 / PREGAP placement.
    resolve_data_track(cue).map_err(refusal)?;
    reject_shared_companions(cue, &companions)?;
    Ok(Ps1CueDisc {
        cue: cue.to_path_buf(),
        companions,
        track_count: layout.tracks.len(),
    })
}

fn reject_shared_companions(cue: &Path, mine: &[PathBuf]) -> Result<(), Ps1CueRefusal> {
    let Some(parent) = cue.parent() else {
        return Ok(());
    };
    let canonical = |path: &Path| std::fs::canonicalize(path).ok();
    let mine: BTreeSet<PathBuf> = mine.iter().filter_map(|path| canonical(path)).collect();
    let own = canonical(cue);
    let entries =
        std::fs::read_dir(parent).map_err(|error| Ps1CueRefusal::Malformed(error.to_string()))?;
    let mut sheets = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_cue(&path) || canonical(&path) == own {
            continue;
        }
        sheets += 1;
        if sheets > MAX_SIBLING_SHEETS {
            return Err(Ps1CueRefusal::SharedCompanion(parent.to_path_buf()));
        }
        let Ok(references) = resolve_cue_all_files_lenient(&path) else {
            continue;
        };
        if let Some(shared) = references
            .into_iter()
            .flatten()
            .find(|reference| mine.contains(reference))
        {
            return Err(Ps1CueRefusal::SharedCompanion(shared));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "ps1_cue_disc_tests.rs"]
mod tests;
