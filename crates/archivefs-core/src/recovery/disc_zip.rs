//! Read-only inspection of a ZIP that may hold one CD image set: a cue sheet
//! and every track file it names.
//!
//! Only the ZIP's central directory and the (small) cue sheet are read. The
//! sheet's bytes are decoded through the hardened member reader, which checks
//! them against the CRC-32 the ZIP records. Nothing is written.

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::cue_sheet::{
    CueFileKind, CueRefusal, CueSheet, CueTrackMode, MAX_CUE_SHEET_BYTES, parse_cue_sheet,
};
use crate::dat::archive::limits::ArchiveLimits;
use crate::dat::archive::member_path::check_member_name;
use crate::dat::archive::zip_preflight::{ZipPreflightEntry, ZipPreflightError, preflight_zip};
use crate::safe_read::{TrustedRoots, open_bounded_read};

/// Bounds for one CD image set. A CD holds at most about 900 MiB of raw
/// sectors; WAV tracks add a small header each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscSetLimits {
    /// Members of the ZIP in total (a sheet, up to 99 tracks, a few notes).
    pub max_members: usize,
    /// One extracted file.
    pub max_member_bytes: u64,
    /// Every file of the set together.
    pub max_total_bytes: u64,
    /// Logical bytes per stored byte before a member is treated as a bomb.
    pub max_compression_ratio: u64,
}

impl Default for DiscSetLimits {
    fn default() -> Self {
        Self {
            max_members: 128,
            max_member_bytes: 1024 * 1024 * 1024,
            max_total_bytes: 2 * 1024 * 1024 * 1024,
            max_compression_ratio: crate::dat::archive::limits::MAX_ARCHIVE_COMPRESSION_RATIO,
        }
    }
}

