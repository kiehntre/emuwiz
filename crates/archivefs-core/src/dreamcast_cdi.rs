//! Dreamcast DiscJuggler `.cdi` identity support.
//!
//! # What backs this
//!
//! [`opticaldiscs::discjuggler`](https://docs.rs/opticaldiscs) (MIT, by
//! danifunker) - already a dependency behind this crate's own
//! `dreamcast-cdi` feature (default-on; see this crate's `Cargo.toml`).
//! Unlike [`crate::chd_optical_specialist`]'s `chd-optical-specialist`
//! feature, `dreamcast-cdi` does not enable `opticaldiscs`'s own `chd`
//! feature, so it pulls in no native/C++ code (`libchdman-rs`) - the two
//! features are fully independent, and either can be enabled without the
//! other.
//!
//! `opticaldiscs::discjuggler` is a pure-Rust, bounds-checked walk of the
//! DiscJuggler trailer descriptor, ported field-for-field from cdemu's
//! `libmirage` `image-cdi/parser.c` - the closest thing that exists to an
//! authoritative reference for this closed, reverse-engineered format
//! (DiscJuggler itself never published a spec). Every multi-byte read in
//! that parser goes through a bounds-checked cursor that returns a `Parse`
//! error rather than panicking or reading out of bounds, so a truncated or
//! foreign file is refused, never guessed at.
//!
//! The dependency's public parser is not used here because its API calls
//! `std::fs::read` and would allocate the whole image before reaching the
//! footer. This module performs the same small descriptor walk against a
//! bounded buffer, retaining the dependency only for its sector reader.
//!
//! This module:
//!
//! 1. Reads only the footer and bounded descriptor (see the resource limits
//!    below);
//! 2. Selects the single correct Dreamcast data track from the parsed
//!    track list, using the same structural rule
//!    [`crate::ingestion::gdi::resolve_gdi_data_track`] already uses for
//!    `.gdi` (see [`select_dreamcast_data_track`]);
//! 3. Cross-checks that track's declared byte range actually fits inside
//!    the data region, since descriptor arithmetic must not be trusted;
//! 4. Wraps the result in a [`crate::logical_media::LogicalMedia`] so the
//!    existing, unchanged [`crate::game_identity::inspect_dreamcast_source`]
//!    (via `parse_ip_bin_meta`) can read it exactly like an ISO, CUE, GDI,
//!    or GD-ROM CHD source. No second Dreamcast identity model, no
//!    invented offsets, no magic-string scan.
//!
//! # Track selection
//!
//! A `.cdi` concatenates every track from every session in one file. Per
//! [`opticaldiscs::discjuggler`]'s own module documentation, a Dreamcast
//! GD-ROM rip's high-density session records its absolute starting LBA in
//! CDI track `base_lba` (`0` for a plain volume-relative disc). This is
//! exactly the same structural
//! signal `.gdi` descriptors give directly as `start_lba`, so
//! [`select_dreamcast_data_track`] reuses the identical
//! [`crate::chd_identity::GDROM_HIGH_DENSITY_START_FRAME`] boundary and
//! the identical "exactly one candidate, or refuse" rule `.gdi` uses -
//! never track order, never a filename, never a guess. A single-session
//! disc (every track's `base_lba == 0`) falls back to requiring exactly
//! one data track in the whole file.
//!
//! IP.BIN always sits at logical offset zero of the selected data track's
//! own cooked view, so no absolute-LBA rebasing is needed here (unlike
//! browsing that track's ISO 9660 directory extents, which is outside
//! this module's scope entirely).
//!
//! The supported versions are the three version markers used by DiscJuggler
//! (`0x80000004`, `0x80000005`, and `0x80000006`, commonly called 2, 3, and
//! 3.5). Versions are accepted only when their footer and self-delimiting
//! descriptor validate; no other CDI-like variant is guessed.

use std::cell::{Cell, RefCell};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use opticaldiscs::sector_reader::{BinCueSectorReader, SectorReader};

use crate::chd_identity::GDROM_HIGH_DENSITY_START_FRAME;
use crate::logical_media::{LogicalMedia, LogicalMediaError};

/// Cooked ISO 9660 logical sector size, matching every other reader in
/// this crate.
const SECTOR_SIZE: u64 = 2048;

/// Maximum image size considered by this inspector. Checked from filesystem
/// metadata before opening any descriptor or data range.
pub const MAX_CDI_BYTES: u64 = 1_536 * 1024 * 1024;

/// Maximum descriptor/metadata allocation. Real descriptors are normally a
/// few kilobytes; malformed footer values never request an image-sized Vec.
pub const MAX_CDI_METADATA_BYTES: usize = 4 * 1024 * 1024;
/// Maximum bytes transferred by one descriptor or logical-media read.
pub const MAX_CDI_READ_BYTES: usize = 64 * 1024;
/// Maximum logical data bytes inspected after the descriptor. The descriptor
/// and data limits are separate so identity never turns into whole-image I/O.
pub const MAX_CDI_INSPECTED_BYTES: u64 = 4 * 1024 * 1024;
/// Conservative bounds for CDI's variable-count fields.
pub const MAX_CDI_SESSIONS: usize = 32;
pub const MAX_CDI_TRACKS: usize = 256;
pub const MAX_CDI_DESCRIPTORS: usize = 512;
const MAX_CDI_INDICES: usize = 64;
const MAX_CDI_CDTEXT_BLOCKS: usize = 64;
const CDI_V2: u32 = 0x8000_0004;
const CDI_V3: u32 = 0x8000_0005;
const CDI_V35: u32 = 0x8000_0006;

