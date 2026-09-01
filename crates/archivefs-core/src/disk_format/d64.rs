//! Bounded structural evidence for sector-order Commodore 1541 `.d64` images.
//!
//! The layout was checked before implementation against two independent
//! references: VICE's `c1541`/1541 DOS reader and the D64 format reference at
//! <https://www.theflatnet.de/pub/cbm/65xx/text/d64.html>. Both describe the
//! same 256-byte sectors, 1541 zone geometry (21/19/18/17 sectors), BAM at
//! 18/0, and directory beginning at 18/1. The reference also documents the
//! 32-byte directory entry offsets used below. This adapter intentionally
//! supports only the verified standard 35-track sector-order images, with or
//! without the one-byte-per-sector error-info tail. Extended 40-track images
//! are deliberately outside this standard-1541 scope.

use std::collections::HashSet;
use std::sync::atomic::AtomicBool;

use super::{
    BoundedReader, D64DirectoryEntry, D64Layout, DiskFormat, DiskFormatContext, DiskFormatEvidence,
    DiskFormatMetadata, DiskFormatRefusal, confidence_for, le_u16,
};

const SECTOR_BYTES: u64 = 256;
const MAX_D64_OFFSET: u64 = 256 * 1024;
const D64_35_TRACK_BYTES: u64 = 174_848;
const D64_35_TRACK_ERROR_BYTES: u64 = 175_531;
const BAM_TRACK: u8 = 18;
const BAM_SECTOR: u8 = 0;
const DIRECTORY_TRACK: u8 = 18;
const DIRECTORY_FIRST_SECTOR: u8 = 1;
const DIRECTORY_ENTRY_BYTES: usize = 32;
const ENTRIES_PER_SECTOR: usize = 8;
const MAX_CHAIN_SECTORS: usize = 768;

fn sectors_on_track(track: u8) -> u8 {
    match track {
        1..=17 => 21,
        18..=24 => 19,
        25..=30 => 18,
        _ => 17,
    }
}

fn sector_count(tracks: u8) -> Option<u16> {
    (1..=tracks)
        .map(sectors_on_track)
        .map(u16::from)
        .try_fold(0_u16, |total, count| total.checked_add(count))
}

fn offset(track: u8, sector: u8) -> Option<u64> {
    let before = (1..track)
        .map(sectors_on_track)
        .map(u64::from)
        .try_fold(0_u64, |total, count| total.checked_add(count))?;
    before
        .checked_add(u64::from(sector))?
        .checked_mul(SECTOR_BYTES)
}

fn valid_ts(track: u8, sector: u8, tracks: u8) -> bool {
    (1..=tracks).contains(&track) && sector < sectors_on_track(track)
}

fn malformed(detail: impl Into<String>) -> DiskFormatRefusal {
    DiskFormatRefusal::Malformed {
        detail: detail.into(),
    }
}

pub(super) fn inspect(
    reader: &mut BoundedReader<'_>,
    context: DiskFormatContext<'_>,
    cancel: Option<&AtomicBool>,
) -> DiskFormatEvidence {
    reader.set_max_offset(MAX_D64_OFFSET);
    match validate(reader, cancel) {
        Ok(layout) => {
            let format = DiskFormat::Commodore1541D64;
            let (confidence, conclusive) = confidence_for(format, context);
            let mut evidence = vec![
                format!(
                    "1541 geometry: {} tracks, {} sectors of 256 bytes ({} bytes of sector data)",
                    layout.tracks,
                    layout.sectors,
                    u64::from(layout.sectors) * SECTOR_BYTES
                ),
                "BAM at track 18 sector 0 and a bounded directory chain starting at track 18 sector 1 validated".to_string(),
                format!("{} directory entr{} and chained file sectors validated", layout.directory.len(), if layout.directory.len() == 1 { "y" } else { "ies" }),
                "This proves Commodore 1541-style disk media only; D64 media is shared by C64, C128 and VIC-20 software, so platform selection remains external evidence.".to_string(),
            ];
            if layout.has_error_info_tail {
                evidence.push("The image has an explicitly bounded one-byte-per-sector error-info tail; tail bytes were preserved and not normalised".to_string());
            }
            if let Some(folder) = context.folder_platform {
                evidence.push(format!(
                    "The containing folder supplies the separate platform evidence: {folder}"
                ));
            }
            DiskFormatEvidence {
                format: Some(format),
                // There is no canonical machine platform for a D64 alone.
                platform: None,
                confidence,
                conclusive,
                evidence,
                bytes_inspected: reader.bytes_read(),
                refusal: None,
                metadata: Some(DiskFormatMetadata::D64(layout)),
                read_via_symlink: false,
            }
        }
        Err(refusal) => {
            let mut result = DiskFormatEvidence::refused(refusal);
            result.bytes_inspected = reader.bytes_read();
            result
        }
    }
}

