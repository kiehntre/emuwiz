//! Bounded, read-only LaserDisc game-set verification.
//!
//! Daphne/Hypseus-style sets are collections, not one image: a framefile (or
//! equivalent mapping), referenced video, and often ROM/script/config files
//! must agree.  This module reports that coherence without decoding video,
//! executing scripts, or choosing an identity from a directory name.

use std::fs;
use std::path::{Path, PathBuf};

pub const MAX_SET_ENTRIES: usize = 4096;
pub const MAX_FRAMEFILE_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_FRAMEFILE_LINES: usize = 32_768;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaserdiscFamily {
    Daphne,
    HypseusSinge,
    Mame,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaserdiscReadiness {
    Ready,
    Partial,
    Broken,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameMapping {
    pub start_frame: u64,
    pub media_name: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoAssetEvidence {
    pub path: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub size_bytes: Option<u64>,
    pub metadata_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaserdiscSetEvidence {
    pub set_root: PathBuf,
    pub detected_family: LaserdiscFamily,
    pub framefile_path: Option<PathBuf>,
    pub mappings: Vec<FrameMapping>,
    pub referenced_media: Vec<String>,
    pub present_media: Vec<PathBuf>,
    pub missing_media: Vec<String>,
    pub video_assets: Vec<VideoAssetEvidence>,
    pub rom_components: Vec<PathBuf>,
    pub script_components: Vec<PathBuf>,
    pub config_components: Vec<PathBuf>,
    pub warnings: Vec<String>,
    pub readiness: LaserdiscReadiness,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaserdiscVerificationError {
    NotDirectory,
    Unreadable(String),
    TooLarge { bytes: u64, maximum: u64 },
}

impl std::fmt::Display for LaserdiscVerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotDirectory => f.write_str("LaserDisc set root is not a directory"),
            Self::Unreadable(e) => write!(f, "LaserDisc set is unreadable: {e}"),
            Self::TooLarge { bytes, maximum } => {
                write!(f, "framefile is {bytes} bytes (maximum {maximum})")
            }
        }
    }
}
impl std::error::Error for LaserdiscVerificationError {}

fn is_video(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("m2v" | "mpg" | "mpeg" | "mp4" | "ogv" | "avi" | "webm" | "vob")
    )
}
fn is_rom(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("rom" | "bin" | "zip")
    )
}
fn is_script(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("singe" | "script")
    )
}
fn is_config(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("ini" | "cfg" | "xml")
    )
}

fn framefile_candidate(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    name.contains("framefile")
        || path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("frame"))
            .unwrap_or(false)
}

fn safe_reference(root: &Path, value: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(value);
    if candidate.is_absolute() {
        return Err("absolute media reference is not allowed".into());
    }
    if candidate
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("path traversal media reference is not allowed".into());
    }
    let joined = root.join(candidate);
    let canonical_root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let canonical_parent = joined.parent().and_then(|p| fs::canonicalize(p).ok());
    if let Some(parent) = canonical_parent {
        if !parent.starts_with(&canonical_root) {
            return Err("media reference escapes set root".into());
        }
    }
    Ok(joined)
}

