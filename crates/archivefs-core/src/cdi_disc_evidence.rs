//! Bounded, read-only Philips CD-i filesystem and startup evidence.
//!
//! The platform registry already performs the narrow CD-i identity gate: the
//! ISO9660 system identifier must be `CD-RTOS`.  This module deliberately does
//! not duplicate that resolver or infer identity from a filename.  It adds the
//! structural facts needed to explain (and audit) that identity: a coherent
//! ISO9660 volume, bounded root/path-table geometry, and conservative startup
//! evidence.  It consumes the shared [`crate::logical_media::LogicalMedia`]
//! abstraction, so plain ISO/BIN and the existing CHD adapter use one reader.

use crate::iso9660::{DiscFilesystemObservation, Iso9660Error, observe_iso9660};
use crate::logical_media::{LogicalMedia, LogicalMediaError};

pub const CD_I_SYSTEM_IDENTIFIER: &[u8] = b"CD-RTOS";
pub const ISO_VOLUME_DESCRIPTOR_BYTES: usize = 2048;
pub const MAX_VOLUME_DESCRIPTORS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdiStatus {
    Confirmed,
    Partial,
    NotCdi,
    Malformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdiIntegrity {
    Valid,
    Invalid,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdiStartupEvidence {
    BootRecordAndStartupFile,
    StartupFile,
    BootRecord,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdiSectorEvidence {
    Logical2048Only,
    RawFormNotAvailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdiDiscEvidence {
    pub status: CdiStatus,
    pub system_identifier: String,
    pub volume_identifier: String,
    pub volume_space_size: u32,
    pub logical_block_size: u32,
    pub root_extent_lba: u32,
    pub root_size_bytes: u32,
    pub path_table_lba_le: Option<u32>,
    pub path_table_lba_be: Option<u32>,
    pub startup: CdiStartupEvidence,
    pub integrity: CdiIntegrity,
    pub sector_evidence: CdiSectorEvidence,
    pub filesystem: DiscFilesystemObservation,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CdiEvidenceError {
    Media(LogicalMediaError),
    Filesystem(Iso9660Error),
    TruncatedDescriptor,
    InvalidDescriptor,
    InconsistentField(&'static str),
    OutOfBounds(&'static str),
}

impl From<LogicalMediaError> for CdiEvidenceError {
    fn from(e: LogicalMediaError) -> Self {
        Self::Media(e)
    }
}
impl From<Iso9660Error> for CdiEvidenceError {
    fn from(e: Iso9660Error) -> Self {
        Self::Filesystem(e)
    }
}

impl std::fmt::Display for CdiEvidenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Media(e) => write!(f, "{e}"),
            Self::Filesystem(e) => write!(f, "{e}"),
            Self::TruncatedDescriptor => f.write_str("truncated ISO volume descriptor"),
            Self::InvalidDescriptor => f.write_str("invalid ISO volume descriptor"),
            Self::InconsistentField(name) => write!(f, "inconsistent CD-i field: {name}"),
            Self::OutOfBounds(name) => write!(f, "CD-i {name} is out of bounds"),
        }
    }
}
impl std::error::Error for CdiEvidenceError {}

fn be_u16(bytes: &[u8]) -> u16 {
    u16::from_be_bytes([bytes[0], bytes[1]])
}
fn le_u16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}
fn both_u32(bytes: &[u8]) -> Option<u32> {
    let le = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
    let be = u32::from_be_bytes(bytes[4..8].try_into().ok()?);
    (le == be).then_some(le)
}
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
}

/// Observe CD-i structural evidence from a logical 2048-byte sector stream.
/// `CD-RTOS` plus a valid ISO9660 filesystem is the confirmed gate; startup
/// records are reported as launch evidence, never executed.
pub fn observe_cdi<M: LogicalMedia>(media: &M) -> Result<CdiDiscEvidence, CdiEvidenceError> {
    let pvd_offset = 16u64 * ISO_VOLUME_DESCRIPTOR_BYTES as u64;
    let mut pvd = [0u8; ISO_VOLUME_DESCRIPTOR_BYTES];
    media.read_at(pvd_offset, &mut pvd).map_err(|e| match e {
        LogicalMediaError::OutOfBounds { .. } => CdiEvidenceError::TruncatedDescriptor,
        other => CdiEvidenceError::Media(other),
    })?;
    if &pvd[1..6] != b"CD001" || pvd[0] != 1 {
        return Err(CdiEvidenceError::InvalidDescriptor);
    }
    let system_identifier = text(&pvd[8..40]);
    let is_cdi = system_identifier.starts_with("CD-RTOS");
    let filesystem = observe_iso9660(media)?;
    let volume_space_size =
        both_u32(&pvd[80..88]).ok_or(CdiEvidenceError::InconsistentField("volume_space_size"))?;
    let logical_block_size = le_u16(&pvd[128..130]);
    if logical_block_size != 2048 || be_u16(&pvd[130..132]) != logical_block_size {
        return Err(CdiEvidenceError::InvalidDescriptor);
    }
    let root = &pvd[156..190];
    let root_extent_lba =
        both_u32(&root[2..10]).ok_or(CdiEvidenceError::InconsistentField("root_extent"))?;
    let root_size_bytes =
        both_u32(&root[10..18]).ok_or(CdiEvidenceError::InconsistentField("root_size"))?;
    let media_blocks = media.len() / logical_block_size as u64;
    if root_extent_lba as u64 >= media_blocks
        || (root_extent_lba as u64).saturating_add((root_size_bytes as u64 + 2047) / 2048)
            > media_blocks
    {
        return Err(CdiEvidenceError::OutOfBounds("root directory"));
    }
    let path_table_lba_le = (u32::from_le_bytes(pvd[132..136].try_into().unwrap()) != 0)
        .then(|| u32::from_le_bytes(pvd[132..136].try_into().unwrap()));
    let path_table_lba_be = (u32::from_be_bytes(pvd[140..144].try_into().unwrap()) != 0)
        .then(|| u32::from_be_bytes(pvd[140..144].try_into().unwrap()));
    let mut warnings = Vec::new();
    if volume_space_size as u64 > media_blocks {
        warnings.push("volume space size exceeds logical media".into());
    }
    for (name, lba) in [
        ("little-endian path table", path_table_lba_le),
        ("big-endian path table", path_table_lba_be),
    ] {
        if let Some(lba) = lba {
            if lba as u64 >= media_blocks {
                warnings.push(format!("{name} is out of bounds"));
            }
        }
    }
    let has_startup = filesystem
        .root_entries
        .iter()
        .any(|e| e.comparison_name == "STARTUP" || e.comparison_name.ends_with(".APP"));
    let boot_record = find_cdi_boot_record(media)?;
    let startup = match (boot_record, has_startup) {
        (true, true) => CdiStartupEvidence::BootRecordAndStartupFile,
        (false, true) => CdiStartupEvidence::StartupFile,
        (true, false) => CdiStartupEvidence::BootRecord,
        (false, false) => CdiStartupEvidence::None,
    };
    if !is_cdi {
        warnings.push("ISO9660 volume is not CD-RTOS".into());
    }
    if path_table_lba_le.is_none() && path_table_lba_be.is_none() {
        warnings.push("no path table location recorded".into());
    }
    let invalid_geometry = warnings.iter().any(|w| {
        w.contains("out of bounds") || w.contains("exceeds logical media")
    });
    Ok(CdiDiscEvidence {
        status: if !is_cdi {
            CdiStatus::NotCdi
        } else if invalid_geometry {
            CdiStatus::Malformed
        } else {
            CdiStatus::Confirmed
        },
        system_identifier,
        volume_identifier: filesystem.volume_identifier.clone(),
        volume_space_size,
        logical_block_size: logical_block_size as u32,
        root_extent_lba,
        root_size_bytes,
        path_table_lba_le,
        path_table_lba_be,
        startup,
        integrity: if invalid_geometry {
            CdiIntegrity::Invalid
        } else {
            CdiIntegrity::Valid
        },
        sector_evidence: CdiSectorEvidence::Logical2048Only,
        filesystem,
        warnings,
    })
}

fn find_cdi_boot_record<M: LogicalMedia>(media: &M) -> Result<bool, CdiEvidenceError> {
    let mut descriptor = [0u8; ISO_VOLUME_DESCRIPTOR_BYTES];
    for index in 0..MAX_VOLUME_DESCRIPTORS {
        media.read_at((16 + index) as u64 * 2048, &mut descriptor)?;
        if &descriptor[1..6] != b"CD001" {
            return Ok(false);
        }
        if descriptor[0] == 0 {
            return Ok(descriptor[7..14].starts_with(b"CD-I"));
        }
        if descriptor[0] == 255 {
            break;
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logical_media::SliceMedia;
    fn both32(v: u32) -> [u8; 8] {
        let mut b = [0; 8];
        b[..4].copy_from_slice(&v.to_le_bytes());
        b[4..].copy_from_slice(&v.to_be_bytes());
        b
    }
    fn record(name: &[u8], extent: u32, size: u32, dir: bool) -> Vec<u8> {
        let n = 33 + name.len();
        let mut r = vec![0; n];
        r[0] = n as u8;
        r[2..10].copy_from_slice(&both32(extent));
        r[10..18].copy_from_slice(&both32(size));
        r[25] = if dir { 2 } else { 0 };
        r[32] = name.len() as u8;
        r[33..].copy_from_slice(name);
        r
    }
    fn fixture(system: &[u8]) -> Vec<u8> {
        let mut d = vec![0; 20 * 2048];
        let p = &mut d[16 * 2048..17 * 2048];
        p[0] = 1;
        p[1..6].copy_from_slice(b"CD001");
        p[6] = 1;
        p[8..8 + system.len()].copy_from_slice(system);
        p[40..72].fill(b' ');
        p[40..44].copy_from_slice(b"TEST");
        p[80..88].copy_from_slice(&both32(20));
        p[128..130].copy_from_slice(&2048u16.to_le_bytes());
        p[130..132].copy_from_slice(&2048u16.to_be_bytes());
        p[132..136].copy_from_slice(&18u32.to_le_bytes());
        p[140..144].copy_from_slice(&18u32.to_be_bytes());
        p[156] = 34;
        p[158..166].copy_from_slice(&both32(18));
        p[166..174].copy_from_slice(&both32(2048));
        let t = &mut d[17 * 2048..18 * 2048];
        let root_record = record(&[0], 18, 2048, true);
        t[..root_record.len()].copy_from_slice(&root_record);
        let startup_record = record(b"STARTUP", 19, 10, false);
        let startup_end = root_record.len() + startup_record.len();
        t[root_record.len()..startup_end].copy_from_slice(&startup_record);
        d
    }
    #[test]
    fn cdi_requires_cd_rtos_and_reads_root() {
        let e = observe_cdi(&SliceMedia(&fixture(b"CD-RTOS"))).unwrap();
        assert_eq!(e.status, CdiStatus::Confirmed);
        assert_eq!(e.startup, CdiStartupEvidence::StartupFile);
    }
    #[test]
    fn generic_iso_is_not_cdi() {
        let e = observe_cdi(&SliceMedia(&fixture(b"GENERIC"))).unwrap();
        assert_eq!(e.status, CdiStatus::NotCdi);
    }
    #[test]
    fn truncation_fails_soft() {
        assert!(matches!(
            observe_cdi(&SliceMedia(&[0; 100])),
            Err(CdiEvidenceError::TruncatedDescriptor)
        ));
    }
}