fn validate(
    reader: &mut BoundedReader<'_>,
    cancel: Option<&AtomicBool>,
) -> Result<D64Layout, DiskFormatRefusal> {
    let length = reader.len();
    let (tracks, has_error_info_tail) = match length {
        D64_35_TRACK_BYTES => (35, false),
        D64_35_TRACK_ERROR_BYTES => (35, true),
        _ => {
            return Err(malformed(format!(
                "unsupported D64 length {length}; supported lengths are 174848 (35-track) or 175531 (35-track with error info)"
            )));
        }
    };
    let sectors = sector_count(tracks).ok_or_else(|| malformed("D64 sector count overflowed"))?;
    let sector_data_bytes = u64::from(sectors)
        .checked_mul(SECTOR_BYTES)
        .ok_or_else(|| malformed("D64 sector data size overflowed"))?;
    let tail_bytes = if has_error_info_tail {
        u64::from(sectors)
    } else {
        0
    };
    if sector_data_bytes
        .checked_add(tail_bytes)
        .ok_or_else(|| malformed("D64 total size overflowed"))?
        != length
    {
        return Err(malformed(
            "D64 geometry and error-info tail length disagree",
        ));
    }
    if super::cancelled(cancel) {
        return Err(DiskFormatRefusal::Cancelled);
    }

    let bam = read_sector(reader, BAM_TRACK, BAM_SECTOR, tracks)?;
    if bam[0] != DIRECTORY_TRACK || bam[1] != DIRECTORY_FIRST_SECTOR {
        return Err(malformed(format!(
            "BAM directory pointer is {}/{}; standard 1541 BAM must point to 18/1",
            bam[0], bam[1]
        )));
    }
    if bam[2] != 0x41 {
        return Err(malformed(format!(
            "BAM DOS version marker is 0x{:02X}; expected 0x41",
            bam[2]
        )));
    }
    for track in 1..=tracks {
        let base = 4 + (usize::from(track) - 1) * 4;
        let free = *bam
            .get(base)
            .ok_or_else(|| malformed("BAM track table is truncated"))?;
        let bitmap = bam
            .get(base + 1..base + 4)
            .ok_or_else(|| malformed("BAM bitmap is truncated"))?;
        let count = sectors_on_track(track);
        if bitmap[usize::from(count / 8)..]
            .iter()
            .enumerate()
            .any(|(i, byte)| {
                let first = if usize::from(count / 8) + i == usize::from(count / 8) {
                    count % 8
                } else {
                    0
                };
                first > 0 && byte & !((1u8 << first) - 1) != 0
            })
        {
            return Err(malformed(format!(
                "BAM has set bits beyond track {track}'s {count} sectors"
            )));
        }
        let free_bits = (0..count)
            .filter(|sector| bitmap[usize::from(*sector / 8)] & (1 << (*sector % 8)) != 0)
            .count();
        if usize::from(free) != free_bits {
            return Err(malformed(format!(
                "BAM free-sector count for track {track} is {free}, bitmap contains {free_bits}"
            )));
        }
    }
    let disk_name = bam[0x90..0xA0].try_into().expect("fixed BAM field");
    let disk_id = bam[0xA2..0xA4].try_into().expect("fixed BAM field");
    let dos_type = bam[0xA5..0xA7].try_into().expect("fixed BAM field");
    if dos_type != *b"2A" {
        return Err(malformed(format!(
            "BAM DOS type is {:?}; expected PETSCII 2A",
            dos_type
        )));
    }

    let mut directory = Vec::new();
    let mut seen = HashSet::new();
    let mut ts = (DIRECTORY_TRACK, DIRECTORY_FIRST_SECTOR);
    while ts.0 != 0 {
        if !valid_ts(ts.0, ts.1, tracks) {
            return Err(malformed(format!(
                "directory points to impossible track/sector {}/{}",
                ts.0, ts.1
            )));
        }
        if !seen.insert(ts) {
            return Err(malformed("directory sector chain loops"));
        }
        if seen.len() > MAX_CHAIN_SECTORS {
            return Err(malformed("directory chain exceeds bounded traversal"));
        }
        let sector = read_sector(reader, ts.0, ts.1, tracks)?;
        for index in 0..ENTRIES_PER_SECTOR {
            // The first directory entry occupies the two bytes immediately
            // after the sector link, so its 32-byte slot starts at zero; the
            // link bytes are the slot's unused first two bytes. Subsequent
            // slots start at 0x20, 0x40, ... .
            let start = index * DIRECTORY_ENTRY_BYTES;
            let entry = &sector[start..start + DIRECTORY_ENTRY_BYTES];
            let raw_type = entry[2];
            let kind = raw_type & 0x07;
            if kind == 0 {
                continue;
            }
            if !(1..=4).contains(&kind) {
                return Err(malformed(format!(
                    "directory entry has unsupported file type 0x{raw_type:02X}"
                )));
            }
            let start_track = entry[3];
            let start_sector = entry[4];
            let blocks =
                le_u16(entry, 0x1E).ok_or_else(|| malformed("directory entry is truncated"))?;
            if blocks > 0 && !valid_ts(start_track, start_sector, tracks) {
                return Err(malformed(format!(
                    "file starts at impossible track/sector {start_track}/{start_sector}"
                )));
            }
            if start_track == BAM_TRACK {
                return Err(malformed(
                    "file start points into reserved directory/BAM track 18",
                ));
            }
            let filename = entry[5..21].try_into().expect("fixed directory field");
            directory.push(D64DirectoryEntry {
                file_type: kind,
                closed: raw_type & 0x80 != 0,
                locked: raw_type & 0x40 != 0,
                start_track,
                start_sector,
                blocks,
                filename,
            });
            validate_file_chain(reader, start_track, start_sector, blocks, tracks, cancel)?;
        }
        ts = (sector[0], sector[1]);
        if ts.0 == 0 && ts.1 != 0xFF {
            return Err(malformed(format!(
                "directory terminator has sector byte 0x{:02X}, expected 0xFF",
                ts.1
            )));
        }
    }
    Ok(D64Layout {
        tracks,
        sectors,
        has_error_info_tail,
        disk_name,
        disk_id,
        dos_type,
        directory,
    })
}

