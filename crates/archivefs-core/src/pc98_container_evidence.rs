//! Bounded wiring from shared Japanese disk containers to PC-98 boot evidence.
//!
//! This module deliberately does not parse D88/HDI/NHD. It consumes the
//! validated layout produced by `disk_format`, reads at most one logical
//! sector, and delegates interpretation to `pc98_boot_evidence`.

use std::path::Path;

use crate::disk_format::{DiskFormat, DiskFormatContext, DiskFormatMetadata, inspect_disk_format};
use crate::pc98_boot_evidence::{Pc98BootEvidence, inspect_pc98_boot_sector};
use crate::safe_read::{TrustedRoots, open_bounded_read};

const BOOT_SECTOR_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pc98ContainerEvidence {
    pub format: DiskFormat,
    pub boot_sector: Option<Pc98BootEvidence>,
    pub container_detail: String,
}

/// Inspect one supported D88/HDI/NHD container and, when its parsed layout
/// establishes a bounded logical sector location, inspect that sector with
/// the existing PC-98 primitive. A shared container or geometry alone never
/// produces a PC-98 result.
pub fn inspect_pc98_container(path: &Path) -> Option<Pc98ContainerEvidence> {
    let inspected = inspect_disk_format(
        path,
        &TrustedRoots::none(),
        DiskFormatContext::default(),
        None,
    );
    let format = inspected.format?;
    let offset = match inspected.metadata.as_ref()? {
        DiskFormatMetadata::D88(layout) => layout.boot_sector_offset,
        DiskFormatMetadata::Hdi(layout) | DiskFormatMetadata::Nhd(layout) => {
            (layout.sector_size == BOOT_SECTOR_BYTES as u64).then_some(layout.data_offset)
        }
        _ => return None,
    }?;
    let mut file = open_bounded_read(path, &TrustedRoots::none()).ok()?;
    let sector = file.read_exact_at(offset, BOOT_SECTOR_BYTES, BOOT_SECTOR_BYTES)?;
    Some(Pc98ContainerEvidence {
        format,
        boot_sector: Some(inspect_pc98_boot_sector(&sector)),
        container_detail: inspected.evidence.join(" "),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pc98_boot_evidence::Pc98EvidenceConfidence;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(ext: &str) -> std::path::PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("emuwiz-pc98-{n}.{ext}"))
    }

    fn boot(oem: &[u8; 8]) -> [u8; 512] {
        let mut b = [0u8; 512];
        b[3..11].copy_from_slice(oem);
        b[0x0b..0x0d].copy_from_slice(&512u16.to_le_bytes());
        b[0x0d] = 1;
        b[0x0e..0x10].copy_from_slice(&1u16.to_le_bytes());
        b[0x10] = 2;
        b[0x11..0x13].copy_from_slice(&224u16.to_le_bytes());
        b[0x13..0x15].copy_from_slice(&1440u16.to_le_bytes());
        b[0x15] = 0xf0;
        b[0x18..0x1a].copy_from_slice(&9u16.to_le_bytes());
        b[0x1a..0x1c].copy_from_slice(&2u16.to_le_bytes());
        b
    }

    fn hdi(path: &Path, sector: [u8; 512]) {
        let mut bytes = vec![0u8; 0x20];
        bytes[8..12].copy_from_slice(&(0x20u32).to_le_bytes());
        bytes[0x0c..0x10].copy_from_slice(&(512u32).to_le_bytes());
        bytes[0x10..0x14].copy_from_slice(&(512u32).to_le_bytes());
        bytes[0x14..0x18].copy_from_slice(&(1u32).to_le_bytes());
        bytes[0x18..0x1c].copy_from_slice(&(1u32).to_le_bytes());
        bytes[0x1c..0x20].copy_from_slice(&(1u32).to_le_bytes());
        bytes.extend_from_slice(&sector);
        fs::write(path, bytes).unwrap();
    }

    fn nhd(path: &Path, sector: [u8; 512]) {
        let mut bytes = vec![0u8; 0x200];
        bytes[..15].copy_from_slice(b"T98HDDIMAGE.R0\0");
        bytes[0x110..0x114].copy_from_slice(&(0x200u32).to_le_bytes());
        bytes[0x114..0x118].copy_from_slice(&(1u32).to_le_bytes());
        bytes[0x118..0x11a].copy_from_slice(&(1u16).to_le_bytes());
        bytes[0x11a..0x11c].copy_from_slice(&(1u16).to_le_bytes());
        bytes[0x11c..0x11e].copy_from_slice(&(512u16).to_le_bytes());
        bytes.extend_from_slice(&sector);
        fs::write(path, bytes).unwrap();
    }

    fn d88(path: &Path, sector: [u8; 512]) {
        let track_offset = 0x2b0u32;
        let mut bytes = vec![0u8; 0x2b0];
        bytes[0x1c..0x20].copy_from_slice(&track_offset.to_le_bytes());
        let mut header = [0u8; 16];
        header[0] = 0;
        header[1] = 0;
        header[2] = 1;
        header[3] = 2;
        header[4..6].copy_from_slice(&1u16.to_le_bytes());
        header[14..16].copy_from_slice(&512u16.to_le_bytes());
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&sector);
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn hdi_sector_is_delegated_to_pc98_primitive() {
        let path = temp_path("hdi");
        hdi(&path, boot(b"NEC     "));
        let result = inspect_pc98_container(&path).unwrap();
        assert_eq!(
            result.boot_sector.unwrap().confidence,
            Pc98EvidenceConfidence::Strong
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn generic_fat_never_becomes_pc98() {
        let path = temp_path("hdi");
        hdi(&path, boot(b"MSDOS5.0"));
        let result = inspect_pc98_container(&path).unwrap();
        assert_eq!(
            result.boot_sector.unwrap().confidence,
            Pc98EvidenceConfidence::GenericFat
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn d88_track_mapping_and_nhd_payload_are_bounded() {
        let d88_path = temp_path("d88");
        d88(&d88_path, boot(b"NEC     "));
        assert_eq!(
            inspect_pc98_container(&d88_path)
                .unwrap()
                .boot_sector
                .unwrap()
                .confidence,
            Pc98EvidenceConfidence::Strong
        );
        fs::remove_file(d88_path).unwrap();

        let nhd_path = temp_path("nhd");
        nhd(&nhd_path, boot(b"NEC     "));
        assert_eq!(
            inspect_pc98_container(&nhd_path)
                .unwrap()
                .boot_sector
                .unwrap()
                .confidence,
            Pc98EvidenceConfidence::Strong
        );
        fs::remove_file(nhd_path).unwrap();
    }
}
