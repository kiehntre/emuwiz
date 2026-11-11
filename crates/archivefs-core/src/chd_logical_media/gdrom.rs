//! Pure-Rust, read-only access to a Dreamcast GD-ROM CHD's high-density
//! area - the part of the disc that holds `IP.BIN` and the game's own
//! ISO 9660 volume.
//!
//! [`super::ChdTrackLogicalMediaReader`] alone only reaches track 1, which
//! on a GD-ROM is the small low-density warning area. This module places
//! each high-density data track the same way chdman/MAME's `cdrom_file`
//! does:
//!
//! - **Physical** position inside the CHD: tracks are stored back to back,
//!   each padded up to a multiple of [`CHD_TRACK_PADDING_FRAMES`] frames
//!   (MAME `CD_TRACK_PADDING`). Verified against real redump GD-ROM CHDs:
//!   the padded sum of every track equals chdman's own `Total Units`.
//! - **Logical** GD-ROM LBA: the unpadded running frame count, so the first
//!   high-density track starts at exactly
//!   [`GDROM_HIGH_DENSITY_START_LBA`] (45000) on a well-formed disc.
//!
//! Addressing matches the optional specialist backend's
//! `GdromSectorReader`: a block below 45000 is read relative to the first
//! high-density track (the volume descriptors at block 16 live there),
//! anything else is routed to whichever high-density data track holds that
//! absolute LBA.
//!
//! Fails closed, never guessed: non-GD-ROM metadata, out-of-order tracks, a
//! stored pregap (`PGTYPE:V...`), a first high-density track not at 45000,
//! padded geometry larger than the CHD itself, or an unsupported sector
//! type are all refusals. A wrong layout would also fail the downstream
//! `SEGA SEGAKATANA` IP.BIN check, but this reader refuses before that.

use std::fs::File;
use std::path::Path;

use chd::Chd;
use chd::header::Header;

use super::{ChdLogicalMediaError, ChdTrackLogicalMediaReader, TrackKind};
use crate::chd_identity::{ChdMetadataFact, ChdMetadataOutcome, GdromTrackFact};
use crate::logical_media::{LogicalMedia, LogicalMediaError};
use crate::raw_cd_sector::LOGICAL_BLOCK_BYTES;

/// MAME's `CD_TRACK_PADDING`: every track's frames are padded to this
/// multiple inside the CHD.
pub const CHD_TRACK_PADDING_FRAMES: u64 = 4;

/// First LBA of a Dreamcast GD-ROM's high-density area.
pub const GDROM_HIGH_DENSITY_START_LBA: u64 =
    crate::chd_identity::GDROM_HIGH_DENSITY_START_FRAME as u64;

/// Where one track sits, both logically and inside the CHD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GdromTrackPlacement {
    pub track: u32,
    pub track_type: String,
    pub frames: u64,
    pub logical_start_lba: u64,
    pub physical_start_frame: u64,
}

/// Lays out every GD-ROM track from the CHD's metadata chain. Pure
/// arithmetic over metadata; no sector is read.
///
/// Returns the placements plus the padded physical frame total.
pub fn place_gdrom_tracks(
    tracks: &[GdromTrackFact],
) -> Result<(Vec<GdromTrackPlacement>, u64), ChdLogicalMediaError> {
    if tracks.is_empty() {
        return Err(ChdLogicalMediaError::NoDataTrack);
    }
    let mut placements = Vec::with_capacity(tracks.len());
    let mut logical = 0u64;
    let mut physical = 0u64;
    for (index, track) in tracks.iter().enumerate() {
        let expected = u32::try_from(index + 1).unwrap_or(u32::MAX);
        if track.track != expected {
            return Err(ChdLogicalMediaError::Codec {
                detail: format!(
                    "GD-ROM track metadata is not in track order (found track {} at position {expected})",
                    track.track
                ),
            });
        }
        let stored_pregap = track
            .pregap_type
            .as_deref()
            .is_some_and(|kind| kind.starts_with('V'))
            && track.pregap.unwrap_or(0) > 0;
        if stored_pregap {
            return Err(ChdLogicalMediaError::UnsupportedPregap {
                pregap: track.pregap,
            });
        }
        let frames = u64::from(track.frames);
        placements.push(GdromTrackPlacement {
            track: track.track,
            track_type: track.track_type.clone(),
            frames,
            logical_start_lba: logical,
            physical_start_frame: physical,
        });
        logical += frames;
        physical += frames.div_ceil(CHD_TRACK_PADDING_FRAMES) * CHD_TRACK_PADDING_FRAMES;
    }
    Ok((placements, physical))
}