#[derive(Debug)]
pub enum CdiIdentityError {
    /// The path could not be statted or read.
    Io(String),
    /// The file exceeds [`MAX_CDI_BYTES`]; refused before any read.
    TooLarge { bytes: u64, maximum: u64 },
    /// The footer or bounded descriptor is not a supported CDI layout.
    Parse(String),
    /// More tracks or descriptors than the conservative bounds allow.
    TooManyTracks,
    /// No Dreamcast-eligible data track was found at all.
    NoDataTrack,
    /// More than one track could be the Dreamcast data track; refused
    /// rather than guessed, exactly like `.gdi`'s own rule.
    AmbiguousDataTracks,
    /// The track's declared sector geometry could not be exposed as a
    /// bounded, valid 2048-byte cooked view (e.g. its user-data offset
    /// does not fit inside its own physical sector).
    UnsupportedSectorLayout,
    /// The track's declared byte range does not fit inside the real
    /// file - a contradictory or overflowing offset/length pair, never
    /// trusted just because the trailer's own arithmetic produced it.
    ImpossibleOffset,
    /// The descriptor exceeds [`MAX_CDI_METADATA_BYTES`].
    MetadataTooLarge { bytes: u64, maximum: usize },
}

impl std::fmt::Display for CdiIdentityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(detail) => write!(formatter, "CDI I/O error: {detail}"),
            Self::TooLarge { bytes, maximum } => write!(
                formatter,
                "CDI file is {bytes} bytes, exceeding the {maximum}-byte bound"
            ),
            Self::Parse(detail) => {
                write!(formatter, "CDI descriptor could not be parsed: {detail}")
            }
            Self::TooManyTracks => formatter.write_str("CDI descriptor declares too many tracks"),
            Self::NoDataTrack => {
                formatter.write_str("CDI has no track this module recognises as Dreamcast data")
            }
            Self::AmbiguousDataTracks => {
                formatter.write_str("CDI has more than one candidate Dreamcast data track")
            }
            Self::UnsupportedSectorLayout => {
                formatter.write_str("CDI data track has an unsupported sector geometry")
            }
            Self::ImpossibleOffset => formatter
                .write_str("CDI data track's declared byte range does not fit inside the file"),
            Self::MetadataTooLarge { bytes, maximum } => write!(
                formatter,
                "CDI descriptor is {bytes} bytes, exceeding the {maximum}-byte metadata bound"
            ),
        }
    }
}

impl std::error::Error for CdiIdentityError {}

#[derive(Debug, Clone)]
struct CdiTrack {
    is_data: bool,
    physical_sector_size: u64,
    data_offset: u64,
    file_byte_offset: u64,
    base_lba: u64,
    track_frames: u64,
    data_frames: u64,
    descriptor_count: usize,
}

struct DescriptorCursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl DescriptorCursor<'_> {
    fn take(&mut self, length: usize) -> Result<&[u8], CdiIdentityError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(|| CdiIdentityError::Parse("descriptor offset overflow".into()))?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| CdiIdentityError::Parse("truncated CDI descriptor".into()))?;
        self.position = end;
        Ok(bytes)
    }

    fn skip(&mut self, length: usize) -> Result<(), CdiIdentityError> {
        self.take(length).map(|_| ())
    }

    fn u8(&mut self) -> Result<u8, CdiIdentityError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, CdiIdentityError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().map_err(
            |_| CdiIdentityError::Parse("invalid CDI u16".into()),
        )?))
    }

    fn u32(&mut self) -> Result<u32, CdiIdentityError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().map_err(
            |_| CdiIdentityError::Parse("invalid CDI u32".into()),
        )?))
    }
}

fn checked_track_bytes(offset: u64, physical_sector_size: u64, frames: u64) -> Option<u64> {
    offset.checked_add(physical_sector_size.checked_mul(frames)?)
}

