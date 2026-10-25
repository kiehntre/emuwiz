//! Standard Acorn DFS `.ssd` / `.dsd` raw sector dumps.
//!
//! The catalogue layout and geometry checks here were cross-checked against
//! two independent references: [The Oddys' Acorn DFS layout
//! table](https://www.theoddys.com/acorn/acorn_system_filing_systems/disk_filing_system/acorn_system_disk_filing_system.html)
//! and Gerald Holdsworth's [*Guide To Disc Formats*](https://www.geraldholdsworth.co.uk/documents/DiscImage.pdf)
//! (pages 7-10). The layout also agrees with the independent `dfstool` reader
//! and libdsk's DFS geometry probe. In particular, standard DFS has 256-byte
//! sectors, ten sectors per track, catalogue sectors 0 and 1, 31 eight-byte
//! entries, and 40- or 80-track sides. A `.dsd` stores side tracks interleaved;
//! its second catalogue begins at byte `0x0a00`.
//!
//! This adapter reads only the two 256-byte catalogue sectors per side (plus a
//! small marker probe). It deliberately does not implement Watford/Keele
//! extended catalogues, ADFS, or any machine-specific interpretation.

use std::sync::atomic::AtomicBool;

use super::{
    BoundedReader, DfsFileEntry, DfsLayout, DfsSideLayout, DiskFormat, DiskFormatContext,
    DiskFormatEvidence, DiskFormatMetadata, DiskFormatRefusal, confidence_for,
};

const SECTOR_BYTES: u64 = 256;
const CATALOGUE_BYTES: usize = 512;
const MAX_FILES: usize = 31;
const SIDE40_SECTORS: u16 = 400;
const SIDE80_SECTORS: u16 = 800;
const SECOND_DSD_CATALOGUE_OFFSET: u64 = 0x0a00;
const SIDE2_MARKER_OFFSET: u64 = 0x0200;

pub(super) fn inspect(
    reader: &mut BoundedReader<'_>,
    context: DiskFormatContext<'_>,
    cancel: Option<&AtomicBool>,
    double_sided: bool,
) -> DiskFormatEvidence {
    match validate(reader, cancel, double_sided) {
        Ok(layout) => {
            let format = DiskFormat::AcornDfsDisk;
            let (confidence, conclusive) = confidence_for(format, context);
            let total_files: usize = layout.sides.iter().map(|side| side.files.len()).sum();
            let titles = layout
                .sides
                .iter()
                .map(|side| format!("{:?}", side.title))
                .collect::<Vec<_>>()
                .join(", ");
            let mut evidence = vec![
                format!(
                    "Standard Acorn DFS catalogue validated for {} side(s), {} file entr{}",
                    layout.sides.len(),
                    total_files,
                    if total_files == 1 { "y" } else { "ies" }
                ),
                format!("Disk title(s): {titles}"),
                "Catalogue sectors, file extents, and declared 40/80-track geometry agree with the raw image length".to_string(),
                "DFS is shared by BBC Micro/BBC Master and Acorn Electron; the structure does not identify one machine".to_string(),
            ];
            if let Some(folder) = context.folder_platform
                && folder != format.platform()
            {
                evidence.push(format!(
                    "The containing folder names {folder}; DFS structure remains family-level evidence"
                ));
            }
            DiskFormatEvidence {
                format: Some(format),
                platform: Some(format.platform()),
                confidence,
                conclusive,
                evidence,
                bytes_inspected: reader.bytes_read(),
                refusal: None,
                metadata: Some(DiskFormatMetadata::Dfs(layout)),
                read_via_symlink: false,
            }
        }
        Err(refusal) => {
            let mut refused = DiskFormatEvidence::refused(refusal);
            refused.bytes_inspected = reader.bytes_read();
            refused
        }
    }
}

