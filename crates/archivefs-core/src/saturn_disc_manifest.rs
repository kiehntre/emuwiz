//! Complete, read-only Saturn optical preservation manifests.
//!
//! This module deliberately projects the existing CUE parser rather than
//! parsing CUE a second time.  A descriptor is the authority for topology;
//! a bare BIN is therefore represented as limited/incomplete evidence and is
//! never promoted to a complete Saturn disc.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ingestion::cue_bin::{
    CueError, CueLayout, CueTimestamp, CueTrack, CueTrackMode, resolve_cue_layout,
};
use crate::raw_cd_sector::{
    LOGICAL_BLOCK_BYTES, RAW_SECTOR_BYTES, detect_sector_mode, extract_user_data,
};
use crate::saturn_boot_evidence::{
    SATURN_SYSTEM_ID_BYTES, SaturnSystemIdFact, parse_saturn_system_id,
};

pub const SATURN_MANIFEST_SCHEMA: &str = "emuwiz.saturn-optical-manifest.v1";
const HASH_CHUNK_BYTES: usize = 128 * 1024;
const MAX_MANIFEST_TRACKS: usize = 99;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaturnDescriptorType {
    Cue,
    LoneBin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaturnManifestStatus {
    Complete,
    CompleteWithWarnings,
    Incomplete,
    Unsafe,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaturnTrackType {
    Data,
    Audio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaturnTrackMode {
    Mode1_2048,
    Mode1_2352,
    Mode2_2352,
    Audio,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaturnPregapKind {
    None,
    InFileIndex00,
    SyntheticPregap,
    InFileAndSynthetic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaturnComponent {
    pub path: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaturnTrackManifest {
    pub number: u32,
    pub track_type: SaturnTrackType,
    pub mode: SaturnTrackMode,
    pub source_file: PathBuf,
    pub source_component_index: usize,
    pub file_byte_offset: u64,
    pub data_byte_offset: u64,
    pub sector_size: u32,
    pub sector_count: u64,
    pub index_00_frame: Option<u64>,
    pub index_01_frame: u64,
    pub pregap_kind: SaturnPregapKind,
    pub in_file_pregap_frames: Option<u64>,
    pub declared_pregap_frames: Option<u64>,
    pub postgap_frames: Option<u64>,
    pub logical_start_frame: u64,
    pub logical_data_start_frame: u64,
    pub logical_end_frame: u64,
    pub audio_sha256: Option<String>,
    pub data_logical_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaturnSystemIdManifest {
    pub location: String,
    pub raw_hex: String,
    pub fact: SaturnSystemIdFact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaturnManifestIssue {
    MissingComponent { path: PathBuf },
    UnsafePath { detail: String },
    AmbiguousComponent { detail: String },
    InvalidCue { detail: String },
    MissingDataTrack,
    MultipleCandidateDataTracks { count: usize },
    InvalidSystemId { detail: String },
    TrackTopologyMismatch { detail: String },
    AudioHashMismatch { track: u32 },
    UnexpectedPregapChange { track: u32 },
    UnexpectedIndexChange { track: u32 },
    ComponentHashMismatch { path: PathBuf },
    ComponentSizeMismatch { path: PathBuf },
    UnsupportedTrackMode { mode: String },
    UnsupportedRepresentation { detail: String },
    DescriptorHashMismatch,
    SystemIdChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaturnPreservationStatus {
    pub audio_preserved: bool,
    pub track_topology_preserved: bool,
    pub system_id_unchanged: bool,
    pub descriptor_unchanged: bool,
    pub component_bytes_unchanged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaturnDiscManifest {
    pub schema: String,
    pub source_descriptor: PathBuf,
    pub descriptor_type: SaturnDescriptorType,
    pub descriptor_sha256: String,
    pub components: Vec<SaturnComponent>,
    pub tracks: Vec<SaturnTrackManifest>,
    pub system_id: Option<SaturnSystemIdManifest>,
    pub issues: Vec<SaturnManifestIssue>,
    pub status: SaturnManifestStatus,
    pub completeness: String,
    pub provenance: String,
    pub preservation: SaturnPreservationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaturnManifestError {
    Cue(CueError),
    Io(String),
    Unsupported(String),
}

impl std::fmt::Display for SaturnManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cue(e) => write!(f, "{e}"),
            Self::Io(e) => write!(f, "Saturn manifest I/O error: {e}"),
            Self::Unsupported(e) => write!(f, "unsupported Saturn representation: {e}"),
        }
    }
}
impl std::error::Error for SaturnManifestError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaturnManifestVerification {
    pub status: SaturnManifestStatus,
    pub issues: Vec<SaturnManifestIssue>,
    pub current: Option<SaturnDiscManifest>,
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

fn hash_file(path: &Path) -> Result<String, SaturnManifestError> {
    let mut file = File::open(path).map_err(|e| SaturnManifestError::Io(e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_CHUNK_BYTES];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|e| SaturnManifestError::Io(e.to_string()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex(hasher.finalize()))
}

fn hash_range(path: &Path, offset: u64, length: u64) -> Result<String, SaturnManifestError> {
    let mut file = File::open(path).map_err(|e| SaturnManifestError::Io(e.to_string()))?;
    let end = offset
        .checked_add(length)
        .ok_or_else(|| SaturnManifestError::Io("range overflows".into()))?;
    let size = file
        .metadata()
        .map_err(|e| SaturnManifestError::Io(e.to_string()))?
        .len();
    if end > size {
        return Err(SaturnManifestError::Io(
            "track range exceeds component".into(),
        ));
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| SaturnManifestError::Io(e.to_string()))?;
    let mut remaining = length;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_CHUNK_BYTES];
    while remaining != 0 {
        let take = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..take])
            .map_err(|e| SaturnManifestError::Io(e.to_string()))?;
        hasher.update(&buffer[..take]);
        remaining -= take as u64;
    }
    Ok(hex(hasher.finalize()))
}

fn frame_bytes(mode: &SaturnTrackMode) -> Option<u64> {
    match mode {
        SaturnTrackMode::Mode1_2048 => Some(2048),
        SaturnTrackMode::Mode1_2352 | SaturnTrackMode::Mode2_2352 | SaturnTrackMode::Audio => {
            Some(2352)
        }
        SaturnTrackMode::Unsupported => None,
    }
}

fn track_mode(track: &CueTrack) -> SaturnTrackMode {
    match track.mode {
        CueTrackMode::Data(crate::ingestion::cue_bin::CueDataTrackMode::Mode1_2048) => {
            SaturnTrackMode::Mode1_2048
        }
        CueTrackMode::Data(crate::ingestion::cue_bin::CueDataTrackMode::Mode1_2352) => {
            SaturnTrackMode::Mode1_2352
        }
        CueTrackMode::Data(crate::ingestion::cue_bin::CueDataTrackMode::Mode2_2352) => {
            SaturnTrackMode::Mode2_2352
        }
        CueTrackMode::Audio => SaturnTrackMode::Audio,
    }
}

fn frames(ts: Option<CueTimestamp>) -> Option<u64> {
    ts.map(|v| v.frames)
}

fn logical_hash(
    path: &Path,
    mode: SaturnTrackMode,
    start_frame: u64,
    count: u64,
) -> Result<String, SaturnManifestError> {
    if count == 0 {
        return Err(SaturnManifestError::Io("empty track range".into()));
    }
    let mut file = File::open(path).map_err(|e| SaturnManifestError::Io(e.to_string()))?;
    let physical = frame_bytes(&mode).ok_or_else(|| {
        SaturnManifestError::Unsupported("track mode has no verified sector geometry".into())
    })?;
    let start = start_frame
        .checked_mul(physical)
        .ok_or_else(|| SaturnManifestError::Io("sector offset overflows".into()))?;
    let bytes = count
        .checked_mul(physical)
        .ok_or_else(|| SaturnManifestError::Io("track size overflows".into()))?;
    let end = start
        .checked_add(bytes)
        .ok_or_else(|| SaturnManifestError::Io("track range overflows".into()))?;
    if end
        > file
            .metadata()
            .map_err(|e| SaturnManifestError::Io(e.to_string()))?
            .len()
    {
        return Err(SaturnManifestError::Io(
            "track range exceeds component".into(),
        ));
    }
    file.seek(SeekFrom::Start(start))
        .map_err(|e| SaturnManifestError::Io(e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut sector = vec![0_u8; physical as usize];
    for _ in 0..count {
        file.read_exact(&mut sector)
            .map_err(|e| SaturnManifestError::Io(e.to_string()))?;
        if mode == SaturnTrackMode::Mode1_2048 {
            hasher.update(&sector);
        } else if mode == SaturnTrackMode::Audio {
            hasher.update(&sector);
        } else {
            let detected = detect_sector_mode(&sector).ok_or_else(|| {
                SaturnManifestError::Unsupported("raw sector layout is not verified".into())
            })?;
            hasher.update(
                extract_user_data(&sector, detected).map_err(SaturnManifestError::Unsupported)?,
            );
        }
    }
    Ok(hex(hasher.finalize()))
}

fn boundary_frames(
    layout: &CueLayout,
    index: usize,
    track: &CueTrack,
    file_frames: u64,
) -> Result<u64, SaturnManifestError> {
    let start = frames(track.index_01)
        .ok_or_else(|| SaturnManifestError::Unsupported("track has no INDEX 01".into()))?;
    let mut boundary = file_frames;
    for next in layout
        .tracks
        .iter()
        .skip(index + 1)
        .filter(|next| next.path == track.path)
    {
        let candidate = frames(next.index_00)
            .or_else(|| frames(next.index_01))
            .ok_or_else(|| SaturnManifestError::Unsupported("track has no index".into()))?;
        if candidate <= start {
            return Err(SaturnManifestError::Unsupported(
                "decreasing file-relative track index".into(),
            ));
        }
        boundary = boundary.min(candidate);
    }
    if boundary <= start || boundary > file_frames {
        return Err(SaturnManifestError::Unsupported(
            "track boundary is outside component".into(),
        ));
    }
    Ok(boundary)
}

fn read_system_id(
    track: &SaturnTrackManifest,
) -> Result<Option<SaturnSystemIdManifest>, SaturnManifestError> {
    let Some(mode_bytes) = frame_bytes(&track.mode) else {
        return Ok(None);
    };
    let mut raw = vec![0_u8; SATURN_SYSTEM_ID_BYTES];
    let offset = track.data_byte_offset;
    let mut file =
        File::open(&track.source_file).map_err(|e| SaturnManifestError::Io(e.to_string()))?;
    let size = file
        .metadata()
        .map_err(|e| SaturnManifestError::Io(e.to_string()))?
        .len();
    if offset.checked_add(mode_bytes).is_none_or(|end| end > size) {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| SaturnManifestError::Io(e.to_string()))?;
    if track.mode == SaturnTrackMode::Mode1_2048 {
        let logical_bytes = track
            .sector_count
            .checked_mul(LOGICAL_BLOCK_BYTES as u64)
            .unwrap_or(0);
        if logical_bytes < SATURN_SYSTEM_ID_BYTES as u64 {
            return Ok(None);
        }
        file.read_exact(&mut raw)
            .map_err(|e| SaturnManifestError::Io(e.to_string()))?;
    } else {
        let mut sector = vec![0_u8; RAW_SECTOR_BYTES];
        file.read_exact(&mut sector)
            .map_err(|e| SaturnManifestError::Io(e.to_string()))?;
        let detected = detect_sector_mode(&sector).ok_or_else(|| {
            SaturnManifestError::Unsupported("data track raw sector is not verified".into())
        })?;
        let user =
            extract_user_data(&sector, detected).map_err(SaturnManifestError::Unsupported)?;
        if track
            .sector_count
            .checked_mul(LOGICAL_BLOCK_BYTES as u64)
            .unwrap_or(0)
            < SATURN_SYSTEM_ID_BYTES as u64
        {
            return Ok(None);
        }
        let take = user.len().min(raw.len());
        raw[..take].copy_from_slice(&user[..take]);
        if SATURN_SYSTEM_ID_BYTES > user.len() {
            return Ok(None);
        }
    }
    let fact = parse_saturn_system_id(&raw)
        .ok_or_else(|| SaturnManifestError::Unsupported("System ID is truncated".into()))?;
    Ok(Some(SaturnSystemIdManifest {
        location: format!("track {} logical data sector 0", track.number),
        raw_hex: hex(raw),
        fact,
    }))
}

fn status_for(issues: &[SaturnManifestIssue], complete: bool) -> SaturnManifestStatus {
    if issues.iter().any(|issue| {
        matches!(
            issue,
            SaturnManifestIssue::UnsafePath { .. }
                | SaturnManifestIssue::UnsupportedRepresentation { .. }
        )
    }) {
        return SaturnManifestStatus::Unsafe;
    }
    if issues.iter().any(|issue| {
        matches!(
            issue,
            SaturnManifestIssue::InvalidCue { .. }
                | SaturnManifestIssue::InvalidSystemId { .. }
                | SaturnManifestIssue::TrackTopologyMismatch { .. }
                | SaturnManifestIssue::ComponentHashMismatch { .. }
        )
    }) {
        return SaturnManifestStatus::Invalid;
    }
    if !complete {
        SaturnManifestStatus::Incomplete
    } else if issues.is_empty() {
        SaturnManifestStatus::Complete
    } else {
        SaturnManifestStatus::CompleteWithWarnings
    }
}

fn from_layout(path: &Path, layout: CueLayout) -> Result<SaturnDiscManifest, SaturnManifestError> {
    if layout.tracks.is_empty() || layout.tracks.len() > MAX_MANIFEST_TRACKS {
        return Err(SaturnManifestError::Unsupported(
            "track count is outside the bounded manifest limit".into(),
        ));
    }
    let descriptor_sha256 = hash_file(path)?;
    let mut components = Vec::new();
    for track in &layout.tracks {
        if !components
            .iter()
            .any(|component: &SaturnComponent| component.path == track.path)
        {
            components.push(SaturnComponent {
                path: track.path.clone(),
                sha256: hash_file(&track.path)?,
                size_bytes: std::fs::metadata(&track.path)
                    .map_err(|e| SaturnManifestError::Io(e.to_string()))?
                    .len(),
            });
        }
    }
    let mut tracks = Vec::with_capacity(layout.tracks.len());
    let mut logical_cursor = 0_u64;
    for (index, cue_track) in layout.tracks.iter().enumerate() {
        let mode = track_mode(cue_track);
        let physical_bytes = frame_bytes(&mode).ok_or_else(|| {
            SaturnManifestError::Unsupported(format!(
                "unsupported mode on track {}",
                cue_track.number
            ))
        })?;
        let size = std::fs::metadata(&cue_track.path)
            .map_err(|e| SaturnManifestError::Io(e.to_string()))?
            .len();
        if !size.is_multiple_of(physical_bytes) {
            return Err(SaturnManifestError::Unsupported(format!(
                "component is not whole-sector aligned for track {}",
                cue_track.number
            )));
        }
        let file_frames = size / physical_bytes;
        let index_01 = frames(cue_track.index_01).ok_or_else(|| {
            SaturnManifestError::Unsupported(format!("track {} has no INDEX 01", cue_track.number))
        })?;
        let boundary = boundary_frames(&layout, index, cue_track, file_frames)?;
        let in_file = cue_track
            .index_00
            .map(|index_00| index_01 - index_00.frames);
        let declared = frames(cue_track.pregap);
        let pregap_kind = match (in_file, declared) {
            (Some(_), Some(_)) => SaturnPregapKind::InFileAndSynthetic,
            (Some(_), None) => SaturnPregapKind::InFileIndex00,
            (None, Some(_)) => SaturnPregapKind::SyntheticPregap,
            (None, None) => SaturnPregapKind::None,
        };
        let pregap = in_file.or(declared).unwrap_or(0);
        let data_count = boundary - index_01;
        let logical_data_start = logical_cursor
            .checked_add(pregap)
            .ok_or_else(|| SaturnManifestError::Io("logical frame arithmetic overflows".into()))?;
        let logical_end = logical_data_start
            .checked_add(data_count)
            .and_then(|end| end.checked_add(cue_track.postgap.map(|p| p.frames).unwrap_or(0)))
            .ok_or_else(|| SaturnManifestError::Io("logical frame arithmetic overflows".into()))?;
        let component_index = components
            .iter()
            .position(|component| component.path == cue_track.path)
            .expect("component inserted from tracks");
        let byte_offset = index_01
            .checked_mul(physical_bytes)
            .ok_or_else(|| SaturnManifestError::Io("track byte offset overflows".into()))?;
        let data_logical_sha256 = if mode != SaturnTrackMode::Audio {
            Some(logical_hash(&cue_track.path, mode, index_01, data_count)?)
        } else {
            None
        };
        let audio_sha256 = if mode == SaturnTrackMode::Audio {
            Some(hash_range(
                &cue_track.path,
                byte_offset,
                data_count
                    .checked_mul(physical_bytes)
                    .ok_or_else(|| SaturnManifestError::Io("audio range overflows".into()))?,
            )?)
        } else {
            None
        };
        tracks.push(SaturnTrackManifest {
            number: cue_track.number,
            track_type: if mode == SaturnTrackMode::Audio {
                SaturnTrackType::Audio
            } else {
                SaturnTrackType::Data
            },
            mode,
            source_file: cue_track.path.clone(),
            source_component_index: component_index,
            file_byte_offset: cue_track
                .index_00
                .map(|v| v.frames)
                .unwrap_or(index_01)
                .checked_mul(physical_bytes)
                .ok_or_else(|| SaturnManifestError::Io("pregap byte offset overflows".into()))?,
            data_byte_offset: byte_offset,
            sector_size: physical_bytes as u32,
            sector_count: data_count,
            index_00_frame: frames(cue_track.index_00),
            index_01_frame: index_01,
            pregap_kind,
            in_file_pregap_frames: in_file,
            declared_pregap_frames: declared,
            postgap_frames: frames(cue_track.postgap),
            logical_start_frame: logical_cursor,
            logical_data_start_frame: logical_data_start,
            logical_end_frame: logical_end,
            audio_sha256,
            data_logical_sha256,
        });
        logical_cursor = logical_end;
    }
    let data_tracks: Vec<_> = tracks
        .iter()
        .filter(|track| track.track_type == SaturnTrackType::Data)
        .collect();
    let mut issues = Vec::new();
    let system_id = if data_tracks.is_empty() {
        issues.push(SaturnManifestIssue::MissingDataTrack);
        None
    } else if data_tracks.len() > 1 {
        issues.push(SaturnManifestIssue::MultipleCandidateDataTracks {
            count: data_tracks.len(),
        });
        None
    } else {
        let result = read_system_id(data_tracks[0])?;
        if result
            .as_ref()
            .is_none_or(|id| !id.fact.hardware_id_recognized)
        {
            issues.push(SaturnManifestIssue::InvalidSystemId { detail: "SEGA SEGASATURN signature was not present at the selected logical data-track location".into() });
        }
        result
    };
    let complete = system_id.is_some() && !tracks.is_empty();
    let preservation = SaturnPreservationStatus {
        audio_preserved: tracks.iter().all(|track| {
            track.track_type != SaturnTrackType::Audio || track.audio_sha256.is_some()
        }),
        track_topology_preserved: true,
        system_id_unchanged: system_id.is_some(),
        descriptor_unchanged: true,
        component_bytes_unchanged: true,
    };
    let status = status_for(&issues, complete);
    Ok(SaturnDiscManifest { schema: SATURN_MANIFEST_SCHEMA.into(), source_descriptor: path.to_path_buf(), descriptor_type: SaturnDescriptorType::Cue, descriptor_sha256, components, tracks, system_id, issues, status, completeness: if complete { "complete source set".into() } else { "limited or incomplete source set".into() }, provenance: "native CUE topology + source component hashes + Saturn System ID logical-sector inspection".into(), preservation })
}

/// Build a deterministic, read-only manifest from a CUE descriptor or a lone BIN.
pub fn inspect_saturn_disc(path: &Path) -> Result<SaturnDiscManifest, SaturnManifestError> {
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("bin"))
    {
        let sibling_cue = path.with_extension("cue");
        if sibling_cue.is_file() {
            return Err(SaturnManifestError::Unsupported("a descriptor exists beside this BIN; inspect the CUE so audio and topology are not lost".into()));
        }
        let component = SaturnComponent {
            path: path.to_path_buf(),
            sha256: hash_file(path)?,
            size_bytes: std::fs::metadata(path)
                .map_err(|e| SaturnManifestError::Io(e.to_string()))?
                .len(),
        };
        return Ok(SaturnDiscManifest {
            schema: SATURN_MANIFEST_SCHEMA.into(),
            source_descriptor: path.to_path_buf(),
            descriptor_type: SaturnDescriptorType::LoneBin,
            descriptor_sha256: component.sha256.clone(),
            components: vec![component],
            tracks: Vec::new(),
            system_id: None,
            issues: vec![SaturnManifestIssue::UnsupportedRepresentation {
                detail: "lone BIN cannot prove descriptor topology or whether audio tracks exist"
                    .into(),
            }],
            status: SaturnManifestStatus::Incomplete,
            completeness: "limited/incomplete media".into(),
            provenance: "bounded source-component hash only; no descriptor semantics inferred"
                .into(),
            preservation: SaturnPreservationStatus {
                audio_preserved: false,
                track_topology_preserved: false,
                system_id_unchanged: false,
                descriptor_unchanged: false,
                component_bytes_unchanged: true,
            },
        });
    }
    let layout = resolve_cue_layout(path).map_err(SaturnManifestError::Cue)?;
    from_layout(path, layout)
}

/// Re-inspect the source and compare every preservation-relevant fact.
pub fn verify_saturn_manifest(expected: &SaturnDiscManifest) -> SaturnManifestVerification {
    let current = match inspect_saturn_disc(&expected.source_descriptor) {
        Ok(current) => current,
        Err(SaturnManifestError::Cue(CueError::MissingDataFile(path))) => {
            return SaturnManifestVerification {
                status: SaturnManifestStatus::Incomplete,
                issues: vec![SaturnManifestIssue::MissingComponent { path }],
                current: None,
            };
        }
        Err(SaturnManifestError::Cue(CueError::UnsafeReference)) => {
            return SaturnManifestVerification {
                status: SaturnManifestStatus::Unsafe,
                issues: vec![SaturnManifestIssue::UnsafePath {
                    detail: "descriptor reference is outside its confined source root".into(),
                }],
                current: None,
            };
        }
        Err(error) => {
            return SaturnManifestVerification {
                status: SaturnManifestStatus::Invalid,
                issues: vec![SaturnManifestIssue::TrackTopologyMismatch {
                    detail: error.to_string(),
                }],
                current: None,
            };
        }
    };
    let mut issues = Vec::new();
    if current.descriptor_sha256 != expected.descriptor_sha256 {
        issues.push(SaturnManifestIssue::DescriptorHashMismatch);
    }
    if current.components.len() != expected.components.len() {
        issues.push(SaturnManifestIssue::TrackTopologyMismatch {
            detail: "component set changed".into(),
        });
    }
    for expected_component in &expected.components {
        match current
            .components
            .iter()
            .find(|component| component.path == expected_component.path)
        {
            None => issues.push(SaturnManifestIssue::MissingComponent {
                path: expected_component.path.clone(),
            }),
            Some(actual) => {
                if actual.sha256 != expected_component.sha256 {
                    issues.push(SaturnManifestIssue::ComponentHashMismatch {
                        path: expected_component.path.clone(),
                    });
                }
                if actual.size_bytes != expected_component.size_bytes {
                    issues.push(SaturnManifestIssue::ComponentSizeMismatch {
                        path: expected_component.path.clone(),
                    });
                }
            }
        }
    }
    if current.tracks.len() != expected.tracks.len() {
        issues.push(SaturnManifestIssue::TrackTopologyMismatch {
            detail: "track count changed".into(),
        });
    }
    for (left, right) in expected.tracks.iter().zip(current.tracks.iter()) {
        if left.number != right.number
            || left.mode != right.mode
            || left.source_file != right.source_file
            || left.sector_count != right.sector_count
        {
            issues.push(SaturnManifestIssue::TrackTopologyMismatch {
                detail: format!("track {} topology changed", left.number),
            });
        }
        if left.index_00_frame != right.index_00_frame
            || left.index_01_frame != right.index_01_frame
        {
            issues.push(SaturnManifestIssue::UnexpectedIndexChange { track: left.number });
        }
        if left.in_file_pregap_frames != right.in_file_pregap_frames
            || left.declared_pregap_frames != right.declared_pregap_frames
        {
            issues.push(SaturnManifestIssue::UnexpectedPregapChange { track: left.number });
        }
        if left.audio_sha256 != right.audio_sha256 {
            issues.push(SaturnManifestIssue::AudioHashMismatch { track: left.number });
        }
    }
    if expected.system_id != current.system_id {
        issues.push(SaturnManifestIssue::SystemIdChanged);
    }
    let status = if issues.is_empty() {
        current.status
    } else if issues.iter().any(|issue| {
        matches!(
            issue,
            SaturnManifestIssue::ComponentHashMismatch { .. }
                | SaturnManifestIssue::UnexpectedIndexChange { .. }
                | SaturnManifestIssue::UnexpectedPregapChange { .. }
                | SaturnManifestIssue::AudioHashMismatch { .. }
                | SaturnManifestIssue::SystemIdChanged
        )
    }) {
        SaturnManifestStatus::Invalid
    } else {
        SaturnManifestStatus::Unsafe
    };
    SaturnManifestVerification {
        status,
        issues,
        current: Some(current),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "emuwiz-saturn-manifest-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(label)
    }
    fn system_id() -> Vec<u8> {
        let mut bytes = vec![b' '; 2048];
        bytes[..16].copy_from_slice(b"SEGA SEGASATURN ");
        bytes[0x20..0x2a].copy_from_slice(b"T-1234G   ");
        bytes[0x2a..0x30].copy_from_slice(b"V1.000");
        bytes[0x30..0x38].copy_from_slice(b"19960101");
        bytes
    }
    fn write_fixture(name: &str, cue: &str, data: Vec<u8>) -> PathBuf {
        let cue_path = temp_dir(name);
        std::fs::write(&cue_path, cue).unwrap();
        std::fs::write(cue_path.with_file_name("disc.bin"), data).unwrap();
        cue_path
    }

    #[test]
    fn single_data_track_manifest_reads_system_id_from_logical_data_start() {
        let cue = write_fixture(
            "single.cue",
            "FILE \"disc.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
            {
                let mut d = vec![0_u8; 2048 * 2];
                d[..2048].copy_from_slice(&system_id());
                d
            },
        );
        let manifest = inspect_saturn_disc(&cue).unwrap();
        assert_eq!(manifest.status, SaturnManifestStatus::Complete);
        assert_eq!(manifest.tracks[0].sector_count, 2);
        assert_eq!(
            manifest.system_id.as_ref().unwrap().fact.product_number,
            "T-1234G"
        );
    }

    #[test]
    fn audio_hash_and_track_order_are_manifested() {
        let cue = write_fixture(
            "audio.cue",
            "FILE \"disc.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\nFILE \"audio.bin\" BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n",
            {
                let mut d = vec![0_u8; 2048 * 2];
                d[..2048].copy_from_slice(&system_id());
                d
            },
        );
        std::fs::write(cue.with_file_name("audio.bin"), vec![0x55_u8; 2352]).unwrap();
        let manifest = inspect_saturn_disc(&cue).unwrap();
        assert_eq!(manifest.tracks.len(), 2);
        assert!(manifest.tracks[1].audio_sha256.is_some());
    }

    #[test]
    fn lone_bin_is_incomplete_and_never_guessed_as_a_disc() {
        let bin = temp_dir("lone.bin");
        std::fs::write(&bin, [1, 2, 3]).unwrap();
        let manifest = inspect_saturn_disc(&bin).unwrap();
        assert_eq!(manifest.status, SaturnManifestStatus::Incomplete);
        assert!(manifest.tracks.is_empty());
    }

    #[test]
    fn verifier_detects_component_and_index_changes_without_writing() {
        let cue = write_fixture(
            "verify.cue",
            "FILE \"disc.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
            {
                let mut d = vec![0_u8; 2048 * 2];
                d[..2048].copy_from_slice(&system_id());
                d
            },
        );
        let before = std::fs::read(cue.with_file_name("disc.bin")).unwrap();
        let manifest = inspect_saturn_disc(&cue).unwrap();
        std::fs::write(cue.with_file_name("disc.bin"), [0_u8; 4096]).unwrap();
        let result = verify_saturn_manifest(&manifest);
        assert!(
            result
                .issues
                .iter()
                .any(|issue| matches!(issue, SaturnManifestIssue::ComponentHashMismatch { .. }))
        );
        std::fs::write(cue.with_file_name("disc.bin"), before).unwrap();
    }

    #[test]
    fn missing_component_and_traversal_are_typed_refusals() {
        let missing = temp_dir("missing.cue");
        std::fs::write(
            &missing,
            "FILE \"missing.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
        )
        .unwrap();
        assert!(matches!(
            inspect_saturn_disc(&missing),
            Err(SaturnManifestError::Cue(CueError::MissingDataFile(_)))
        ));
        let traversal = temp_dir("traversal.cue");
        std::fs::write(
            &traversal,
            "FILE \"../outside.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
        )
        .unwrap();
        assert!(matches!(
            inspect_saturn_disc(&traversal),
            Err(SaturnManifestError::Cue(CueError::UnsafeReference))
        ));
    }
}