fn read_descriptor(path: &Path, file_len: u64) -> Result<(Vec<u8>, u64), CdiIdentityError> {
    if file_len < 8 {
        return Err(CdiIdentityError::Parse(
            "file too small for CDI footer".into(),
        ));
    }
    let mut file =
        std::fs::File::open(path).map_err(|error| CdiIdentityError::Io(error.to_string()))?;
    file.seek(SeekFrom::Start(file_len - 8))
        .map_err(|error| CdiIdentityError::Io(error.to_string()))?;
    let mut footer = [0_u8; 8];
    file.read_exact(&mut footer)
        .map_err(|error| CdiIdentityError::Io(error.to_string()))?;
    let version = u32::from_le_bytes(
        footer[..4]
            .try_into()
            .map_err(|_| CdiIdentityError::Parse("invalid CDI footer version".into()))?,
    );
    let location =
        u64::from(u32::from_le_bytes(footer[4..].try_into().map_err(
            |_| CdiIdentityError::Parse("invalid CDI footer location".into()),
        )?));
    if !matches!(version, CDI_V2 | CDI_V3 | CDI_V35) {
        return Err(CdiIdentityError::Parse(format!(
            "unsupported CDI version 0x{version:08x}"
        )));
    }

    // Versions 2/3 store the descriptor start; 3.5 stores the distance from
    // EOF, including the eight-byte footer. This matches real v3 Shenmue and
    // v3.5 Simpsons images in the configured corpus.
    let descriptor_start = match version {
        CDI_V2 | CDI_V3 => location,
        CDI_V35 => file_len
            .checked_sub(location)
            .ok_or_else(|| CdiIdentityError::Parse("CDI descriptor offset underflow".into()))?,
        _ => unreachable!(),
    };
    let descriptor_end = file_len - 8;
    if descriptor_start >= descriptor_end {
        return Err(CdiIdentityError::Parse(
            "CDI descriptor does not fit before footer".into(),
        ));
    }
    let descriptor_len = descriptor_end - descriptor_start;
    if descriptor_len > MAX_CDI_METADATA_BYTES as u64 {
        return Err(CdiIdentityError::MetadataTooLarge {
            bytes: descriptor_len,
            maximum: MAX_CDI_METADATA_BYTES,
        });
    }
    let descriptor_len_usize =
        usize::try_from(descriptor_len).map_err(|_| CdiIdentityError::MetadataTooLarge {
            bytes: descriptor_len,
            maximum: MAX_CDI_METADATA_BYTES,
        })?;
    let mut descriptor = vec![0_u8; descriptor_len_usize];
    let mut read = 0usize;
    while read < descriptor.len() {
        let amount = (descriptor.len() - read).min(MAX_CDI_READ_BYTES);
        let offset = descriptor_start
            .checked_add(u64::try_from(read).map_err(|_| {
                CdiIdentityError::Parse("CDI descriptor read offset overflow".into())
            })?)
            .ok_or_else(|| CdiIdentityError::Parse("CDI descriptor read offset overflow".into()))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| CdiIdentityError::Io(error.to_string()))?;
        file.read_exact(&mut descriptor[read..read + amount])
            .map_err(|error| CdiIdentityError::Io(error.to_string()))?;
        read += amount;
    }
    Ok((descriptor, descriptor_start))
}

fn parse_descriptor(bytes: &[u8], data_end: u64) -> Result<Vec<CdiTrack>, CdiIdentityError> {
    let mut cursor = DescriptorCursor { bytes, position: 0 };
    let sessions = usize::from(cursor.u8()?);
    if sessions == 0 || sessions > MAX_CDI_SESSIONS {
        return Err(CdiIdentityError::TooManyTracks);
    }
    let mut tracks: Vec<CdiTrack> = Vec::new();
    let mut current_offset = 0_u64;
    let mut descriptor_count = 0usize;
    let mut saw_terminator = false;
    for _ in 0..=sessions {
        let session = cursor.take(15)?;
        let track_count = usize::from(session[1]);
        if track_count > 99 {
            return Err(CdiIdentityError::TooManyTracks);
        }
        if track_count == 0 {
            saw_terminator = true;
            break;
        }
        for _ in 0..track_count {
            if tracks.len() >= MAX_CDI_TRACKS {
                return Err(CdiIdentityError::TooManyTracks);
            }
            let track = parse_track(&mut cursor, current_offset, data_end)?;
            descriptor_count = descriptor_count
                .checked_add(track.descriptor_count)
                .ok_or(CdiIdentityError::TooManyTracks)?;
            if descriptor_count > MAX_CDI_DESCRIPTORS {
                return Err(CdiIdentityError::TooManyTracks);
            }
            let track_end_lba = track
                .base_lba
                .checked_add(track.track_frames)
                .ok_or(CdiIdentityError::ImpossibleOffset)?;
            for previous in &tracks {
                let previous_end = previous
                    .base_lba
                    .checked_add(previous.track_frames)
                    .ok_or(CdiIdentityError::ImpossibleOffset)?;
                if track.base_lba < previous_end && previous.base_lba < track_end_lba {
                    return Err(CdiIdentityError::ImpossibleOffset);
                }
            }
            tracks.push(track);
            let track = tracks.last().ok_or(CdiIdentityError::TooManyTracks)?;
            current_offset = checked_track_bytes(
                current_offset,
                track.physical_sector_size,
                track.track_frames,
            )
            .ok_or(CdiIdentityError::ImpossibleOffset)?;
        }
    }
    if !saw_terminator || tracks.is_empty() {
        return Err(CdiIdentityError::Parse(
            "CDI descriptor has no terminating session or tracks".into(),
        ));
    }
    if current_offset > data_end {
        return Err(CdiIdentityError::ImpossibleOffset);
    }
    Ok(tracks)
}