fn parse_framefile(
    path: &Path,
    root: &Path,
) -> Result<(Vec<FrameMapping>, Vec<String>), LaserdiscVerificationError> {
    let meta =
        fs::metadata(path).map_err(|e| LaserdiscVerificationError::Unreadable(e.to_string()))?;
    if meta.len() > MAX_FRAMEFILE_BYTES {
        return Err(LaserdiscVerificationError::TooLarge {
            bytes: meta.len(),
            maximum: MAX_FRAMEFILE_BYTES,
        });
    }
    let text = fs::read_to_string(path)
        .map_err(|e| LaserdiscVerificationError::Unreadable(e.to_string()))?;
    let mut mappings = Vec::new();
    let mut warnings = Vec::new();
    let mut seen = std::collections::BTreeMap::<u64, String>::new();
    for (index, raw) in text.lines().take(MAX_FRAMEFILE_LINES).enumerate() {
        let line = index + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = trimmed
            .splitn(2, |c: char| c.is_whitespace() || c == '|')
            .map(str::trim)
            .collect();
        if fields.len() != 2 {
            warnings.push(format!("line {line}: malformed frame mapping"));
            continue;
        }
        let Ok(frame) = fields[0].parse::<u64>() else {
            warnings.push(format!("line {line}: invalid frame number"));
            continue;
        };
        let media = fields[1].trim_matches('"');
        if media.is_empty() {
            warnings.push(format!("line {line}: empty media reference"));
            continue;
        }
        if let Err(reason) = safe_reference(root, media) {
            warnings.push(format!("line {line}: {reason}"));
            continue;
        }
        if let Some(previous) = seen.insert(frame, media.to_string()) {
            if previous != media {
                warnings.push(format!(
                    "line {line}: conflicting mapping for frame {frame}"
                ));
            } else {
                warnings.push(format!("line {line}: duplicate mapping for frame {frame}"));
            }
        }
        mappings.push(FrameMapping {
            start_frame: frame,
            media_name: media.to_string(),
            line,
        });
    }
    if text.lines().count() > MAX_FRAMEFILE_LINES {
        warnings.push("framefile line limit reached".into());
    }
    mappings.sort_by_key(|m| (m.start_frame, m.line));
    for pair in mappings.windows(2) {
        if pair[0].start_frame == pair[1].start_frame {
            continue;
        }
    }
    Ok((mappings, warnings))
}