/// The high-density data tracks of a placed GD-ROM, in disc order. The
/// first one must start exactly at [`GDROM_HIGH_DENSITY_START_LBA`].
pub fn high_density_data_tracks(
    placements: &[GdromTrackPlacement],
) -> Result<Vec<&GdromTrackPlacement>, ChdLogicalMediaError> {
    let hd: Vec<_> = placements
        .iter()
        .filter(|t| t.track_type != "AUDIO" && t.logical_start_lba >= GDROM_HIGH_DENSITY_START_LBA)
        .collect();
    let first = hd.first().ok_or(ChdLogicalMediaError::NoDataTrack)?;
    if first.logical_start_lba != GDROM_HIGH_DENSITY_START_LBA {
        return Err(ChdLogicalMediaError::UnsupportedTrackPosition { track: first.track });
    }
    Ok(hd)
}

fn track_kind(track_type: &str) -> Result<TrackKind, ChdLogicalMediaError> {
    match track_type {
        "MODE1_RAW" => Ok(TrackKind::Mode1Raw),
        "MODE1" => Ok(TrackKind::Mode1Cooked),
        "MODE2_RAW" => Ok(TrackKind::Mode2Raw),
        other => Err(ChdLogicalMediaError::UnsupportedTrackType {
            track_type: other.to_string(),
        }),
    }
}

struct Segment {
    start_lba: u64,
    frames: u64,
    reader: ChdTrackLogicalMediaReader<File>,
}

/// A [`LogicalMedia`] over every high-density data track of one GD-ROM CHD,
/// addressed by GD-ROM LBA (see the module documentation).
pub struct ChdGdromHighDensityMedia {
    segments: Vec<Segment>,
    len: u64,
}