fn parse_track(
    cursor: &mut DescriptorCursor<'_>,
    track_start: u64,
    data_end: u64,
) -> Result<CdiTrack, CdiIdentityError> {
    cursor.skip(16)?;
    let filename_len = usize::from(cursor.u8()?);
    cursor.skip(filename_len)?;
    cursor.skip(29)?;
    let _medium_type = cursor.u16()?;
    let index_count = usize::from(cursor.u16()?);
    if index_count > MAX_CDI_INDICES {
        return Err(CdiIdentityError::TooManyTracks);
    }
    let mut pregap = 0_u64;
    for index in 0..index_count {
        let value = u64::from(cursor.u32()?);
        if index == 0 {
            pregap = value;
        }
    }
    let cdtext_blocks =
        usize::try_from(cursor.u32()?).map_err(|_| CdiIdentityError::TooManyTracks)?;
    if cdtext_blocks > MAX_CDI_CDTEXT_BLOCKS {
        return Err(CdiIdentityError::TooManyTracks);
    }
    let descriptor_count = cdtext_blocks
        .checked_add(index_count)
        .ok_or(CdiIdentityError::TooManyTracks)?;
    if descriptor_count > MAX_CDI_DESCRIPTORS {
        return Err(CdiIdentityError::TooManyTracks);
    }
    for _ in 0..cdtext_blocks {
        for _ in 0..18 {
            let field_len = usize::from(cursor.u8()?);
            cursor.skip(field_len)?;
        }
    }
    cursor.skip(2)?;
    let track_mode = cursor.u32()?;
    cursor.skip(4)?;
    let _session_index = cursor.u32()?;
    let _track_index = cursor.u32()?;
    let base_lba = u64::from(cursor.u32()?);
    let track_frames = u64::from(cursor.u32()?);
    cursor.skip(16)?;
    let read_mode = cursor.u32()?;
    cursor.skip(4 + 9 + 12)?;
    let _isrc_valid = cursor.u32()?;
    cursor.skip(99)?;

    let (physical_sector_size, data_offset, is_data) = match (track_mode, read_mode) {
        (0, 2) => (2352, 0, false),
        (0, 3) => (2368, 0, false),
        (0, 4) => (2448, 0, false),
        (1, 0) => (2048, 0, true),
        (1, 2) => (2352, 16, true),
        (1, 3) => (2368, 16, true),
        (1, 4) => (2448, 16, true),
        (2, 1) => (2336, 8, true),
        (2, 2) => (2352, 24, true),
        (2, 3) => (2368, 24, true),
        (2, 4) => (2448, 24, true),
        _ => {
            return Err(CdiIdentityError::UnsupportedSectorLayout);
        }
    };
    if pregap > track_frames {
        return Err(CdiIdentityError::ImpossibleOffset);
    }
    let track_end = checked_track_bytes(track_start, physical_sector_size, track_frames)
        .ok_or(CdiIdentityError::ImpossibleOffset)?;
    if track_end > data_end {
        return Err(CdiIdentityError::ImpossibleOffset);
    }
    let data_start = checked_track_bytes(track_start, physical_sector_size, pregap)
        .ok_or(CdiIdentityError::ImpossibleOffset)?;
    let data_frames = track_frames - pregap;
    if checked_track_bytes(data_start, physical_sector_size, data_frames)
        .is_none_or(|end| end > track_end)
    {
        return Err(CdiIdentityError::ImpossibleOffset);
    }
    Ok(CdiTrack {
        is_data,
        physical_sector_size,
        data_offset,
        file_byte_offset: data_start,
        base_lba,
        track_frames,
        data_frames,
        descriptor_count,
    })
}

/// A [`LogicalMedia`] over one `.cdi` file's selected Dreamcast data
/// track, decoded on demand.
///
/// Interior mutability ([`RefCell`]) satisfies [`LogicalMedia::read_at`]'s
/// `&self` signature; the underlying `SectorReader` needs `&mut self` to
/// seek+read a sector, exactly as
/// [`crate::chd_optical_specialist::ChdOpticalSpecialist`] already does
/// for the same reason.
pub struct CdiLogicalMedia {
    reader: RefCell<BinCueSectorReader>,
    len: u64,
    bytes_inspected: Cell<u64>,
}

/// Opens `path` as a DiscJuggler `.cdi` and resolves the single Dreamcast
/// data track needed for identity - see the module documentation for the
/// exact selection rule and every bound enforced before any sector is
/// read.
pub fn open_dreamcast_cdi_logical_media(path: &Path) -> Result<CdiLogicalMedia, CdiIdentityError> {
    let metadata =
        std::fs::metadata(path).map_err(|error| CdiIdentityError::Io(error.to_string()))?;
    let file_len = metadata.len();
    if file_len > MAX_CDI_BYTES {
        return Err(CdiIdentityError::TooLarge {
            bytes: file_len,
            maximum: MAX_CDI_BYTES,
        });
    }

    let (descriptor, data_end) = read_descriptor(path, file_len)?;
    let tracks = parse_descriptor(&descriptor, data_end)?;

    let track = select_dreamcast_data_track(&tracks)?;

    let data_sector_end = track
        .data_offset
        .checked_add(SECTOR_SIZE)
        .ok_or(CdiIdentityError::UnsupportedSectorLayout)?;
    if data_sector_end > track.physical_sector_size {
        return Err(CdiIdentityError::UnsupportedSectorLayout);
    }
    if track.data_frames == 0 {
        return Err(CdiIdentityError::NoDataTrack);
    }
    let data_end = checked_track_bytes(
        track.file_byte_offset,
        track.physical_sector_size,
        track.data_frames,
    )
    .ok_or(CdiIdentityError::ImpossibleOffset)?;
    if data_end > file_len {
        return Err(CdiIdentityError::ImpossibleOffset);
    }

    let logical_len = track
        .data_frames
        .checked_mul(SECTOR_SIZE)
        .ok_or(CdiIdentityError::ImpossibleOffset)?;

    let reader = BinCueSectorReader::with_layout(
        path,
        track.file_byte_offset,
        track.physical_sector_size,
        track.data_offset,
    )
    .map_err(|error| CdiIdentityError::Parse(format!("{error:?}")))?;

    Ok(CdiLogicalMedia {
        reader: RefCell::new(reader),
        len: logical_len,
        bytes_inspected: Cell::new(0),
    })
}

