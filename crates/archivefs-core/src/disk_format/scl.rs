//! The `.scl` ("SINCLAIR") archive of TR-DOS files.
//!
//! # What the format is
//!
//! `.scl` is a compact transport container for a set of TR-DOS files - the
//! format emulators and TR-DOS tooling use to move a disk's contents around
//! without a whole sector image. Its layout, at fixed offsets:
//!
//! | Offset            | Size      | Field |
//! |-------------------|-----------|-------|
//! | 0x00              | 8         | the literal ASCII signature `SINCLAIR` |
//! | 0x08              | 1         | number of catalogued files, `N` (0..=128) |
//! | 0x09              | 14 * `N`  | directory: one 14-byte entry per file |
//! | 0x09 + 14*`N`     | 256 * `S` | file data, `S` = the sectors the entries declare between them |
//! | end - 4           | 4         | optional little-endian 32-bit sum of every preceding byte |
//!
//! Each 14-byte directory entry is a TR-DOS catalogue entry without its last
//! two bytes (start track / start sector, which `.scl` does not need because
//! files are stored back to back): 8-byte name, 1-byte type, 2-byte start
//! address, 2-byte length in bytes, 1-byte length in **sectors**.
//!
//! Verified against the community "SCL file format" description used by
//! Real Speccy / Unreal / ZXVDT and the same one every emulator's `.scl`
//! loader follows.
//!
//! # What a valid one proves
//!
//! The `SINCLAIR` signature and this exact table+payload arithmetic are
//! written only by TR-DOS / ZX Spectrum-family tooling, so a valid `.scl`
//! settles the platform ([`DiskFormat::proves_platform`] is `true`). It says
//! nothing about a *machine* subtype and nothing about which games the files
//! are - directory filenames are never read as identity here.
//!
//! # What is read
//!
//! The 9-byte header and the `14 * N`-byte directory (`N` <= 128, so at most
//! ~1.8 KiB). The file payload and the trailing checksum are **not** read;
//! their sizes are only checked against the file length. Verifying the
//! checksum would need a whole-file read, outside this module's budget, so
//! this reports only whether a checksum-sized tail is present.

use std::sync::atomic::AtomicBool;

use super::{
    BoundedReader, DiskFormat, DiskFormatContext, DiskFormatEvidence, DiskFormatMetadata,
    DiskFormatRefusal, MAX_SCL_BYTES, SclLayout, TRDOS_MAX_FILES, confidence_for,
};

const SIGNATURE: &[u8; 8] = b"SINCLAIR";
const HEADER_BYTES: usize = 9;
const DIR_ENTRY_BYTES: usize = 14;
/// Byte 13 of a directory entry: the file's length in 256-byte sectors.
const ENTRY_SECTORS_OFFSET: usize = 13;
const SECTOR_BYTES: u64 = 256;
const CHECKSUM_BYTES: u64 = 4;