impl std::fmt::Debug for ChdGdromHighDensityMedia {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChdGdromHighDensityMedia")
            .field(
                "segments",
                &self
                    .segments
                    .iter()
                    .map(|s| (s.start_lba, s.frames))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Opens `path` (a standalone, non-child GD-ROM CHD) for high-density reads.
/// Reads only the header, metadata chain and hunk map at open time.
pub fn open_chd_gdrom_high_density_file(
    path: &Path,
) -> Result<ChdGdromHighDensityMedia, ChdLogicalMediaError> {
    let identity = crate::chd_identity::observe_chd_identity_file(path)
        .map_err(ChdLogicalMediaError::Header)?;
    if identity.parent_required {
        return Err(ChdLogicalMediaError::NeedsParent {
            parent_sha1: identity.parent_sha1,
        });
    }
    let ChdMetadataOutcome::Observed(metadata) = &identity.metadata else {
        return Err(ChdLogicalMediaError::NoDataTrack);
    };
    let mut gd_tracks = Vec::new();
    for entry in &metadata.entries {
        match &entry.fact {
            ChdMetadataFact::GdromTrack(track) => gd_tracks.push(track.clone()),
            ChdMetadataFact::CdromTrack(_) => {
                return Err(ChdLogicalMediaError::Codec {
                    detail: "CHD mixes CD-ROM and GD-ROM track metadata".to_string(),
                });
            }
            _ => {}
        }
    }
    let (placements, physical_total) = place_gdrom_tracks(&gd_tracks)?;
    let hd = high_density_data_tracks(&placements)?;

    let mut segments = Vec::with_capacity(hd.len());
    for placement in hd {
        let kind = track_kind(&placement.track_type)?;
        let file = File::open(path).map_err(|error| ChdLogicalMediaError::Codec {
            detail: error.to_string(),
        })?;
        let chd = Chd::open(file, None).map_err(|error| match error {
            chd::Error::RequiresParent => ChdLogicalMediaError::NeedsParent {
                parent_sha1: identity.parent_sha1,
            },
            other => ChdLogicalMediaError::Codec {
                detail: format!("{other:?}"),
            },
        })?;
        let Header::V5Header(header) = chd.header() else {
            return Err(ChdLogicalMediaError::Codec {
                detail: "expected a CHD v5 header".to_string(),
            });
        };
        let hunk_bytes = u64::from(header.hunk_bytes);
        let unit_bytes = u64::from(header.unit_bytes);
        if hunk_bytes == 0 || unit_bytes == 0 || !hunk_bytes.is_multiple_of(unit_bytes) {
            return Err(ChdLogicalMediaError::Codec {
                detail: "CHD has invalid hunk/unit geometry".to_string(),
            });
        }
        if unit_bytes != 2448 {
            return Err(ChdLogicalMediaError::UnsupportedTrackType {
                track_type: format!("{} with non-CD frame geometry", placement.track_type),
            });
        }
        if physical_total.saturating_mul(unit_bytes) > header.logical_bytes {
            return Err(ChdLogicalMediaError::Codec {
                detail: format!(
                    "padded GD-ROM track layout ({physical_total} frames) exceeds the CHD's logical size"
                ),
            });
        }
        segments.push(Segment {
            start_lba: placement.logical_start_lba,
            frames: placement.frames,
            reader: ChdTrackLogicalMediaReader {
                chd: std::cell::RefCell::new(chd),
                hunk_bytes,
                unit_bytes,
                start_frame: placement.physical_start_frame,
                frame_count: placement.frames,
                track_kind: kind,
                cache: std::cell::RefCell::new(None),
            },
        });
    }
    let last = segments.last().ok_or(ChdLogicalMediaError::NoDataTrack)?;
    let len = (last.start_lba + last.frames) * LOGICAL_BLOCK_BYTES as u64;
    Ok(ChdGdromHighDensityMedia { segments, len })
}

impl ChdGdromHighDensityMedia {
    /// `(segment index, block within that segment)` for one 2048-byte block.
    fn locate(&self, lba: u64) -> Option<(usize, u64)> {
        if lba < GDROM_HIGH_DENSITY_START_LBA {
            let first = self.segments.first()?;
            return (lba < first.frames).then_some((0, lba));
        }
        self.segments
            .iter()
            .position(|s| lba >= s.start_lba && lba < s.start_lba + s.frames)
            .map(|index| (index, lba - self.segments[index].start_lba))
    }
}

impl LogicalMedia for ChdGdromHighDensityMedia {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), LogicalMediaError> {
        let out_of_bounds = || LogicalMediaError::OutOfBounds {
            offset,
            requested_len: buf.len(),
            media_len: self.len,
        };
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or_else(out_of_bounds)?;
        if end > self.len {
            return Err(out_of_bounds());
        }
        let block = LOGICAL_BLOCK_BYTES as u64;
        let mut filled = 0usize;
        while filled < buf.len() {
            let absolute = offset + filled as u64;
            let lba = absolute / block;
            let within = (absolute % block) as usize;
            let (segment, relative) =
                self.locate(lba)
                    .ok_or_else(|| LogicalMediaError::DecodeFailed {
                        detail: format!(
                            "GD-ROM LBA {lba} is not within any high-density data track"
                        ),
                    })?;
            let take = (LOGICAL_BLOCK_BYTES - within).min(buf.len() - filled);
            self.segments[segment].reader.read_at(
                relative * block + within as u64,
                &mut buf[filled..filled + take],
            )?;
            filled += take;
        }
        Ok(())
    }
}

/// Test-only builder: an uncompressed CHD v5 with chdman's GD-ROM layout
/// (unit 2448, 8 frames per hunk). Tracks 1-2 are absent (zero hunks);
/// `hd_blocks` 2048-byte blocks fill track 3 as MODE1_RAW sectors, and every
/// padding frame chdman inserts before track 3 holds a `WRONG FRAME` sentinel
/// so a reader that ignored the padding would read it instead of IP.BIN.
#[cfg(test)]
pub(crate) fn synthetic_gdrom_chd(hd_blocks: &[[u8; 2048]]) -> Vec<u8> {
    use crate::chd_identity::{CHD_METADATA_HEADER_BYTES, meta_tag};
    use crate::dat::archive::chd::CHD_MAGIC;
    fn put_u32(b: &mut [u8], o: usize, v: u32) {
        b[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }
    fn put_u64(b: &mut [u8], o: usize, v: u64) {
        b[o..o + 8].copy_from_slice(&v.to_be_bytes());
    }
    const UNIT: u64 = 2448;
    const PER_HUNK: u64 = 8;
    let t3_frames = hd_blocks.len() as u64;
    let t3_physical = 452 + 44_552;
    let total_frames = t3_physical + t3_frames;
    let hunk_bytes = UNIT * PER_HUNK;
    let hunk_count = total_frames.div_ceil(PER_HUNK);
    let logical_bytes = hunk_count * hunk_bytes;

    let mut data = vec![0u8; 124];
    data[0..8].copy_from_slice(CHD_MAGIC);
    put_u32(&mut data, 8, 124);
    put_u32(&mut data, 12, 5);
    put_u64(&mut data, 32, logical_bytes);
    put_u32(&mut data, 56, hunk_bytes as u32);
    put_u32(&mut data, 60, UNIT as u32);

    let t3 = format!(
        "TRACK:3 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:{t3_frames} PAD:0 PREGAP:150 PGTYPE:MODE1 PGSUB:NONE POSTGAP:0"
    );
    let entries: [Vec<u8>; 3] = [
        b"TRACK:1 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:450 PAD:0 PREGAP:0 PGTYPE:MODE1 PGSUB:NONE POSTGAP:0".to_vec(),
        b"TRACK:2 TYPE:AUDIO SUBTYPE:NONE FRAMES:44550 PAD:0 PREGAP:0 PGTYPE:MODE1 PGSUB:NONE POSTGAP:0".to_vec(),
        t3.into_bytes(),
    ];
    let meta_start = data.len() as u64;
    let mut offsets = Vec::new();
    let mut cursor = meta_start;
    for payload in &entries {
        offsets.push(cursor);
        cursor += CHD_METADATA_HEADER_BYTES as u64 + payload.len() as u64;
    }
    for (index, payload) in entries.iter().enumerate() {
        let next = offsets.get(index + 1).copied().unwrap_or(0);
        data.extend_from_slice(&meta_tag::GDROM_TRACK.to_be_bytes());
        data.push(0);
        data.extend_from_slice(&(payload.len() as u32).to_be_bytes()[1..]);
        data.extend_from_slice(&next.to_be_bytes());
        data.extend_from_slice(payload);
    }
    let map_offset = data.len() as u64;
    let first_hunk = (t3_physical - 4) / PER_HUNK;
    let map_end = map_offset + hunk_count * 4;
    let data_start = map_end.div_ceil(hunk_bytes).max(1) * hunk_bytes;
    let base = data_start / hunk_bytes;
    for hunk in 0..hunk_count {
        let value = if hunk >= first_hunk {
            base + hunk - first_hunk
        } else {
            0
        };
        data.extend_from_slice(&(value as u32).to_be_bytes());
    }
    data.resize(data_start as usize, 0);
    for frame in first_hunk * PER_HUNK..hunk_count * PER_HUNK {
        let mut unit = [0u8; UNIT as usize];
        unit[..12].copy_from_slice(&[
            0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0,
        ]);
        unit[15] = 1;
        if frame < t3_physical {
            unit[16..27].copy_from_slice(b"WRONG FRAME");
        } else if let Some(block) = hd_blocks.get((frame - t3_physical) as usize) {
            unit[16..16 + 2048].copy_from_slice(block);
        }
        data.extend_from_slice(&unit);
    }
    put_u64(&mut data, 40, map_offset);
    put_u64(&mut data, 48, meta_start);
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_gdrom_chd_reads_ip_bin_past_the_padding_frames() {
        let mut blocks = vec![[0u8; 2048]; 20];
        blocks[0][..16].copy_from_slice(b"SEGA SEGAKATANA ");
        blocks[16][1..6].copy_from_slice(b"CD001");
        blocks[19][..4].copy_from_slice(b"LAST");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("game.chd");
        std::fs::write(&path, synthetic_gdrom_chd(&blocks)).unwrap();
        let media = open_chd_gdrom_high_density_file(&path).unwrap();
        let mut ip = [0u8; 16];
        media.read_at(0, &mut ip).unwrap();
        assert_eq!(&ip, b"SEGA SEGAKATANA ");
        let mut absolute = [0u8; 16];
        media.read_at(45_000 * 2048, &mut absolute).unwrap();
        assert_eq!(&absolute, b"SEGA SEGAKATANA ");
        let mut pvd = [0u8; 6];
        media.read_at(16 * 2048, &mut pvd).unwrap();
        assert_eq!(&pvd[1..], b"CD001");
        let mut last = [0u8; 4];
        media.read_at((45_000 + 19) * 2048, &mut last).unwrap();
        assert_eq!(&last, b"LAST");
        assert!(media.read_at(45_020 * 2048, &mut [0u8; 1]).is_err());
    }

    fn track(n: u32, kind: &str, frames: u32) -> GdromTrackFact {
        GdromTrackFact {
            track: n,
            track_type: kind.to_string(),
            subtype: "NONE".to_string(),
            frames,
            pad: Some(0),
            pregap: Some(0),
            pregap_type: Some("MODE1".to_string()),
            pregap_subtype: Some("NONE".to_string()),
            postgap: Some(0),
        }
    }

    /// Track table copied from the real "Mr. Driller (USA)" redump CHD
    /// (`chdman info`): Total Units 549200, track 3 at logical 45000.
    fn mr_driller() -> Vec<GdromTrackFact> {
        let frames = [
            450, 44550, 288980, 13232, 10282, 12297, 12490, 14881, 11326, 16149, 12175, 12419,
            11994, 9219, 6850, 3161, 4111, 8567, 4398, 7559, 6629, 7202, 6397, 1918, 471, 4111,
            3157, 8956, 5219,
        ];
        frames
            .iter()
            .enumerate()
            .map(|(i, &f)| {
                let n = i as u32 + 1;
                let kind = if n == 1 || n == 3 || n == 29 {
                    "MODE1_RAW"
                } else {
                    "AUDIO"
                };
                track(n, kind, f)
            })
            .collect()
    }

    #[test]
    fn padded_layout_matches_chdman_total_units_for_a_real_disc() {
        let (placed, physical) = place_gdrom_tracks(&mr_driller()).unwrap();
        assert_eq!(physical, 549_200);
        assert_eq!(placed[2].logical_start_lba, 45_000);
        assert_eq!(placed[2].physical_start_frame, 45_004);
        assert_eq!(placed[28].logical_start_lba, 543_931);
        assert_eq!(placed[28].physical_start_frame, 543_980);
        let hd = high_density_data_tracks(&placed).unwrap();
        assert_eq!(hd.iter().map(|t| t.track).collect::<Vec<_>>(), vec![3, 29]);
    }

    #[test]
    fn first_high_density_track_off_45000_is_refused() {
        let tracks = vec![
            track(1, "MODE1_RAW", 300),
            track(2, "AUDIO", 44_701),
            track(3, "MODE1_RAW", 10),
        ];
        let (placed, _) = place_gdrom_tracks(&tracks).unwrap();
        assert!(matches!(
            high_density_data_tracks(&placed),
            Err(ChdLogicalMediaError::UnsupportedTrackPosition { track: 3 })
        ));
    }

    #[test]
    fn stored_pregap_and_out_of_order_tracks_are_refused() {
        let mut stored = mr_driller();
        stored[2].pregap = Some(150);
        stored[2].pregap_type = Some("V MODE1".to_string());
        assert!(matches!(
            place_gdrom_tracks(&stored),
            Err(ChdLogicalMediaError::UnsupportedPregap { pregap: Some(150) })
        ));
        let mut swapped = mr_driller();
        swapped.swap(0, 1);
        assert!(matches!(
            place_gdrom_tracks(&swapped),
            Err(ChdLogicalMediaError::Codec { .. })
        ));
        assert!(matches!(
            place_gdrom_tracks(&[]),
            Err(ChdLogicalMediaError::NoDataTrack)
        ));
    }

    #[test]
    fn a_disc_with_no_high_density_data_is_refused() {
        let tracks = vec![
            track(1, "MODE1_RAW", 300),
            track(2, "AUDIO", 44_700),
            track(3, "AUDIO", 10),
        ];
        let (placed, _) = place_gdrom_tracks(&tracks).unwrap();
        assert!(matches!(
            high_density_data_tracks(&placed),
            Err(ChdLogicalMediaError::NoDataTrack)
        ));
    }

    /// Opt-in real-media check: `EMUWIZ_REAL_DC_CHD=/path/game.chd`. Reads
    /// only IP.BIN's first block; never writes.
    #[test]
    fn real_gdrom_chd_exposes_segakatana_ip_bin_when_provided() {
        let Some(path) = std::env::var_os("EMUWIZ_REAL_DC_CHD") else {
            return;
        };
        let media = open_chd_gdrom_high_density_file(Path::new(&path)).unwrap();
        let mut ip = [0u8; 16];
        media.read_at(0, &mut ip).unwrap();
        assert_eq!(&ip, b"SEGA SEGAKATANA ");
        let mut pvd = [0u8; 6];
        media.read_at(16 * 2048, &mut pvd).unwrap();
        assert_eq!(&pvd[1..6], b"CD001");
    }
}

