//! Bounded, read-only LaserDisc game-set verification.
//!
//! Daphne/Hypseus-style sets are collections, not one image: a framefile (or
//! equivalent mapping), referenced video, and often ROM/script/config files
//! must agree.  This module reports that coherence without decoding video,
//! executing scripts, or choosing an identity from a directory name.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

pub const MAX_SET_ENTRIES: usize = 4096;
pub const MAX_FRAMEFILE_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_FRAMEFILE_LINES: usize = 32_768;
const MAX_FFPROBE_OUTPUT: usize = 128 * 1024;
const FFPROBE_TIMEOUT: Duration = Duration::from_secs(5);

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaserdiscProbeStatus {
    Available,
    Unavailable,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaserdiscMediaMetadata {
    pub container_format: Option<String>,
    pub video_codec: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Kept as the ffprobe rational/text value; no guessed floating-point FPS
    /// is used for frame-range validation.
    pub frame_rate: Option<String>,
    pub duration_millis: Option<u64>,
    pub reported_frame_count: Option<u64>,
    pub audio_stream_count: usize,
    pub video_stream_count: usize,
    pub probe_status: LaserdiscProbeStatus,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaserdiscFrameRangeStatus {
    RangeValid,
    RangeExceedsMedia,
    RangeUnverified,
    MetadataUnavailable,
    MalformedMapping,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaserdiscFrameRangeEvidence {
    pub media_name: String,
    pub first_referenced_frame: u64,
    pub last_referenced_frame: u64,
    pub status: LaserdiscFrameRangeStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameMapping {
    pub start_frame: u64,
    pub media_name: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoAssetEvidence {
    pub media_name: String,
    pub path: PathBuf,
    pub exists: bool,
    pub readable: bool,
    pub size_bytes: Option<u64>,
    pub metadata_note: Option<String>,
    pub metadata: Option<LaserdiscMediaMetadata>,
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
    pub frame_ranges: Vec<LaserdiscFrameRangeEvidence>,
    pub frame_range_status: LaserdiscFrameRangeStatus,
    pub rom_components: Vec<PathBuf>,
    pub script_components: Vec<PathBuf>,
    pub config_components: Vec<PathBuf>,
    pub warnings: Vec<String>,
    pub readiness: LaserdiscReadiness,
}

fn parse_u64(value: Option<&Value>) -> Option<u64> {
    value
        .and_then(Value::as_str)
        .and_then(|text| text.parse().ok())
        .or_else(|| value.and_then(Value::as_u64))
}

fn parse_u32(value: Option<&Value>) -> Option<u32> {
    parse_u64(value).and_then(|value| u32::try_from(value).ok())
}

fn parse_duration_millis(value: Option<&Value>) -> Option<u64> {
    let text = value?.as_str()?;
    let seconds = text.parse::<f64>().ok()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    let millis = seconds * 1000.0;
    (millis <= u64::MAX as f64).then(|| millis.round() as u64)
}

fn probe_error_metadata(status: LaserdiscProbeStatus, warning: String) -> LaserdiscMediaMetadata {
    LaserdiscMediaMetadata {
        container_format: None,
        video_codec: None,
        width: None,
        height: None,
        frame_rate: None,
        duration_millis: None,
        reported_frame_count: None,
        audio_stream_count: 0,
        video_stream_count: 0,
        probe_status: status,
        warnings: vec![warning],
    }
}

fn read_bounded(mut reader: impl Read) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    while bytes.len() < MAX_FFPROBE_OUTPUT {
        let limit = (MAX_FFPROBE_OUTPUT - bytes.len()).min(buffer.len());
        match reader.read(&mut buffer[..limit]) {
            Ok(0) | Err(_) => break,
            Ok(read) => bytes.extend_from_slice(&buffer[..read]),
        }
    }
    bytes
}

fn run_ffprobe(executable: &Path, media: &Path) -> Result<Vec<u8>, String> {
    let mut child = Command::new(executable)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_entries",
            "stream=index,codec_type,codec_name,width,height,avg_frame_rate,r_frame_rate,duration,nb_frames:format=format_name,duration",
            "-show_streams",
            "-show_format",
        ])
        .arg(media)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "ffprobe stdout unavailable".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "ffprobe stderr unavailable".to_string())?;
    let stdout_thread = thread::spawn(move || read_bounded(stdout));
    let stderr_thread = thread::spawn(move || read_bounded(stderr));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < FFPROBE_TIMEOUT => {
                thread::sleep(Duration::from_millis(20))
            }
            Ok(None) => {
                let _ = child.kill();
                return Err("ffprobe timed out".into());
            }
            Err(error) => {
                let _ = child.kill();
                return Err(error.to_string());
            }
        }
    };
    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();
    if !status.success() {
        return Err(String::from_utf8_lossy(&stderr).trim().to_string());
    }
    Ok(stdout)
}

