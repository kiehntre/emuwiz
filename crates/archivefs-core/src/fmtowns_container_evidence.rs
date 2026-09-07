//! Bounded FM Towns boot evidence over already-validated disk containers.
//!
//! This adapter consumes D88/HDI/NHD metadata and performs no container
//! parsing of its own.  D88 gets an IPL4 boot-sector probe; HDI/NHD also get a
//! bounded `FBIOS` probe when the IPL4 sector declares a safe contiguous
//! IO.SYS range.  A container header or geometry alone is never sufficient.

use std::path::Path;

use crate::disk_format::{inspect_disk_format, DiskFormat, DiskFormatContext, DiskFormatMetadata};
use crate::fmtowns_boot_evidence::{inspect_fmtowns_boot_sector, FmTownsBootEvidence};
use crate::safe_read::{open_bounded_read, TrustedRoots};

const MAX_BOOT_READ: usize = 1024;
const FBIOS_MAGIC: &[u8; 5] = b"FBIOS";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FmTownsContainerEvidence {
    pub format: DiskFormat,
    pub boot_sector: Option<FmTownsBootEvidence>,
    pub container_detail: String,
}

/// Inspect one validated D88/HDI/NHD payload using one bounded boot read.
pub fn inspect_fmtowns_container(path: &Path) -> Option<FmTownsContainerEvidence> {
    let inspected = inspect_disk_format(
        path,
        &TrustedRoots::none(),
        DiskFormatContext::default(),
        None,
    );
    let format = inspected.format?;
    let (offset, sector_size, contiguous) = match inspected.metadata.as_ref()? {
        DiskFormatMetadata::D88(layout) => (layout.boot_sector_offset?, 512_u64, false),
        DiskFormatMetadata::Hdi(layout) | DiskFormatMetadata::Nhd(layout) => {
            (layout.data_offset, layout.sector_size, true)
        }
        _ => return None,
    };
    if !matches!(sector_size, 512 | 1024) {
        return None;
    }
    let mut file = open_bounded_read(path, &TrustedRoots::none()).ok()?;
    let read_len = usize::try_from(sector_size).ok()?.min(MAX_BOOT_READ);
    let sector = file.read_exact_at(offset, read_len, MAX_BOOT_READ)?;
    let mut boot = inspect_fmtowns_boot_sector(&sector);

    if contiguous && boot.townsos_candidate {
        let start = u64::from(boot.io_sys_start_sector?);
        let io_offset = offset.checked_add(start.checked_mul(sector_size)?)?;
        let io_sys = file.read_exact_at(io_offset, FBIOS_MAGIC.len(), FBIOS_MAGIC.len())?;
        if io_sys.as_slice() == FBIOS_MAGIC {
            boot.reasons
                .push("bounded IO.SYS probe found the TownsOS FBIOS marker".into());
        } else {
            boot.townsos_candidate = false;
            boot.warnings
                .push("declared IO.SYS range did not begin with FBIOS".into());
        }
    }

    Some(FmTownsContainerEvidence {
        format,
        boot_sector: Some(boot),
        container_detail: inspected.evidence.join(" "),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fmtowns_boot_evidence::FmTownsEvidenceConfidence;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(ext: &str) -> std::path::PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("emuwiz-fmtowns-{n}.{ext}"))
    }

    fn boot() -> [u8; 512] {
        let mut b = [0u8; 512];
        b[0..4].copy_from_slice(b"IPL4");
        b[4] = 0xeb;
        b[0x0b..0x0d].copy_from_slice(&512u16.to_le_bytes());
        b[0x0d] = 1;
        b[0x0e..0x10].copy_from_slice(&1u16.to_le_bytes());
        b[0x10] = 2;
        b[0x11..0x13].copy_from_slice(&224u16.to_le_bytes());
        b[0x13..0x15].copy_from_slice(&1440u16.to_le_bytes());
        b[0x15] = 0xf0;
        b[0x18..0x1a].copy_from_slice(&9u16.to_le_bytes());
        b[0x1a..0x1c].copy_from_slice(&2u16.to_le_bytes());
        b[0x20..0x24].copy_from_slice(&1u32.to_le_bytes());
        b[0x24..0x28].copy_from_slice(&1u32.to_le_bytes());
        b
    }

    fn hdi(path: &Path, towns: bool) {
        let mut bytes = vec![0u8; 0x20];
        bytes[8..12].copy_from_slice(&(0x20u32).to_le_bytes());
        bytes[0x0c..0x10].copy_from_slice(&(1024u32).to_le_bytes());
        bytes[0x10..0x14].copy_from_slice(&(512u32).to_le_bytes());
        bytes[0x14..0x18].copy_from_slice(&(2u32).to_le_bytes());
        bytes[0x18..0x1c].copy_from_slice(&(1u32).to_le_bytes());
        bytes[0x1c..0x20].copy_from_slice(&(1u32).to_le_bytes());
        bytes.extend_from_slice(&boot());
        let mut io_sys = vec![0u8; 512];
        io_sys[..5].copy_from_slice(if towns { b"FBIOS" } else { b"NOPE!" });
        bytes.extend_from_slice(&io_sys);
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn hdi_requires_townsos_marker_for_townsos_candidate() {
        for marker in [true, false] {
            let path = temp_path("hdi");
            hdi(&path, marker);
            let evidence = inspect_fmtowns_container(&path)
                .unwrap()
                .boot_sector
                .unwrap();
            assert_eq!(evidence.confidence, FmTownsEvidenceConfidence::Strong);
            assert_eq!(evidence.townsos_candidate, marker);
            fs::remove_file(path).unwrap();
        }
    }
}