fn validate(
    reader: &mut BoundedReader<'_>,
    cancel: Option<&AtomicBool>,
    double_sided: bool,
) -> Result<DfsLayout, DiskFormatRefusal> {
    let length = reader.len();
    let side_count = if double_sided { 2 } else { 1 };
    let side_bytes = length / side_count as u64;
    if !matches!(side_bytes,
        value if value == u64::from(SIDE40_SECTORS) * SECTOR_BYTES
            || value == u64::from(SIDE80_SECTORS) * SECTOR_BYTES)
    {
        return Err(DiskFormatRefusal::Malformed {
            detail: format!(
                "standard DFS geometry requires 100K/200K per `.ssd` side or 200K/400K `.dsd`, \
                 got {length} bytes for {side_count} side(s)"
            ),
        });
    }
    if super::cancelled(cancel) {
        return Err(DiskFormatRefusal::Cancelled);
    }

    let side_sectors =
        u16::try_from(side_bytes / SECTOR_BYTES).map_err(|_| DiskFormatRefusal::Malformed {
            detail: format!("{side_bytes} bytes does not fit a DFS sector count"),
        })?;
    if !matches!(side_sectors, SIDE40_SECTORS | SIDE80_SECTORS) {
        return Err(DiskFormatRefusal::Malformed {
            detail: format!("{side_sectors} sectors is not a standard 40- or 80-track DFS side"),
        });
    }

    let mut sides = Vec::with_capacity(side_count);
    for side in 0..side_count {
        if super::cancelled(cancel) {
            return Err(DiskFormatRefusal::Cancelled);
        }
        let catalogue_offset = if side == 0 {
            0
        } else {
            SECOND_DSD_CATALOGUE_OFFSET
        };
        let catalogue = reader.read_exact_at(catalogue_offset, CATALOGUE_BYTES)?;
        // Holdsworth documents 0xAA x 8 at sector 2 as Watford's extended
        // catalogue marker. Refuse it rather than treating its extra entries
        // as ordinary file data. Keele variants are not guessed or extended.
        let marker = reader.read_exact_at(catalogue_offset + SIDE2_MARKER_OFFSET, 8)?;
        if marker == [0xAA; 8] {
            return Err(DiskFormatRefusal::Malformed {
                detail: format!(
                    "side {} carries an unsupported Watford DFS extended catalogue",
                    side + 1
                ),
            });
        }
        sides.push(parse_side(&catalogue, side_sectors)?);
    }

    let expected_bytes = u64::from(side_sectors)
        .checked_mul(SECTOR_BYTES)
        .and_then(|bytes| bytes.checked_mul(side_count as u64))
        .ok_or_else(|| DiskFormatRefusal::Malformed {
            detail: "DFS geometry byte count overflowed".to_string(),
        })?;
    if expected_bytes != length {
        return Err(DiskFormatRefusal::GeometryMismatch {
            declared_bytes: expected_bytes,
            actual_bytes: length,
        });
    }
    Ok(DfsLayout {
        double_sided: side_count == 2,
        sides,
    })
}