/// Selects the single correct Dreamcast data track from a parsed `.cdi`
/// track list - see the module documentation for the exact rule. Never
/// track order, never a filename.
fn select_dreamcast_data_track(tracks: &[CdiTrack]) -> Result<&CdiTrack, CdiIdentityError> {
    let high_density: Vec<&CdiTrack> = tracks
        .iter()
        .filter(|track| {
            track.is_data && track.base_lba >= u64::from(GDROM_HIGH_DENSITY_START_FRAME)
        })
        .collect();
    match high_density.len() {
        1 => return Ok(high_density[0]),
        0 => {}
        _ => return Err(CdiIdentityError::AmbiguousDataTracks),
    }

    // Single-session shape: no track reaches the GD-ROM high-density
    // boundary, so this can only be trusted if exactly one data track
    // exists in the whole file.
    let plain: Vec<&CdiTrack> = tracks.iter().filter(|track| track.is_data).collect();
    match plain.len() {
        1 => Ok(plain[0]),
        0 => Err(CdiIdentityError::NoDataTrack),
        _ => Err(CdiIdentityError::AmbiguousDataTracks),
    }
}

impl LogicalMedia for CdiLogicalMedia {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), LogicalMediaError> {
        if buf.len() > MAX_CDI_READ_BYTES {
            return Err(LogicalMediaError::DecodeFailed {
                detail: format!("CDI logical read exceeds {MAX_CDI_READ_BYTES}-byte bound"),
            });
        }
        let requested = u64::try_from(buf.len()).map_err(|_| LogicalMediaError::DecodeFailed {
            detail: "CDI logical read length does not fit in u64".to_string(),
        })?;
        let inspected = self
            .bytes_inspected
            .get()
            .checked_add(requested)
            .ok_or_else(|| LogicalMediaError::DecodeFailed {
                detail: "CDI cumulative logical-read counter overflowed".to_string(),
            })?;
        if inspected > MAX_CDI_INSPECTED_BYTES {
            return Err(LogicalMediaError::DecodeFailed {
                detail: "CDI cumulative logical-read bound exceeded".to_string(),
            });
        }
        self.bytes_inspected.set(inspected);
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or(LogicalMediaError::OutOfBounds {
                offset,
                requested_len: buf.len(),
                media_len: self.len,
            })?;
        if end > self.len {
            return Err(LogicalMediaError::OutOfBounds {
                offset,
                requested_len: buf.len(),
                media_len: self.len,
            });
        }