fn read_sector(
    reader: &mut BoundedReader<'_>,
    track: u8,
    sector: u8,
    tracks: u8,
) -> Result<Vec<u8>, DiskFormatRefusal> {
    if !valid_ts(track, sector, tracks) {
        return Err(malformed(format!(
            "impossible track/sector {track}/{sector}"
        )));
    }
    let offset = offset(track, sector)
        .ok_or_else(|| malformed("D64 track/sector offset arithmetic overflowed"))?;
    reader.read_exact_at(offset, SECTOR_BYTES as usize)
}

fn validate_file_chain(
    reader: &mut BoundedReader<'_>,
    start_track: u8,
    start_sector: u8,
    blocks: u16,
    tracks: u8,
    cancel: Option<&AtomicBool>,
) -> Result<(), DiskFormatRefusal> {
    if blocks == 0 {
        return Ok(());
    }
    let mut seen = HashSet::new();
    let mut ts = (start_track, start_sector);
    for block in 0..blocks {
        if super::cancelled(cancel) {
            return Err(DiskFormatRefusal::Cancelled);
        }
        if !valid_ts(ts.0, ts.1, tracks) || ts.0 == BAM_TRACK {
            return Err(malformed(format!(
                "file chain points to impossible/reserved sector {}/{}",
                ts.0, ts.1
            )));
        }
        if !seen.insert(ts) {
            return Err(malformed("file sector chain loops"));
        }
        if seen.len() > MAX_CHAIN_SECTORS {
            return Err(malformed("file chain exceeds bounded traversal"));
        }
        let sector = read_sector(reader, ts.0, ts.1, tracks)?;
        let next = (sector[0], sector[1]);
        if block + 1 == blocks {
            if next.0 != 0 {
                return Err(malformed("file block count ends before the sector chain"));
            }
            if next.1 > 254 {
                return Err(malformed("last file sector has an impossible byte count"));
            }
        } else if !valid_ts(next.0, next.1, tracks) || next.0 == BAM_TRACK {
            return Err(malformed(
                "file sector chain is truncated or points to a reserved sector",
            ));
        } else {
            ts = next;
        }
    }
    Ok(())
}
