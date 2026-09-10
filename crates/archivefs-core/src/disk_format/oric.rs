//! Reviewed Euphoric/OSDK MFM_DISK geometry-1 subset, never generic raw DSK.
//! Side-major 6400-byte tracks, 15..=18 ordinary 256-byte sectors, ID/data CRC16.
//! Sources and deliberate exclusions: docs/research/ORIC_MEDIA_IDENTITY_V1.md.

use super::{
    BoundedReader, DiskFormat, DiskFormatContext, DiskFormatEvidence, DiskFormatMetadata,
    DiskFormatRefusal, confidence_for,
};
use serde::Serialize;
use std::sync::atomic::AtomicBool;

pub const ORIC_MFM_HEADER_BYTES: usize = 256;
pub const ORIC_MFM_TRACK_BYTES: usize = 6400;
pub const MAX_ORIC_MFM_TRACKS: usize = 84;
pub const MAX_ORIC_MFM_BYTES: usize =
    ORIC_MFM_HEADER_BYTES + 2 * MAX_ORIC_MFM_TRACKS * ORIC_MFM_TRACK_BYTES;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OricMfmLayout {
    pub sides: u8,
    pub tracks_per_side: u8,
    pub sectors_per_track: u8,
    pub sector_bytes: u16,
}

fn bad(detail: &str) -> DiskFormatRefusal {
    DiskFormatRefusal::Malformed {
        detail: detail.into(),
    }
}

fn geometry(bytes: &[u8], total: usize) -> Result<(usize, usize), DiskFormatRefusal> {
    if total > MAX_ORIC_MFM_BYTES {
        return Err(DiskFormatRefusal::TooLarge {
            length: total as u64,
            maximum: MAX_ORIC_MFM_BYTES as u64,
        });
    }
    let h = bytes
        .get(..ORIC_MFM_HEADER_BYTES)
        .ok_or_else(|| bad("truncated MFM_DISK header"))?;
    if !h.starts_with(b"MFM_DISK") {
        return Err(bad("not an Oric MFM_DISK container"));
    }
    let le = |n| u32::from_le_bytes(h[n..n + 4].try_into().unwrap());
    let sides = le(8) as usize;
    let tracks = le(12) as usize;
    if !(1..=2).contains(&sides) || !(1..=MAX_ORIC_MFM_TRACKS).contains(&tracks) || le(16) != 1 {
        return Err(bad(
            "unsupported MFM_DISK geometry (V1: 1..2 sides, 1..84 tracks, geometry 1)",
        ));
    }
    let expected = ORIC_MFM_HEADER_BYTES + sides * tracks * ORIC_MFM_TRACK_BYTES;
    if expected != total {
        return Err(DiskFormatRefusal::GeometryMismatch {
            declared_bytes: expected as u64,
            actual_bytes: total as u64,
        });
    }
    Ok((sides, tracks))
}

fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0xffffu16;
    for byte in bytes {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn checked_crc(bytes: &[u8]) -> bool {
    let end = bytes.len() - 2;
    crc16(&bytes[..end]) == u16::from_be_bytes([bytes[end], bytes[end + 1]])
}

fn skip_gap(track: &[u8], at: &mut usize) {
    while track.get(*at).is_some_and(|b| matches!(b, 0 | 0x4e | 0x22)) {
        *at += 1;
    }
}

fn track_sectors(track: &[u8], cylinder: usize, side: usize) -> Result<u8, DiskFormatRefusal> {
    let mut at = 0usize;
    let mut sectors = 0u8;
    loop {
        skip_gap(track, &mut at);
        if at == track.len() {
            break;
        }
        let id = track
            .get(at..at + 10)
            .ok_or_else(|| bad("truncated MFM sector ID"))?;
        if id[..4] != [0xa1, 0xa1, 0xa1, 0xfe]
            || id[4] as usize != cylinder
            || id[5] as usize != side
            || id[6] != sectors + 1
            || id[7] != 1
            || sectors >= 18
        {
            return Err(bad("unsupported MFM sector ID, geometry, ordering or size"));
        }
        if !checked_crc(id) {
            return Err(bad("MFM sector ID CRC mismatch"));
        }
        at += 10;
        skip_gap(track, &mut at);
        let data = track
            .get(at..at + 262)
            .ok_or_else(|| bad("truncated MFM sector payload"))?;
        if data[..4] != [0xa1, 0xa1, 0xa1, 0xfb] {
            return Err(bad("unsupported MFM data address mark"));
        }
        if !checked_crc(data) {
            return Err(bad("MFM sector data CRC mismatch"));
        }
        at += 262;
        sectors += 1;
    }
    if !(15..=18).contains(&sectors) {
        return Err(bad(
            "V1 requires 15..18 ordinary sectors on every MFM track",
        ));
    }
    Ok(sectors)
}

pub fn parse_oric_mfm(bytes: &[u8]) -> Result<OricMfmLayout, DiskFormatRefusal> {
    let (sides, tracks) = geometry(bytes, bytes.len())?;
    let mut count = None;
    for side in 0..sides {
        for cylinder in 0..tracks {
            let offset = ORIC_MFM_HEADER_BYTES + (side * tracks + cylinder) * ORIC_MFM_TRACK_BYTES;
            let sectors = track_sectors(
                &bytes[offset..offset + ORIC_MFM_TRACK_BYTES],
                cylinder,
                side,
            )?;
            if count.is_some_and(|n| n != sectors) {
                return Err(bad("mixed MFM sector geometry is deferred"));
            }
            count = Some(sectors);
        }
    }
    Ok(OricMfmLayout {
        sides: sides as u8,
        tracks_per_side: tracks as u8,
        sectors_per_track: count.unwrap(),
        sector_bytes: 256,
    })
}

pub(super) fn inspect(
    reader: &mut BoundedReader<'_>,
    context: DiskFormatContext<'_>,
    cancel: Option<&AtomicBool>,
) -> DiskFormatEvidence {
    let result = (|| {
        if reader.len() > MAX_ORIC_MFM_BYTES as u64 {
            return Err(DiskFormatRefusal::TooLarge {
                length: reader.len(),
                maximum: MAX_ORIC_MFM_BYTES as u64,
            });
        }
        let h = reader.read_exact_at(0, ORIC_MFM_HEADER_BYTES)?;
        geometry(&h, reader.len() as usize)?;
        let mut bytes = h;
        while bytes.len() < reader.len() as usize {
            if super::cancelled(cancel) {
                return Err(DiskFormatRefusal::Cancelled);
            }
            let count =
                (reader.len() as usize - bytes.len()).min(super::MAX_DISK_FORMAT_READ_CHUNK);
            bytes.extend(reader.read_exact_at_with_offset_limit(
                bytes.len() as u64,
                count,
                MAX_ORIC_MFM_BYTES as u64,
            )?);
        }
        parse_oric_mfm(&bytes)
    })();
    match result {
        Ok(layout) => {
            let format = DiskFormat::OricMfm;
            let (confidence, conclusive) = confidence_for(format, context);
            DiskFormatEvidence { format: Some(format), platform: Some("Oric"), confidence, conclusive,
                evidence: vec!["Complete MFM_DISK geometry-1 container and ordinary sector ID/data CRCs validated; filesystem, machine model and release remain unknown".into()],
                bytes_inspected: reader.bytes_read(), refusal: None,
                metadata: Some(DiskFormatMetadata::OricMfm(layout)), read_via_symlink: false }
        }
        Err(error) => DiskFormatEvidence::refused(error),
    }
}