fn parse_ffprobe_metadata(bytes: &[u8]) -> Result<LaserdiscMediaMetadata, String> {
    let value: Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let streams = value
        .get("streams")
        .and_then(Value::as_array)
        .ok_or_else(|| "ffprobe output has no streams array".to_string())?;
    let video: Vec<&Value> = streams
        .iter()
        .filter(|stream| stream.get("codec_type").and_then(Value::as_str) == Some("video"))
        .collect();
    let audio_count = streams
        .iter()
        .filter(|stream| stream.get("codec_type").and_then(Value::as_str) == Some("audio"))
        .count();
    let mut warnings = Vec::new();
    if video.is_empty() {
        warnings.push("no video stream reported".into());
    }
    if video.len() > 1 {
        warnings.push("multiple video streams reported; first stream used for metadata".into());
    }
    let first = video.first().copied();
    let frame_count = first.and_then(|stream| parse_u64(stream.get("nb_frames")));
    if first.is_some() && frame_count.is_none() {
        warnings.push("video frame count was not reported; range remains unverified".into());
    }
    let format = value.get("format");
    Ok(LaserdiscMediaMetadata {
        container_format: format
            .and_then(|format| format.get("format_name"))
            .and_then(Value::as_str)
            .map(str::to_string),
        video_codec: first
            .and_then(|stream| stream.get("codec_name"))
            .and_then(Value::as_str)
            .map(str::to_string),
        width: first.and_then(|stream| parse_u32(stream.get("width"))),
        height: first.and_then(|stream| parse_u32(stream.get("height"))),
        frame_rate: first
            .and_then(|stream| stream.get("avg_frame_rate"))
            .and_then(Value::as_str)
            .filter(|rate| !rate.is_empty() && *rate != "0/0")
            .map(str::to_string),
        duration_millis: first
            .and_then(|stream| parse_duration_millis(stream.get("duration")))
            .or_else(|| format.and_then(|format| parse_duration_millis(format.get("duration")))),
        reported_frame_count: frame_count,
        audio_stream_count: audio_count,
        video_stream_count: video.len(),
        probe_status: LaserdiscProbeStatus::Available,
        warnings,
    })
}