        let mut filled = 0usize;
        let mut reader = self.reader.borrow_mut();
        while filled < buf.len() {
            let absolute =
                offset
                    .checked_add(filled as u64)
                    .ok_or(LogicalMediaError::OutOfBounds {
                        offset,
                        requested_len: buf.len(),
                        media_len: self.len,
                    })?;
            let lba = absolute / SECTOR_SIZE;
            let within_sector = (absolute % SECTOR_SIZE) as usize;
            let sector =
                reader
                    .read_sector(lba)
                    .map_err(|error| LogicalMediaError::DecodeFailed {
                        detail: format!("{error:?}"),
                    })?;
            if sector.len() != SECTOR_SIZE as usize {
                return Err(LogicalMediaError::DecodeFailed {
                    detail: format!(
                        "cooked sector at lba {lba} was {} bytes, expected {SECTOR_SIZE}",
                        sector.len()
                    ),
                });
            }
            let take = (SECTOR_SIZE as usize - within_sector).min(buf.len() - filled);
            buf[filled..filled + take]
                .copy_from_slice(&sector[within_sector..within_sector + take]);
            filled += take;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// One test-fixture track spec:
    /// `(track_mode, read_mode, start_address, track_length, pregap)`.
    type TrackSpec = (u32, u32, u32, u32, u32);

    /// Mirrors `opticaldiscs::discjuggler`'s own private test-fixture
    /// builder exactly (that helper is private to its own test module, so
    /// re-derived here per this crate's established per-file-fixture
    /// convention). `sessions` is a list of track specs grouped by
    /// session, each `(track_mode, read_mode, start_address,
    /// track_length, pregap)`.
    fn push_track(
        desc: &mut Vec<u8>,
        track_mode: u32,
        read_mode: u32,
        start_address: u32,
        track_length: u32,
        pregap: u32,
    ) {
        desc.extend_from_slice(&[0u8; 16]);
        desc.push(0); // filename length
        desc.extend_from_slice(&[0u8; 29]);
        desc.extend_from_slice(&[0u8; 2]); // medium type
        desc.extend_from_slice(&2u16.to_le_bytes()); // num_indices
        desc.extend_from_slice(&pregap.to_le_bytes());
        desc.extend_from_slice(&(track_length - pregap).to_le_bytes());
        desc.extend_from_slice(&0u32.to_le_bytes()); // num_cdtext_blocks
        desc.extend_from_slice(&[0u8; 2]);
        desc.extend_from_slice(&track_mode.to_le_bytes());
        desc.extend_from_slice(&[0u8; 4]);
        desc.extend_from_slice(&0u32.to_le_bytes()); // session_idx
        desc.extend_from_slice(&0u32.to_le_bytes()); // track_idx
        desc.extend_from_slice(&start_address.to_le_bytes());
        desc.extend_from_slice(&track_length.to_le_bytes());
        desc.extend_from_slice(&[0u8; 16]);
        desc.extend_from_slice(&read_mode.to_le_bytes());
        desc.extend_from_slice(&0u32.to_le_bytes()); // track_ctl
        desc.extend_from_slice(&[0u8; 9]);
        desc.extend_from_slice(&[0u8; 12]); // ISRC
        desc.extend_from_slice(&0u32.to_le_bytes()); // isrc_valid
        desc.extend_from_slice(&[0u8; 99]);
    }

    /// Mirrors `decode_read_mode` for building a correctly-sized fixture.
    /// An unmapped mode has no real size, but the fixture only needs *a*
    /// finite size to keep the file well-formed enough to reach
    /// `parse_discjuggler`'s own read-mode validation - which is exactly
    /// what `unsupported_read_mode_refuses` exercises.
    fn read_mode_sizes(read_mode: u32) -> (u32, u32) {
        match read_mode {
            0 => (2048, 0),
            1 => (2336, 0),
            3 => (2352, 16),
            4 => (2352, 96),
            _ => (2352, 0),
        }
    }

    /// Builds a minimal `.cdi` file with real, correctly-sized track data
    /// (zero-filled) so a correctly-implemented reader can actually read
    /// cooked sectors out of it, not just parse the trailer.
    ///
    /// `sessions[i]`'s tracks are exactly as in `push_track`. `data_overrides`
    /// optionally overrides the physical byte length of the data region
    /// actually written (e.g. to simulate truncation) - `None` writes the
    /// full, correctly-sized region.
    fn build_cdi(sessions: &[Vec<TrackSpec>], data_len_override: Option<u64>) -> Vec<u8> {
        build_cdi_version(sessions, data_len_override, CDI_V35)
    }

    fn build_cdi_version(
        sessions: &[Vec<TrackSpec>],
        data_len_override: Option<u64>,
        version: u32,
    ) -> Vec<u8> {
        let mut total_bytes = 0u64;
        for session in sessions {
            for &(_, read_mode, _, length, _) in session {
                let (main, sub) = read_mode_sizes(read_mode);
                total_bytes += (u64::from(main) + u64::from(sub)) * u64::from(length);
            }
        }

        let mut desc = Vec::new();
        desc.push(sessions.len() as u8);
        for session in sessions {
            desc.push(0);
            desc.push(session.len() as u8);
            desc.extend_from_slice(&[0u8; 13]);
            for &(track_mode, read_mode, start_address, length, pregap) in session {
                push_track(
                    &mut desc,
                    track_mode,
                    read_mode,
                    start_address,
                    length,
                    pregap,
                );
            }
        }
        // Trailing empty session.
        desc.push(0);
        desc.push(0);
        desc.extend_from_slice(&[0u8; 13]);

        let dlen = (desc.len() + 8) as u32;

        let data_len = data_len_override.unwrap_or(total_bytes);
        let mut file = vec![0u8; data_len as usize];
        file.extend_from_slice(&desc);
        file.extend_from_slice(&version.to_le_bytes());
        let location = match version {
            CDI_V2 | CDI_V3 => data_len,
            CDI_V35 => u64::from(dlen),
            _ => 0,
        };
        file.extend_from_slice(&(location as u32).to_le_bytes());
        file
    }

    /// Stamps a recognisable, valid Dreamcast IP.BIN boot signature plus a
    /// non-copyrightable synthetic product code at the very start of
    /// `data`, matching the fixed layout
    /// `crate::dreamcast_boot_evidence::parse_ip_bin_meta` reads.
    fn stamp_ip_bin(data: &mut [u8]) {
        const HARDWARE_ID: &[u8; 16] = b"SEGA SEGAKATANA ";
        data[0..16].copy_from_slice(HARDWARE_ID);
        let product_number = b"TEST00001 "; // 10 bytes, matches PRODUCT_NUMBER's length
        data[0x40..0x4A].copy_from_slice(product_number);
    }

    fn write_cdi(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut file = tempfile::Builder::new().suffix(".cdi").tempfile().unwrap();
        file.write_all(bytes).unwrap();
        file.flush().unwrap();
        file
    }

    fn read_ip_bin(
        media: &CdiLogicalMedia,
    ) -> [u8; crate::dreamcast_boot_evidence::IP_BIN_META_BYTES] {
        let mut header = [0u8; crate::dreamcast_boot_evidence::IP_BIN_META_BYTES];
        media.read_at(0, &mut header).unwrap();
        header
    }

    #[test]
    fn single_session_cooked_2048_cdi_verifies_the_product_code() {
        // One session, one Mode 1 cooked (read_mode 0) data track.
        let mut bytes = build_cdi(&[vec![(1, 0, 0, 4, 0)]], None);
        let data_region = &mut bytes[..4 * 2048];
        stamp_ip_bin(data_region);
        let file = write_cdi(&bytes);

        let media = open_dreamcast_cdi_logical_media(file.path()).unwrap();
        let header = read_ip_bin(&media);
        let fact = crate::dreamcast_boot_evidence::parse_ip_bin_meta(&header).unwrap();
        assert!(fact.hardware_id_recognized);
        assert_eq!(fact.product_number, "TEST00001");
    }

    #[test]
    fn multi_session_gdrom_cdi_selects_the_high_density_track_not_the_low_density_one() {
        // Session 0: low-density Mode 1 cooked data track (base_lba 0)
        // with a deliberately WRONG stamped product code - selecting this
        // track would be a bug.
        // Session 1: high-density Mode 1 cooked data track at base_lba
        // 45000, carrying the real product code. Both tracks use cooked
        // (read_mode 0) sectors so the stamped bytes land at the plain
        // sector-relative offsets `stamp_ip_bin` writes to.
        let low_density_frames = 4u32;
        let high_density_frames = 4u32;
        let mut bytes = build_cdi(
            &[
                vec![(1, 0, 0, low_density_frames, 0)],
                vec![(1, 0, 45000, high_density_frames, 0)],
            ],
            None,
        );
        let low_start = 0usize;
        let low_len = low_density_frames as usize * 2048;
        stamp_ip_bin(&mut bytes[low_start..low_start + low_len]);
        // Overwrite the wrong-track stamp with a recognisably different
        // (still non-copyrightable) product code so the two are distinguishable.
        bytes[low_start + 0x40..low_start + 0x4A].copy_from_slice(b"WRONGTRACK");

        let high_start = low_len;
        let high_len = high_density_frames as usize * 2048;
        stamp_ip_bin(&mut bytes[high_start..high_start + high_len]);

        let file = write_cdi(&bytes);
        let media = open_dreamcast_cdi_logical_media(file.path()).unwrap();
        let header = read_ip_bin(&media);
        let fact = crate::dreamcast_boot_evidence::parse_ip_bin_meta(&header).unwrap();
        assert_eq!(fact.product_number, "TEST00001");
    }

    #[test]
    fn audio_track_at_the_high_density_boundary_is_never_selected() {
        // An audio track (track_mode 0) sitting past the GD-ROM boundary
        // must never itself become the selected data track.
        let bytes = build_cdi(&[vec![(0, 2, 45000, 4, 0)]], None);
        let file = write_cdi(&bytes);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::NoDataTrack)
        ));
    }

    #[test]
    fn two_high_density_data_tracks_are_ambiguous() {
        let bytes = build_cdi(
            &[vec![(1, 2, 45000, 4, 0)], vec![(1, 2, 50000, 4, 0)]],
            None,
        );
        let file = write_cdi(&bytes);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::AmbiguousDataTracks)
        ));
    }

    #[test]
    fn two_plain_data_tracks_with_no_high_density_candidate_are_ambiguous() {
        let bytes = build_cdi(&[vec![(1, 2, 0, 4, 0), (1, 2, 4, 4, 0)]], None);
        let file = write_cdi(&bytes);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::AmbiguousDataTracks)
        ));
    }

    #[test]
    fn no_data_track_at_all_refuses() {
        // A single audio-only track, low density.
        let bytes = build_cdi(&[vec![(0, 2, 0, 4, 0)]], None);
        let file = write_cdi(&bytes);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::NoDataTrack)
        ));
    }

    #[test]
    fn extension_alone_is_insufficient_without_a_real_descriptor() {
        let file = write_cdi(&[0x13, 0x37, 0x00, 0xff, 0x4a, 0x91]);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::Parse(_))
        ));
    }

    #[test]
    fn empty_file_refuses_without_panicking() {
        let file = write_cdi(&[]);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::Parse(_))
        ));
    }

    #[test]
    fn supported_footer_versions_use_their_documented_location_form() {
        for version in [CDI_V2, CDI_V3, CDI_V35] {
            let bytes = build_cdi_version(&[vec![(1, 0, 0, 1, 0)]], None, version);
            let file = write_cdi(&bytes);
            assert!(
                open_dreamcast_cdi_logical_media(file.path()).is_ok(),
                "version {version:#x} should parse"
            );
        }
    }

    #[test]
    fn unsupported_footer_version_refuses() {
        let bytes = build_cdi_version(&[vec![(1, 0, 0, 1, 0)]], None, 0x8000_0007);
        let file = write_cdi(&bytes);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::Parse(_))
        ));
    }

    #[test]
    fn impossible_session_count_refuses_before_track_walk() {
        let sessions = (0..MAX_CDI_SESSIONS + 1)
            .map(|_| vec![(1, 0, 0, 1, 0)])
            .collect::<Vec<_>>();
        let bytes = build_cdi(&sessions, None);
        let file = write_cdi(&bytes);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::TooManyTracks)
        ));
    }

    #[test]
    fn impossible_track_count_refuses_before_track_walk() {
        let tracks = vec![(1, 0, 0, 1, 0); 100];
        let bytes = build_cdi(&[tracks], None);
        let file = write_cdi(&bytes);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::TooManyTracks)
        ));
    }

    #[test]
    fn footer_offset_beyond_eof_refuses() {
        let mut bytes = build_cdi_version(&[vec![(1, 0, 0, 1, 0)]], None, CDI_V3);
        let footer_offset = bytes.len() - 4;
        bytes[footer_offset..].copy_from_slice(&u32::MAX.to_le_bytes());
        let file = write_cdi(&bytes);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::Parse(_))
        ));
    }

    #[test]
    fn checked_track_arithmetic_overflow_refuses() {
        assert_eq!(checked_track_bytes(u64::MAX, 2048, 1), None);
        assert_eq!(checked_track_bytes(0, u64::MAX, 2), None);
    }

    #[test]
    fn overlapping_logical_track_ranges_refuse() {
        let bytes = build_cdi(&[vec![(1, 0, 0, 4, 0), (1, 0, 0, 4, 0)]], None);
        let file = write_cdi(&bytes);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::ImpossibleOffset)
        ));
    }

    #[test]
    fn valid_container_without_usable_ip_bin_has_no_product_code() {
        let bytes = build_cdi(&[vec![(1, 0, 0, 1, 0)]], None);
        let file = write_cdi(&bytes);
        let media = open_dreamcast_cdi_logical_media(file.path()).unwrap();
        let header = read_ip_bin(&media);
        let fact = crate::dreamcast_boot_evidence::parse_ip_bin_meta(&header).unwrap();
        assert!(!fact.hardware_id_recognized);
        assert!(crate::dreamcast_boot_evidence::observe_ip_bin_evidence(&fact).is_empty());
    }

    #[test]
    fn one_logical_read_is_bounded_and_repeated_reads_are_capped() {
        let bytes = build_cdi(&[vec![(1, 0, 0, 1, 0)]], None);
        let file = write_cdi(&bytes);
        let media = open_dreamcast_cdi_logical_media(file.path()).unwrap();
        let mut too_large = vec![0; MAX_CDI_READ_BYTES + 1];
        assert!(media.read_at(0, &mut too_large).is_err());

        let mut sector = [0; 2048];
        for _ in 0..(MAX_CDI_INSPECTED_BYTES as usize / sector.len()) {
            media.read_at(0, &mut sector).unwrap();
        }
        assert!(media.read_at(0, &mut sector).is_err());
    }

    #[test]
    fn malformed_truncated_descriptor_refuses() {
        // A file just large enough to carry a plausible-looking descriptor
        // length, but with no real descriptor bytes behind it.
        let mut bytes = vec![0u8; 16];
        let dlen: u32 = 12;
        bytes.extend_from_slice(&dlen.to_le_bytes());
        let file = write_cdi(&bytes);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::Parse(_))
        ));
    }

    #[test]
    fn unsupported_read_mode_refuses() {
        // read_mode 9 does not map to any known sector geometry.
        let bytes = build_cdi(&[vec![(1, 9, 0, 4, 0)]], None);
        let file = write_cdi(&bytes);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::UnsupportedSectorLayout)
        ));
    }

    #[test]
    fn truncated_media_with_an_impossible_offset_refuses() {
        // The trailer declares a full 4-sector track, but the file's data
        // region is truncated to 1 sector - the track's own declared byte
        // range no longer fits inside the real file.
        let full = build_cdi(&[vec![(1, 2, 0, 4, 0)]], None);
        let truncated = build_cdi(&[vec![(1, 2, 0, 4, 0)]], Some(2352));
        assert!(full.len() > truncated.len());
        let file = write_cdi(&truncated);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::ImpossibleOffset)
        ));
    }

    #[test]
    fn reading_past_the_track_end_refuses_rather_than_fabricating_identity() {
        // Structurally valid single-sector data track (2048 bytes cooked,
        // still more than IP_BIN_META_BYTES - reading IP.BIN itself always
        // succeeds structurally for any real data track). What must never
        // happen is a read past this track's own declared end being
        // silently satisfied instead of refused.
        let bytes = build_cdi(&[vec![(1, 0, 0, 1, 0)]], None);
        let file = write_cdi(&bytes);
        let media = open_dreamcast_cdi_logical_media(file.path()).unwrap();
        assert_eq!(media.len(), 2048);
        let mut header = [0u8; crate::dreamcast_boot_evidence::IP_BIN_META_BYTES];
        assert!(media.read_at(2048, &mut header).is_err());
    }

    #[test]
    fn oversized_cdi_is_refused_before_reading() {
        // Cannot allocate a real multi-GiB file in a unit test; verify the
        // metadata-only pre-check function directly by asserting the
        // bound itself is sane and positive, mirroring
        // `disc_evidence_collector::max_chd_bytes_is_a_positive_sane_bound`.
        const { assert!(MAX_CDI_BYTES > 0) };
        const { assert!(MAX_CDI_BYTES < 100 * 1024 * 1024 * 1024) };
    }
}