fn parse_side(catalogue: &[u8], total_sectors: u16) -> Result<DfsSideLayout, DiskFormatRefusal> {
    let malformed = |detail: String| DiskFormatRefusal::Malformed { detail };
    let title = printable_text(&catalogue[0..8], "first eight title bytes")?;
    let title_tail = printable_text(&catalogue[256..260], "last four title bytes")?;
    let title = format!("{}{}", title, title_tail)
        .trim_end_matches('\0')
        .trim_end()
        .to_string();
    let cycle = catalogue[260];
    if cycle & 0x0f > 9 || cycle >> 4 > 9 {
        return Err(malformed(
            "catalogue cycle number is not valid BCD".to_string(),
        ));
    }
    let entries_byte = catalogue[261];
    if entries_byte & 7 != 0 || usize::from(entries_byte / 8) > MAX_FILES {
        return Err(malformed(format!(
            "catalogue entry count byte {entries_byte:#04x} is not a multiple of 8 for standard DFS"
        )));
    }
    let file_count = entries_byte / 8;
    let geometry = catalogue[262];
    if geometry & 0xcc != 0 {
        return Err(malformed(format!(
            "catalogue geometry byte {geometry:#04x} has reserved bits set"
        )));
    }
    let declared_sectors = (u16::from(geometry & 3) << 8) | u16::from(catalogue[263]);
    if declared_sectors != total_sectors {
        return Err(malformed(format!(
            "catalogue declares {declared_sectors} sectors but image geometry is {total_sectors}"
        )));
    }
    if declared_sectors < 3 {
        return Err(malformed(
            "catalogue declares fewer than the two catalogue sectors".to_string(),
        ));
    }

    let boot_option = (geometry >> 4) & 3;
    let mut files = Vec::with_capacity(usize::from(file_count));
    let mut extents: Vec<(u32, u32)> = Vec::with_capacity(usize::from(file_count));
    for index in 0..usize::from(file_count) {
        let name_offset = 8 + index * 8;
        let detail_offset = 256 + 8 + index * 8;
        let filename = printable_text(&catalogue[name_offset..name_offset + 7], "file name")?;
        let qualifier = catalogue[name_offset + 7];
        let directory_byte = qualifier & 0x7f;
        if !(0x20..=0x7e).contains(&directory_byte) {
            return Err(malformed(format!(
                "file entry {index} has a non-printable directory character"
            )));
        }
        let load_low = u32::from(u16::from_le_bytes([
            catalogue[detail_offset],
            catalogue[detail_offset + 1],
        ]));
        let execution_low = u32::from(u16::from_le_bytes([
            catalogue[detail_offset + 2],
            catalogue[detail_offset + 3],
        ]));
        let length_low = u32::from(u16::from_le_bytes([
            catalogue[detail_offset + 4],
            catalogue[detail_offset + 5],
        ]));
        let high = catalogue[detail_offset + 6];
        let start_sector = (u16::from(high & 3) << 8) | u16::from(catalogue[detail_offset + 7]);
        let load_address = load_low | (u32::from((high >> 2) & 3) << 16);
        let length = length_low | (u32::from((high >> 4) & 3) << 16);
        let execution_address = execution_low | (u32::from(high >> 6) << 16);
        let rounded_length = u64::from(length)
            .checked_add(SECTOR_BYTES - 1)
            .ok_or_else(|| malformed(format!("file entry {index} length rounding overflows")))?;
        let data_sectors = u16::try_from(rounded_length / SECTOR_BYTES)
            .map_err(|_| malformed(format!("file entry {index} length overflows sector count")))?;
        let end_sector = u32::from(start_sector)
            .checked_add(u32::from(data_sectors))
            .ok_or_else(|| malformed(format!("file entry {index} sector range overflows")))?;
        if start_sector < 2 || end_sector > u32::from(total_sectors) {
            return Err(malformed(format!(
                "file entry {index} starts at sector {start_sector} and extends past the {total_sectors}-sector side"
            )));
        }
        if extents.iter().any(|(other_start, other_end)| {
            u32::from(start_sector) < *other_end && *other_start < end_sector
        }) {
            return Err(malformed(format!(
                "file entry {index} overlaps another file extent"
            )));
        }
        extents.push((u32::from(start_sector), end_sector));
        files.push(DfsFileEntry {
            directory: char::from(directory_byte).to_string(),
            filename: filename.trim_end().to_string(),
            locked: qualifier & 0x80 != 0,
            load_address,
            execution_address,
            length,
            start_sector,
        });
    }
    Ok(DfsSideLayout {
        total_sectors,
        file_count,
        title,
        boot_option,
        files,
    })
}

fn printable_text<'a>(bytes: &'a [u8], field: &str) -> Result<&'a str, DiskFormatRefusal> {
    if bytes
        .iter()
        .any(|byte| *byte != 0 && !(0x20..=0x7e).contains(byte))
    {
        return Err(DiskFormatRefusal::Malformed {
            detail: format!("{field} contains a non-printable or high-bit byte"),
        });
    }
    std::str::from_utf8(bytes).map_err(|_| DiskFormatRefusal::Malformed {
        detail: format!("{field} is not ASCII"),
    })
}