/// Verify one set root. Only direct children are considered; this avoids
/// accidentally combining unrelated sibling titles and keeps work bounded.
pub fn verify_laserdisc_set(
    root: &Path,
) -> Result<LaserdiscSetEvidence, LaserdiscVerificationError> {
    if !root.is_dir() {
        return Err(LaserdiscVerificationError::NotDirectory);
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(root)
        .map_err(|e| LaserdiscVerificationError::Unreadable(e.to_string()))?
        .take(MAX_SET_ENTRIES)
    {
        let entry = entry.map_err(|e| LaserdiscVerificationError::Unreadable(e.to_string()))?;
        if entry
            .file_type()
            .map_err(|e| LaserdiscVerificationError::Unreadable(e.to_string()))?
            .is_file()
        {
            files.push(entry.path());
        }
    }
    let mut warnings = Vec::new();
    let mut framefiles: Vec<PathBuf> = files
        .iter()
        .filter(|p| framefile_candidate(p))
        .cloned()
        .collect();
    framefiles.sort();
    if framefiles.len() > 1 {
        warnings.push("multiple competing framefiles found; no automatic winner".into());
    }
    let framefile_path = framefiles.first().cloned();
    let (mappings, parse_warnings) = if let Some(path) = &framefile_path {
        parse_framefile(path, root)?
    } else {
        (Vec::new(), Vec::new())
    };
    warnings.extend(parse_warnings);
    let referenced_media: Vec<String> = {
        let mut v: Vec<_> = mappings.iter().map(|m| m.media_name.clone()).collect();
        v.sort();
        v.dedup();
        v
    };
    let mut present_media = Vec::new();
    let mut missing_media = Vec::new();
    let mut video_assets = Vec::new();
    for name in &referenced_media {
        match safe_reference(root, name) {
            Ok(path) => match fs::metadata(&path) {
                Ok(meta) if meta.is_file() && meta.len() > 0 => {
                    present_media.push(path.clone());
                    video_assets.push(VideoAssetEvidence {
                        path: path.clone(),
                        exists: true,
                        readable: fs::File::open(&path).is_ok(),
                        size_bytes: Some(meta.len()),
                        metadata_note: Some(
                            "container/frame metadata not decoded; bounded stat only".into(),
                        ),
                    });
                }
                Ok(_) => {
                    missing_media.push(name.clone());
                    warnings.push(format!("referenced media is empty or not a file: {name}"));
                }
                Err(_) => {
                    missing_media.push(name.clone());
                    warnings.push(format!("referenced media is missing: {name}"));
                }
            },
            Err(reason) => {
                missing_media.push(name.clone());
                warnings.push(format!("{name}: {reason}"));
            }
        }
    }
    let rom_components: Vec<PathBuf> = files.iter().filter(|p| is_rom(p)).cloned().collect();
    let script_components: Vec<PathBuf> = files.iter().filter(|p| is_script(p)).cloned().collect();
    let config_components: Vec<PathBuf> = files.iter().filter(|p| is_config(p)).cloned().collect();
    let has_hypseus = !script_components.is_empty();
    let has_daphne = framefile_path.is_some() && !rom_components.is_empty();
    let detected_family = if has_hypseus {
        LaserdiscFamily::HypseusSinge
    } else if has_daphne {
        LaserdiscFamily::Daphne
    } else if files.iter().any(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.eq_ignore_ascii_case("mame.ini") || n.eq_ignore_ascii_case("mame.cfg"))
            .unwrap_or(false)
    }) {
        LaserdiscFamily::Mame
    } else {
        LaserdiscFamily::Unknown
    };
    let readiness = if framefile_path.is_none() {
        if detected_family == LaserdiscFamily::Unknown {
            LaserdiscReadiness::Unknown
        } else {
            LaserdiscReadiness::Partial
        }
    } else if mappings.is_empty()
        || !missing_media.is_empty()
        || warnings.iter().any(|w| {
            w.contains("malformed")
                || w.contains("invalid frame")
                || w.contains("path traversal")
                || w.contains("absolute media")
        })
    {
        LaserdiscReadiness::Broken
    } else if detected_family == LaserdiscFamily::Unknown
        || (detected_family == LaserdiscFamily::HypseusSinge && script_components.is_empty())
        || (detected_family == LaserdiscFamily::Daphne && rom_components.is_empty())
    {
        LaserdiscReadiness::Partial
    } else {
        LaserdiscReadiness::Ready
    };
    Ok(LaserdiscSetEvidence {
        set_root: root.to_path_buf(),
        detected_family,
        framefile_path,
        mappings,
        referenced_media,
        present_media,
        missing_media,
        video_assets,
        rom_components,
        script_components,
        config_components,
        warnings,
        readiness,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn set() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }
    fn write(path: &Path, data: &[u8]) {
        let mut f = fs::File::create(path).unwrap();
        f.write_all(data).unwrap();
    }
    #[test]
    fn valid_daphne_ready() {
        let d = set();
        write(&d.path().join("game.framefile"), b"0 video.m2v\n");
        write(&d.path().join("video.m2v"), b"video");
        write(&d.path().join("game.rom"), b"rom");
        let e = verify_laserdisc_set(d.path()).unwrap();
        assert_eq!(e.detected_family, LaserdiscFamily::Daphne);
        assert_eq!(e.readiness, LaserdiscReadiness::Ready);
    }
    #[test]
    fn missing_media_is_broken() {
        let d = set();
        write(&d.path().join("framefile"), b"0 missing.m2v\n");
        let e = verify_laserdisc_set(d.path()).unwrap();
        assert_eq!(e.readiness, LaserdiscReadiness::Broken);
    }
    #[test]
    fn traversal_is_reported() {
        let d = set();
        write(&d.path().join("framefile"), b"0 ../outside.m2v\n");
        let e = verify_laserdisc_set(d.path()).unwrap();
        assert!(e.warnings.iter().any(|w| w.contains("traversal")));
        assert_eq!(e.readiness, LaserdiscReadiness::Broken);
    }
    #[test]
    fn generic_video_folder_is_unknown() {
        let d = set();
        write(&d.path().join("movie.m2v"), b"x");
        let e = verify_laserdisc_set(d.path()).unwrap();
        assert_eq!(e.readiness, LaserdiscReadiness::Unknown);
    }
    #[test]
    fn duplicate_conflict_is_preserved() {
        let d = set();
        write(&d.path().join("framefile"), b"0 a.m2v\n0 b.m2v\n");
        write(&d.path().join("a.m2v"), b"a");
        write(&d.path().join("b.m2v"), b"b");
        let e = verify_laserdisc_set(d.path()).unwrap();
        assert!(e.warnings.iter().any(|w| w.contains("conflicting")));
    }
}