pub(super) fn inspect(
    reader: &mut BoundedReader<'_>,
    context: DiskFormatContext<'_>,
    cancel: Option<&AtomicBool>,
) -> DiskFormatEvidence {
    match validate(reader, cancel) {
        Ok(layout) => {
            let format = DiskFormat::SpectrumSclArchive;
            let (confidence, conclusive) = confidence_for(format, context);
            let mut evidence = vec![
                format!(
                    "`SINCLAIR` signature and a {}-file directory at the start of the archive",
                    layout.file_count
                ),
                format!(
                    "The {} directory entries account for exactly {} 256-byte sector(s) of \
                     payload, matching the file's length{}",
                    layout.file_count,
                    layout.declared_sectors,
                    if layout.has_trailing_checksum {
                        " (with a 4-byte trailing checksum)"
                    } else {
                        ""
                    }
                ),
                "The `.scl` container is written only by TR-DOS / ZX Spectrum tooling, so a valid \
                 one settles the platform; it carries no machine subtype and no game identity"
                    .to_string(),
                "Directory filenames and the file payload were not read - only the header and the \
                 entry table were walked"
                    .to_string(),
            ];
            if let Some(folder) = context.folder_platform
                && folder != format.platform()
            {
                evidence.push(format!(
                    "The containing folder names {folder} instead, so the structure and the \
                     folder disagree"
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
                metadata: Some(DiskFormatMetadata::Scl(layout)),
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
) -> Result<SclLayout, DiskFormatRefusal> {
    let length = reader.len();
    if length < HEADER_BYTES as u64 {
        return Err(DiskFormatRefusal::TooSmall {
            length,
            minimum: HEADER_BYTES as u64,
        });
    }
    if length > MAX_SCL_BYTES {
        return Err(DiskFormatRefusal::TooLarge {
            length,
            maximum: MAX_SCL_BYTES,
        });
    }
    if super::cancelled(cancel) {
        return Err(DiskFormatRefusal::Cancelled);
    }

    let header = reader.read_exact_at(0, HEADER_BYTES)?;
    let malformed = |detail: String| DiskFormatRefusal::Malformed { detail };
    if header.get(0..8) != Some(&SIGNATURE[..]) {
        return Err(malformed(
            "the archive does not begin with the `SINCLAIR` signature".to_string(),
        ));
    }
    let file_count = header[8];
    if file_count > TRDOS_MAX_FILES {
        return Err(malformed(format!(
            "the header declares {file_count} files, more than the TR-DOS maximum of \
             {TRDOS_MAX_FILES}"
        )));
    }

    let dir_bytes = DIR_ENTRY_BYTES
        .checked_mul(usize::from(file_count))
        .ok_or_else(|| malformed("the directory size overflows".to_string()))?;
    let body_bytes = (HEADER_BYTES as u64)
        .checked_add(dir_bytes as u64)
        .ok_or_else(|| malformed("the header + directory size overflows".to_string()))?;
    if body_bytes > length {
        return Err(malformed(format!(
            "a {file_count}-entry directory needs {body_bytes} bytes, past the file's {length}"
        )));
    }

    // Walk the directory, one 14-byte entry at a time. Every read stays inside
    // the file (proven just above), so the walk is finite and never leaves it.
    let mut declared_sectors: u64 = 0;
    for index in 0..usize::from(file_count) {
        if super::cancelled(cancel) {
            return Err(DiskFormatRefusal::Cancelled);
        }
        let offset = (HEADER_BYTES + index * DIR_ENTRY_BYTES) as u64;
        let entry = reader.read_exact_at(offset, DIR_ENTRY_BYTES)?;
        let sectors = u64::from(entry[ENTRY_SECTORS_OFFSET]);
        declared_sectors = declared_sectors
            .checked_add(sectors)
            .ok_or_else(|| malformed("the declared sector total overflows".to_string()))?;
    }

    let payload_bytes = declared_sectors
        .checked_mul(SECTOR_BYTES)
        .ok_or_else(|| malformed("the declared payload size overflows".to_string()))?;
    let without_checksum = body_bytes
        .checked_add(payload_bytes)
        .ok_or_else(|| malformed("the declared archive size overflows".to_string()))?;
    let with_checksum = without_checksum
        .checked_add(CHECKSUM_BYTES)
        .ok_or_else(|| malformed("the declared archive size overflows".to_string()))?;

    let has_trailing_checksum = if length == without_checksum {
        false
    } else if length == with_checksum {
        true
    } else if length < without_checksum {
        return Err(DiskFormatRefusal::GeometryMismatch {
            declared_bytes: without_checksum,
            actual_bytes: length,
        });
    } else {
        return Err(DiskFormatRefusal::GeometryMismatch {
            declared_bytes: with_checksum,
            actual_bytes: length,
        });
    };

    Ok(SclLayout {
        file_count,
        declared_sectors: u32::try_from(declared_sectors).unwrap_or(u32::MAX),
        has_trailing_checksum,
    })
}