impl DiscSetLimits {
    pub(crate) fn archive_limits(&self) -> ArchiveLimits {
        ArchiveLimits {
            max_members: self.max_members,
            max_member_logical_bytes: self.max_member_bytes,
            max_archive_logical_bytes: self.max_total_bytes,
            max_compression_ratio: self.max_compression_ratio,
            ..ArchiveLimits::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscMemberRole {
    CueSheet,
    DataTrack,
    AudioTrack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscSetMember {
    pub name: String,
    pub role: DiscMemberRole,
    pub size_bytes: u64,
    pub stored_bytes: u64,
    /// Eight lower-case hex digits, from the ZIP's central directory.
    pub crc32: String,
}

/// The filesystem identity of the ZIP at inspection time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceIdentity {
    pub device: u64,
    pub inode: u64,
    pub size_bytes: u64,
    pub modified_ns: i128,
    pub changed_ns: i128,
}

impl SourceIdentity {
    pub(crate) fn of(metadata: &std::fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size_bytes: metadata.len(),
            modified_ns: i128::from(metadata.mtime()) * 1_000_000_000
                + i128::from(metadata.mtime_nsec()),
            changed_ns: i128::from(metadata.ctime()) * 1_000_000_000
                + i128::from(metadata.ctime_nsec()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscSetInspection {
    pub source: SourceIdentity,
    /// The sheet first, then each track file in sheet order.
    pub members: Vec<DiscSetMember>,
    pub sheet: CueSheet,
    pub total_bytes: u64,
    /// SHA-256 over every set member's name, size and CRC-32. Two ZIPs with
    /// the same fingerprint hold the same disc set.
    pub fingerprint: String,
    /// Members the sheet does not name (notes, artwork). Never extracted.
    pub ignored_members: Vec<String>,
    /// Things that look wrong but do not make the set unusable.
    pub irregularities: Vec<String>,
}

impl DiscSetInspection {
    pub fn cue_name(&self) -> &str {
        &self.members[0].name
    }

    pub fn audio_track_files(&self) -> usize {
        self.members
            .iter()
            .filter(|member| member.role == DiscMemberRole::AudioTrack)
            .count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscSetRefusal {
    /// The path is missing, a symlink, not a regular file, or unreadable.
    Source(String),
    /// The ZIP is corrupt or uses a feature the reader refuses.
    Archive(String),
    /// A safety bound was exceeded (named).
    Limit(&'static str),
    /// No cue sheet inside: not a disc set this engine understands.
    NoCueSheet,
    MultipleCueSheets(Vec<String>),
    /// Files are stored in a folder inside the ZIP.
    NestedFolders,
    /// A member name is unsafe, duplicated, or not valid text.
    UnsafeMember(String),
    Cue(CueRefusal),
    /// The sheet names files the ZIP does not hold.
    MissingTracks(Vec<String>),
    /// The sheet's spelling differs from the member's only by letter case.
    CaseMismatch {
        sheet: String,
        member: String,
    },
    Cancelled,
}

impl std::fmt::Display for DiscSetRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Source(detail) => write!(f, "the file cannot be read safely: {detail}"),
            Self::Archive(detail) => write!(f, "the ZIP cannot be read safely: {detail}"),
            Self::Limit(which) => write!(f, "a safety limit was exceeded: {which}"),
            Self::NoCueSheet => f.write_str("the ZIP holds no cue sheet"),
            Self::MultipleCueSheets(names) => {
                write!(
                    f,
                    "the ZIP holds more than one cue sheet: {}",
                    names.join(", ")
                )
            }
            Self::NestedFolders => f.write_str("the disc files are inside a folder in the ZIP"),
            Self::UnsafeMember(detail) => {
                write!(f, "a file name inside the ZIP is unsafe: {detail}")
            }
            Self::Cue(refusal) => write!(f, "{refusal}"),
            Self::MissingTracks(names) => write!(
                f,
                "the cue sheet names files the ZIP does not hold: {}",
                names.join(", ")
            ),
            Self::CaseMismatch { sheet, member } => write!(
                f,
                "the cue sheet names {sheet} but the ZIP holds {member}; the spelling differs"
            ),
            Self::Cancelled => f.write_str("cancelled"),
        }
    }
}

pub(crate) fn crc_hex(crc: u32) -> String {
    format!("{crc:08x}")
}

fn is_regular_file_entry(entry: &ZipPreflightEntry) -> bool {
    // Only a Unix-made entry carries a file type in its attributes.
    if (entry.version_made_by >> 8) != 3 {
        return true;
    }
    matches!((entry.external_attributes >> 16) & 0xf000, 0 | 0x8000)
}

fn ratio_exceeded(logical: u64, stored: u64, maximum: u64) -> bool {
    logical > 0
        && (stored == 0
            || stored
                .checked_mul(maximum)
                .is_none_or(|most| logical > most))
}

/// Decodes one small member (stored or deflated) from the byte range the
/// preflight validated, bounded to `limit` bytes, and checks its size and
/// CRC-32 against the central directory.
fn read_small_member(
    file: &mut std::fs::File,
    entry: &ZipPreflightEntry,
    limit: usize,
) -> Result<Vec<u8>, DiscSetRefusal> {
    use std::io::{Read, Seek, SeekFrom};
    let unreadable = |detail: String| {
        DiscSetRefusal::Archive(format!("the cue sheet could not be read: {detail}"))
    };
    if entry.logical_size > limit as u64 || entry.compressed_size > 4 * limit as u64 + 1024 {
        return Err(DiscSetRefusal::Cue(CueRefusal::TooLarge));
    }
    file.seek(SeekFrom::Start(entry.data_start))
        .map_err(|error| unreadable(error.to_string()))?;
    let packed = file.take(entry.compressed_size);
    let mut bytes = Vec::with_capacity(entry.logical_size as usize);
    let bound = limit as u64 + 1;
    match entry.method {
        0 => packed.take(bound).read_to_end(&mut bytes),
        8 => flate2::read::DeflateDecoder::new(packed)
            .take(bound)
            .read_to_end(&mut bytes),
        other => return Err(unreadable(format!("compression method {other}"))),
    }
    .map_err(|error| unreadable(error.to_string()))?;
    let mut crc = crate::identity_source::hashing::Crc32::new();
    crc.update(&bytes);
    if bytes.len() as u64 != entry.logical_size || crc.finish() != entry.crc32 {
        return Err(unreadable(
            "its contents do not match the size and checksum the ZIP records".into(),
        ));
    }
    Ok(bytes)
}

/// Inspects `path` as a ZIP holding one CD image set. Read-only and bounded:
/// the central directory (at most 16 MiB) and the cue sheet (at most 64 KiB).
pub fn inspect_disc_zip(
    path: &Path,
    trusted: &TrustedRoots,
    limits: &DiscSetLimits,
    cancel: &AtomicBool,
) -> Result<DiscSetInspection, DiscSetRefusal> {
    let safe = open_bounded_read(path, trusted)
        .map_err(|refusal| DiscSetRefusal::Source(format!("{refusal:?}")))?;
    let length = safe.len();
    let mut file = safe.into_file();
    let metadata = file
        .metadata()
        .map_err(|error| DiscSetRefusal::Source(error.to_string()))?;
    if !metadata.is_file() {
        return Err(DiscSetRefusal::Source("not a regular file".into()));
    }
    let source = SourceIdentity::of(&metadata);
    let archive_limits = limits.archive_limits();
    let info =
        preflight_zip(&mut file, length, &archive_limits, cancel).map_err(|error| match error {
            ZipPreflightError::Cancelled => DiscSetRefusal::Cancelled,
            ZipPreflightError::Refused(reason) => DiscSetRefusal::Limit(reason),
            ZipPreflightError::Corrupt(detail) => DiscSetRefusal::Archive(detail),
        })?;
    if info.entries.len() > limits.max_members {
        return Err(DiscSetRefusal::Limit("number of files in the ZIP"));
    }

    let mut names: Vec<(&ZipPreflightEntry, &str)> = Vec::with_capacity(info.entries.len());
    for entry in &info.entries {
        let name = std::str::from_utf8(&entry.name_raw)
            .map_err(|_| DiscSetRefusal::UnsafeMember("a name is not valid UTF-8".into()))?;
        if entry.is_directory || name.contains('/') {
            return Err(DiscSetRefusal::NestedFolders);
        }
        check_member_name(name)
            .map_err(|refusal| DiscSetRefusal::UnsafeMember(format!("{name:?}: {refusal:?}")))?;
        if !super::cue_sheet::is_plain_file_name(name) {
            return Err(DiscSetRefusal::UnsafeMember(format!("{name:?}")));
        }
        if !is_regular_file_entry(entry) {
            return Err(DiscSetRefusal::UnsafeMember(format!(
                "{name:?} is a link or a special file"
            )));
        }
        if entry.flags & ((1 << 0) | (1 << 6) | (1 << 13)) != 0 {
            return Err(DiscSetRefusal::Archive(format!("{name:?} is encrypted")));
        }
        if entry.flags & ((1 << 4) | (1 << 5)) != 0 || !matches!(entry.method, 0 | 8) {
            return Err(DiscSetRefusal::Archive(format!(
                "{name:?} uses compression method {}, which is not supported",
                entry.method
            )));
        }
        if names
            .iter()
            .any(|(_, seen)| seen.eq_ignore_ascii_case(name))
        {
            return Err(DiscSetRefusal::UnsafeMember(format!(
                "{name:?} appears more than once"
            )));
        }
        names.push((entry, name));
    }

    let cue_sheets: Vec<&(&ZipPreflightEntry, &str)> = names
        .iter()
        .filter(|(_, name)| {
            Path::new(name)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("cue"))
        })
        .collect();
    let (cue_entry, cue_name) = match cue_sheets.as_slice() {
        [] => return Err(DiscSetRefusal::NoCueSheet),
        [one] => **one,
        many => {
            return Err(DiscSetRefusal::MultipleCueSheets(
                many.iter().map(|(_, name)| (*name).to_string()).collect(),
            ));
        }
    };
    if cue_entry.logical_size > MAX_CUE_SHEET_BYTES as u64 {
        return Err(DiscSetRefusal::Cue(CueRefusal::TooLarge));
    }

    // The sheet is decoded here from the range the validated index gives,
    // so inspection reads the index and these few kilobytes and nothing else.
    let cue_bytes = read_small_member(&mut file, cue_entry, MAX_CUE_SHEET_BYTES)?;
    let after = file
        .metadata()
        .map_err(|error| DiscSetRefusal::Source(error.to_string()))?;
    if SourceIdentity::of(&after) != source {
        return Err(DiscSetRefusal::Source(
            "the ZIP changed while it was being read".into(),
        ));
    }
    drop(file);
    let sheet = parse_cue_sheet(&cue_bytes).map_err(DiscSetRefusal::Cue)?;

    let mut members = vec![DiscSetMember {
        name: cue_name.to_string(),
        role: DiscMemberRole::CueSheet,
        size_bytes: cue_entry.logical_size,
        stored_bytes: cue_entry.compressed_size,
        crc32: crc_hex(cue_entry.crc32),
    }];
    let mut missing = Vec::new();
    let mut irregularities = Vec::new();
    for cue_file in &sheet.files {
        let Some((entry, name)) = names.iter().find(|(_, name)| *name == cue_file.name) else {
            if let Some((_, other)) = names
                .iter()
                .find(|(_, name)| name.eq_ignore_ascii_case(&cue_file.name))
            {
                return Err(DiscSetRefusal::CaseMismatch {
                    sheet: cue_file.name.clone(),
                    member: (*other).to_string(),
                });
            }
            missing.push(cue_file.name.clone());
            continue;
        };
        if *name == cue_name {
            return Err(DiscSetRefusal::Cue(CueRefusal::UnsafeFileName(0)));
        }
        if entry.logical_size == 0 {
            irregularities.push(format!("{name} is empty"));
        }
        let all_audio = cue_file
            .tracks
            .iter()
            .all(|track| track.mode == CueTrackMode::Audio);
        if cue_file.kind == CueFileKind::Binary {
            let sector = if cue_file
                .tracks
                .iter()
                .all(|track| track.mode == CueTrackMode::Mode1Cooked)
            {
                Some(2048)
            } else if cue_file
                .tracks
                .iter()
                .all(|track| track.mode != CueTrackMode::Mode1Cooked)
            {
                Some(2352)
            } else {
                None
            };
            if let Some(sector) = sector
                && entry.logical_size % sector != 0
            {
                irregularities.push(format!(
                    "{name} is not a whole number of {sector}-byte sectors"
                ));
            }
        }
        members.push(DiscSetMember {
            name: (*name).to_string(),
            role: if all_audio {
                DiscMemberRole::AudioTrack
            } else {
                DiscMemberRole::DataTrack
            },
            size_bytes: entry.logical_size,
            stored_bytes: entry.compressed_size,
            crc32: crc_hex(entry.crc32),
        });
    }
    if !missing.is_empty() {
        return Err(DiscSetRefusal::MissingTracks(missing));
    }

    let mut total_bytes: u64 = 0;
    for member in &members {
        if member.size_bytes > limits.max_member_bytes {
            return Err(DiscSetRefusal::Limit("size of one file in the set"));
        }
        if ratio_exceeded(
            member.size_bytes,
            member.stored_bytes,
            limits.max_compression_ratio,
        ) {
            return Err(DiscSetRefusal::Limit("compression ratio"));
        }
        total_bytes = total_bytes
            .checked_add(member.size_bytes)
            .ok_or(DiscSetRefusal::Limit("size of the whole set"))?;
    }
    if total_bytes > limits.max_total_bytes {
        return Err(DiscSetRefusal::Limit("size of the whole set"));
    }

    let ignored_members = names
        .iter()
        .filter(|(_, name)| members.iter().all(|member| member.name != *name))
        .map(|(_, name)| (*name).to_string())
        .collect();
    let fingerprint = fingerprint_of(&members);
    Ok(DiscSetInspection {
        source,
        members,
        sheet,
        total_bytes,
        fingerprint,
        ignored_members,
        irregularities,
    })
}

/// Order-independent: the same set of (name, size, CRC-32) always gives the
/// same value, whatever order the ZIP stores its members in.
pub(crate) fn fingerprint_of(members: &[DiscSetMember]) -> String {
    let mut rows: Vec<String> = members
        .iter()
        .map(|member| format!("{}\0{}\0{}\n", member.name, member.size_bytes, member.crc32))
        .collect();
    rows.sort();
    let mut hasher = Sha256::new();
    hasher.update(b"emuwiz-disc-set-v1\n");
    for row in rows {
        hasher.update(row.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