fn probe_media(path: &Path) -> LaserdiscMediaMetadata {
    let executable = std::env::var_os("ARCHIVEFS_FFPROBE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("ffprobe"));
    match run_ffprobe(&executable, path) {
        Ok(bytes) => match parse_ffprobe_metadata(&bytes) {
            Ok(metadata) => metadata,
            Err(error) => probe_error_metadata(
                LaserdiscProbeStatus::Failed,
                format!("ffprobe metadata was malformed: {error}"),
            ),
        },
        Err(error) if error.contains("No such file") || error.contains("not found") => {
            probe_error_metadata(
                LaserdiscProbeStatus::Unavailable,
                "ffprobe is not installed; media metadata unavailable".into(),
            )
        }
        Err(error) => probe_error_metadata(
            LaserdiscProbeStatus::Failed,
            format!("ffprobe failed: {error}"),
        ),
    }
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

fn verify_frame_ranges(
    mappings: &[FrameMapping],
    metadata: &[VideoAssetEvidence],
    malformed_mapping: bool,
) -> (Vec<LaserdiscFrameRangeEvidence>, LaserdiscFrameRangeStatus) {
    let mut by_media = std::collections::BTreeMap::<&str, Vec<u64>>::new();
    for mapping in mappings {
        by_media
            .entry(mapping.media_name.as_str())
            .or_default()
            .push(mapping.start_frame);
    }
    let mut statuses = std::collections::BTreeMap::<&str, LaserdiscFrameRangeStatus>::new();
    for (index, mapping) in mappings.iter().enumerate() {
        let status = if malformed_mapping {
            LaserdiscFrameRangeStatus::MalformedMapping
        } else {
            let asset = metadata
                .iter()
                .find(|asset| asset.media_name == mapping.media_name);
            match asset.and_then(|asset| asset.metadata.as_ref()) {
                Some(metadata) if metadata.probe_status == LaserdiscProbeStatus::Available => {
                    match metadata.reported_frame_count {
                        Some(frame_count) if frame_count == 0 => {
                            LaserdiscFrameRangeStatus::RangeExceedsMedia
                        }
                        Some(frame_count) if mappings.len() == 1 => {
                            if mapping.start_frame < frame_count {
                                LaserdiscFrameRangeStatus::RangeValid
                            } else {
                                LaserdiscFrameRangeStatus::RangeExceedsMedia
                            }
                        }
                        Some(frame_count) => {
                            let required = mappings
                                .get(index + 1)
                                .and_then(|next| next.start_frame.checked_sub(mapping.start_frame));
                            match required {
                                Some(0) => LaserdiscFrameRangeStatus::MalformedMapping,
                                Some(required) if required > frame_count => {
                                    LaserdiscFrameRangeStatus::RangeExceedsMedia
                                }
                                Some(_) => LaserdiscFrameRangeStatus::RangeValid,
                                None => LaserdiscFrameRangeStatus::RangeUnverified,
                            }
                        }
                        None => LaserdiscFrameRangeStatus::RangeUnverified,
                    }
                }
                Some(_) | None => LaserdiscFrameRangeStatus::MetadataUnavailable,
            }
        };
        let entry = statuses
            .entry(mapping.media_name.as_str())
            .or_insert(status);
        *entry = match (*entry, status) {
            (LaserdiscFrameRangeStatus::RangeExceedsMedia, _)
            | (_, LaserdiscFrameRangeStatus::RangeExceedsMedia) => {
                LaserdiscFrameRangeStatus::RangeExceedsMedia
            }
            (LaserdiscFrameRangeStatus::MalformedMapping, _)
            | (_, LaserdiscFrameRangeStatus::MalformedMapping) => {
                LaserdiscFrameRangeStatus::MalformedMapping
            }
            (LaserdiscFrameRangeStatus::MetadataUnavailable, _)
            | (_, LaserdiscFrameRangeStatus::MetadataUnavailable) => {
                LaserdiscFrameRangeStatus::MetadataUnavailable
            }
            (LaserdiscFrameRangeStatus::RangeUnverified, _)
            | (_, LaserdiscFrameRangeStatus::RangeUnverified) => {
                LaserdiscFrameRangeStatus::RangeUnverified
            }
            _ => LaserdiscFrameRangeStatus::RangeValid,
        };
    }
    let mut ranges = Vec::new();
    for (media_name, mut starts) in by_media {
        starts.sort_unstable();
        let first = starts[0];
        let last = *starts.last().unwrap_or(&first);
        let status = statuses
            .get(media_name)
            .copied()
            .unwrap_or(LaserdiscFrameRangeStatus::MetadataUnavailable);
        ranges.push(LaserdiscFrameRangeEvidence {
            media_name: media_name.to_string(),
            first_referenced_frame: first,
            last_referenced_frame: last,
            status,
        });
    }
    let overall = if malformed_mapping || mappings.is_empty() {
        LaserdiscFrameRangeStatus::MalformedMapping
    } else if ranges
        .iter()
        .any(|range| range.status == LaserdiscFrameRangeStatus::MalformedMapping)
    {
        LaserdiscFrameRangeStatus::MalformedMapping
    } else if ranges
        .iter()
        .any(|range| range.status == LaserdiscFrameRangeStatus::RangeExceedsMedia)
    {
        LaserdiscFrameRangeStatus::RangeExceedsMedia
    } else if ranges
        .iter()
        .any(|range| range.status == LaserdiscFrameRangeStatus::MetadataUnavailable)
    {
        LaserdiscFrameRangeStatus::MetadataUnavailable
    } else if ranges
        .iter()
        .any(|range| range.status == LaserdiscFrameRangeStatus::RangeUnverified)
    {
        LaserdiscFrameRangeStatus::RangeUnverified
    } else {
        LaserdiscFrameRangeStatus::RangeValid
    };
    (ranges, overall)
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
                    let metadata = probe_media(&path);
                    for warning in &metadata.warnings {
                        warnings.push(format!("{}: {warning}", name));
                    }
                    video_assets.push(VideoAssetEvidence {
                        media_name: name.clone(),
                        path: path.clone(),
                        exists: true,
                        readable: fs::File::open(&path).is_ok(),
                        size_bytes: Some(meta.len()),
                        metadata_note: Some("bounded ffprobe summary; no full decode".into()),
                        metadata: Some(metadata),
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
    let malformed_mapping = warnings.iter().any(|warning| {
        warning.contains("malformed")
            || warning.contains("invalid frame")
            || warning.contains("path traversal")
            || warning.contains("absolute media")
    });
    let (frame_ranges, frame_range_status) =
        verify_frame_ranges(&mappings, &video_assets, malformed_mapping);
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
        || malformed_mapping
        || frame_range_status == LaserdiscFrameRangeStatus::RangeExceedsMedia
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
        frame_ranges,
        frame_range_status,
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

    #[test]
    fn parses_bounded_ffprobe_summary_without_using_duration_as_frame_count() {
        let json = br#"{
            "streams": [
                {"codec_type":"video","codec_name":"mpeg2video","width":640,"height":480,
                 "avg_frame_rate":"30000/1001","duration":"12.5","nb_frames":"375"},
                {"codec_type":"audio","codec_name":"pcm_s16le"}
            ],
            "format": {"format_name":"mpeg","duration":"12.5"}
        }"#;
        let metadata = parse_ffprobe_metadata(json).unwrap();
        assert_eq!(metadata.container_format.as_deref(), Some("mpeg"));
        assert_eq!(metadata.video_codec.as_deref(), Some("mpeg2video"));
        assert_eq!(metadata.width, Some(640));
        assert_eq!(metadata.height, Some(480));
        assert_eq!(metadata.frame_rate.as_deref(), Some("30000/1001"));
        assert_eq!(metadata.duration_millis, Some(12_500));
        assert_eq!(metadata.reported_frame_count, Some(375));
        assert_eq!(metadata.audio_stream_count, 1);
        assert_eq!(metadata.video_stream_count, 1);
        assert_eq!(metadata.probe_status, LaserdiscProbeStatus::Available);
    }

    #[test]
    fn frame_ranges_distinguish_valid_excess_and_unverified() {
        let metadata = |frames| VideoAssetEvidence {
            media_name: "video.m2v".into(),
            path: PathBuf::from("video.m2v"),
            exists: true,
            readable: true,
            size_bytes: Some(1),
            metadata_note: None,
            metadata: Some(LaserdiscMediaMetadata {
                container_format: Some("mpeg".into()),
                video_codec: Some("mpeg2video".into()),
                width: Some(640),
                height: Some(480),
                frame_rate: Some("30000/1001".into()),
                duration_millis: Some(1_000),
                reported_frame_count: frames,
                audio_stream_count: 0,
                video_stream_count: 1,
                probe_status: LaserdiscProbeStatus::Available,
                warnings: Vec::new(),
            }),
        };
        let mapping = |frame| FrameMapping {
            start_frame: frame,
            media_name: "video.m2v".into(),
            line: 1,
        };
        let (ranges, status) = verify_frame_ranges(&[mapping(374)], &[metadata(Some(375))], false);
        assert_eq!(status, LaserdiscFrameRangeStatus::RangeValid);
        assert_eq!(ranges[0].last_referenced_frame, 374);
        let (_, status) = verify_frame_ranges(&[mapping(375)], &[metadata(Some(375))], false);
        assert_eq!(status, LaserdiscFrameRangeStatus::RangeExceedsMedia);
        let (_, status) = verify_frame_ranges(&[mapping(10)], &[metadata(None)], false);
        assert_eq!(status, LaserdiscFrameRangeStatus::RangeUnverified);
    }

    #[test]
    fn unavailable_metadata_does_not_downgrade_complete_set() {
        let d = set();
        write(&d.path().join("game.framefile"), b"0 video.m2v\n");
        write(&d.path().join("video.m2v"), b"video");
        write(&d.path().join("game.rom"), b"rom");
        let evidence = verify_laserdisc_set(d.path()).unwrap();
        assert_eq!(evidence.readiness, LaserdiscReadiness::Ready);
        assert!(matches!(
            evidence.frame_range_status,
            LaserdiscFrameRangeStatus::MetadataUnavailable
                | LaserdiscFrameRangeStatus::RangeUnverified
        ));
    }
}
