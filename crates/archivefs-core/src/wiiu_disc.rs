//! Bounded, read-only Wii U WUD/WUX structural inspection.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const HEADER: u64 = 32;
const MAGIC0: [u8; 4] = *b"WUX0";
const MAGIC1: u32 = 0x1099_d02e;
const MAX_TABLE: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WiiUDiscFormat {
    Wud,
    Wux,
    Wua,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WiiUDiscKeyState {
    NotRequiredForContainerInspection,
    RequiredForDeeperInspection,
    AvailableLocally,
    Missing,
    Invalid,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WiiUDiscReadiness {
    ReadyForContainerInspection,
    StructurallyComplete,
    StructurallyIncomplete,
    RequiresKeysForDeeperInspection,
    UnsupportedRepresentation,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WiiUDiscIssue {
    EmptyInput,
    TruncatedHeader,
    InvalidMagic,
    InvalidSectorSize(u32),
    LogicalSizeOverflow,
    AbsurdBlockCount(u64),
    TruncatedIndexTable { expected: u64, available: u64 },
    BlockOutsideContainer { index: u32, offset: u64 },
    InvalidBlockMapping { index: u32 },
    WuaIsSeparateFormat,
    RawWudInnerStructureUnavailable,
    SplitMissingPart { index: u32 },
    SplitDuplicatePart { index: u32 },
    SplitAmbiguousSequence,
    Io(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WiiUDiscPart {
    pub path: PathBuf,
    pub index: Option<u32>,
    pub size_bytes: u64,
    pub required: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WiiUDiscStructure {
    pub detected_format: WiiUDiscFormat,
    pub total_size_bytes: u64,
    pub logical_disc_size_bytes: Option<u64>,
    pub physical_container_size_bytes: u64,
    pub compressed: bool,
    pub sector_size_bytes: Option<u32>,
    pub block_count: Option<u64>,
    pub index_table_bytes: Option<u64>,
    pub sector_array_offset: Option<u64>,
    pub referenced_block_count: Option<u64>,
    pub flags: Option<u32>,
    pub parts: Vec<WiiUDiscPart>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WiiUDiscInspection {
    pub source: PathBuf,
    pub format: WiiUDiscFormat,
    pub structure: Option<WiiUDiscStructure>,
    pub issues: Vec<WiiUDiscIssue>,
    pub structural_complete: bool,
    pub key_state: WiiUDiscKeyState,
    pub readiness: WiiUDiscReadiness,
    pub provenance: String,
}

pub fn inspect_wii_u_disc(path: &Path) -> WiiUDiscInspection {
    let source = path.to_path_buf();
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_file() && !m.file_type().is_symlink() => m,
        Ok(_) => return unavailable(source, "source is not a regular file"),
        Err(e) => return unavailable(source, &e.to_string()),
    };
    if metadata.len() == 0 {
        return inspection(
            source,
            WiiUDiscFormat::Unknown,
            None,
            vec![WiiUDiscIssue::EmptyInput],
            false,
            WiiUDiscKeyState::Unknown,
            WiiUDiscReadiness::StructurallyIncomplete,
            "bounded Wii U inspection",
        );
    }
    let ext = path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if ext == "wua" {
        return inspection(
            source,
            WiiUDiscFormat::Wua,
            None,
            vec![WiiUDiscIssue::WuaIsSeparateFormat],
            false,
            WiiUDiscKeyState::Unknown,
            WiiUDiscReadiness::UnsupportedRepresentation,
            "WUA is kept separate",
        );
    }
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) => return unavailable(source, &e.to_string()),
    };
    let mut header = [0_u8; HEADER as usize];
    let read = file.read(&mut header).unwrap_or(0);
    let wux = read >= 8
        && header[..4] == MAGIC0
        && u32::from_le_bytes(header[4..8].try_into().unwrap()) == MAGIC1;
    if wux || ext == "wux" {
        return inspect_wux(source, metadata.len(), read, &header, &mut file);
    }
    let (parts, mut issues) = split_parts(path, metadata.len());
    issues.insert(0, WiiUDiscIssue::RawWudInnerStructureUnavailable);
    let complete = issues.len() == 1;
    let size = parts.iter().map(|p| p.size_bytes).sum();
    inspection(
        source,
        WiiUDiscFormat::Wud,
        Some(WiiUDiscStructure {
            detected_format: WiiUDiscFormat::Wud,
            total_size_bytes: metadata.len(),
            logical_disc_size_bytes: Some(size),
            physical_container_size_bytes: size,
            compressed: false,
            sector_size_bytes: None,
            block_count: None,
            index_table_bytes: None,
            sector_array_offset: None,
            referenced_block_count: None,
            flags: None,
            parts,
        }),
        issues.clone(),
        complete,
        WiiUDiscKeyState::NotRequiredForContainerInspection,
        if complete {
            WiiUDiscReadiness::RequiresKeysForDeeperInspection
        } else {
            WiiUDiscReadiness::StructurallyIncomplete
        },
        "raw WUD metadata only; encrypted inner structure not guessed",
    )
}

fn inspect_wux(
    source: PathBuf,
    physical: u64,
    read: usize,
    header: &[u8; HEADER as usize],
    file: &mut File,
) -> WiiUDiscInspection {
    let mut issues = Vec::new();
    if read < HEADER as usize {
        issues.push(WiiUDiscIssue::TruncatedHeader);
        return wux_result(source, physical, None, None, None, None, issues, false);
    }
    if header[..4] != MAGIC0 || u32::from_le_bytes(header[4..8].try_into().unwrap()) != MAGIC1 {
        issues.push(WiiUDiscIssue::InvalidMagic);
        return wux_result(source, physical, None, None, None, None, issues, false);
    }
    let sector = u32::from_le_bytes(header[8..12].try_into().unwrap());
    let logical = u64::from_le_bytes(header[16..24].try_into().unwrap());
    let flags = u32::from_le_bytes(header[24..28].try_into().unwrap());
    if !(0x100..0x1000_0000).contains(&sector) {
        issues.push(WiiUDiscIssue::InvalidSectorSize(sector));
        return wux_result(
            source,
            physical,
            Some(logical),
            Some(sector),
            None,
            Some(flags),
            issues,
            false,
        );
    }
    let blocks = match logical
        .checked_add(u64::from(sector - 1))
        .and_then(|v| v.checked_div(u64::from(sector)))
    {
        Some(v) if v > 0 && v.checked_mul(4).is_some_and(|n| n <= MAX_TABLE) => v,
        Some(v) => {
            issues.push(WiiUDiscIssue::AbsurdBlockCount(v));
            return wux_result(
                source,
                physical,
                Some(logical),
                Some(sector),
                Some(v),
                Some(flags),
                issues,
                false,
            );
        }
        None => {
            issues.push(WiiUDiscIssue::LogicalSizeOverflow);
            return wux_result(
                source,
                physical,
                None,
                Some(sector),
                None,
                Some(flags),
                issues,
                false,
            );
        }
    };
    let table_bytes = blocks * 4;
    let table_end = match HEADER.checked_add(table_bytes) {
        Some(v) => v,
        None => {
            issues.push(WiiUDiscIssue::LogicalSizeOverflow);
            return wux_result(
                source,
                physical,
                None,
                Some(sector),
                Some(blocks),
                Some(flags),
                issues,
                false,
            );
        }
    };
    let data_offset = align(table_end, u64::from(sector));
    if table_end > physical || data_offset > physical {
        issues.push(WiiUDiscIssue::TruncatedIndexTable {
            expected: table_end,
            available: physical,
        });
        return wux_result(
            source,
            physical,
            Some(logical),
            Some(sector),
            Some(blocks),
            Some(flags),
            issues,
            false,
        );
    }
    let mut table = vec![0_u8; table_bytes as usize];
    if file.seek(SeekFrom::Start(HEADER)).is_err() || file.read_exact(&mut table).is_err() {
        issues.push(WiiUDiscIssue::TruncatedIndexTable {
            expected: table_end,
            available: physical,
        });
        return wux_result(
            source,
            physical,
            Some(logical),
            Some(sector),
            Some(blocks),
            Some(flags),
            issues,
            false,
        );
    }
    for bytes in table.chunks_exact(4) {
        let index = u32::from_le_bytes(bytes.try_into().unwrap());
        let offset = data_offset.checked_add(u64::from(index).saturating_mul(u64::from(sector)));
        if offset == Some(u64::MAX) {
            issues.push(WiiUDiscIssue::InvalidBlockMapping { index });
            continue;
        }
        if let Some(offset) = offset {
            if offset
                .checked_add(u64::from(sector))
                .is_none_or(|end| end > physical)
            {
                issues.push(WiiUDiscIssue::BlockOutsideContainer { index, offset });
            }
        }
    }
    wux_result(
        source,
        physical,
        Some(logical),
        Some(sector),
        Some(blocks),
        Some(flags),
        issues.clone(),
        issues.is_empty(),
    )
}

fn wux_result(
    source: PathBuf,
    physical: u64,
    logical: Option<u64>,
    sector: Option<u32>,
    blocks: Option<u64>,
    flags: Option<u32>,
    issues: Vec<WiiUDiscIssue>,
    complete: bool,
) -> WiiUDiscInspection {
    let table = blocks.map(|v| v * 4);
    let data = sector.map(|s| align(HEADER.saturating_add(table.unwrap_or(0)), u64::from(s)));
    inspection(
        source.clone(),
        WiiUDiscFormat::Wux,
        Some(WiiUDiscStructure {
            detected_format: WiiUDiscFormat::Wux,
            total_size_bytes: physical,
            logical_disc_size_bytes: logical,
            physical_container_size_bytes: physical,
            compressed: true,
            sector_size_bytes: sector,
            block_count: blocks,
            index_table_bytes: table,
            sector_array_offset: data,
            referenced_block_count: blocks,
            flags,
            parts: vec![WiiUDiscPart {
                path: source,
                index: None,
                size_bytes: physical,
                required: true,
            }],
        }),
        issues,
        complete,
        WiiUDiscKeyState::NotRequiredForContainerInspection,
        if complete {
            WiiUDiscReadiness::RequiresKeysForDeeperInspection
        } else {
            WiiUDiscReadiness::StructurallyIncomplete
        },
        "bounded WUX header/index inspection; no decompression or key access",
    )
}

fn inspection(
    source: PathBuf,
    format: WiiUDiscFormat,
    structure: Option<WiiUDiscStructure>,
    issues: Vec<WiiUDiscIssue>,
    complete: bool,
    key: WiiUDiscKeyState,
    readiness: WiiUDiscReadiness,
    provenance: &str,
) -> WiiUDiscInspection {
    WiiUDiscInspection {
        source,
        format,
        structure,
        issues,
        structural_complete: complete,
        key_state: key,
        readiness,
        provenance: provenance.into(),
    }
}
fn unavailable(source: PathBuf, detail: &str) -> WiiUDiscInspection {
    inspection(
        source,
        WiiUDiscFormat::Unknown,
        None,
        vec![WiiUDiscIssue::Io(detail.into())],
        false,
        WiiUDiscKeyState::Unknown,
        WiiUDiscReadiness::StructurallyIncomplete,
        "bounded Wii U inspection",
    )
}
fn align(v: u64, a: u64) -> u64 {
    v.checked_add(a.saturating_sub(1))
        .map(|x| x / a * a)
        .unwrap_or(u64::MAX)
}

fn split_parts(path: &Path, size: u64) -> (Vec<WiiUDiscPart>, Vec<WiiUDiscIssue>) {
    let Some((prefix, current)) = split_name(path) else {
        return (
            vec![WiiUDiscPart {
                path: path.to_path_buf(),
                index: None,
                size_bytes: size,
                required: true,
            }],
            Vec::new(),
        );
    };
    let mut found = Vec::new();
    if let Some(parent) = path.parent()
        && let Ok(entries) = std::fs::read_dir(parent)
    {
        for entry in entries.flatten() {
            let candidate = entry.path();
            if let Some((p, i)) = split_name(&candidate)
                && p == prefix
                && candidate
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("wud"))
                && let Ok(m) = std::fs::symlink_metadata(&candidate)
                && m.is_file()
            {
                found.push((i, candidate, m.len()));
            }
        }
    }
    found.sort_by_key(|v| (v.0, v.1.clone()));
    let max = found.iter().map(|v| v.0).max().unwrap_or(current);
    let mut parts = Vec::new();
    let mut issues = Vec::new();
    for i in 1..=max {
        let matches: Vec<_> = found.iter().filter(|v| v.0 == i).collect();
        if matches.is_empty() {
            issues.push(WiiUDiscIssue::SplitMissingPart { index: i });
            continue;
        }
        if matches.len() > 1 {
            issues.push(WiiUDiscIssue::SplitDuplicatePart { index: i });
        }
        let (_, p, s) = matches[0];
        parts.push(WiiUDiscPart {
            path: p.clone(),
            index: Some(i),
            size_bytes: *s,
            required: true,
        });
    }
    if parts.is_empty() {
        parts.push(WiiUDiscPart {
            path: path.to_path_buf(),
            index: Some(current),
            size_bytes: size,
            required: true,
        });
    }
    (parts, issues)
}
fn split_name(path: &Path) -> Option<(String, u32)> {
    let stem = path.file_stem()?.to_str()?.to_ascii_lowercase();
    for marker in ["_part", ".part", "-part"] {
        if let Some(pos) = stem.rfind(marker) {
            let digits = &stem[pos + marker.len()..];
            if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                return Some((stem[..pos].into(), digits.parse().ok()?));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;
    fn make_wux(path: &Path, sector: u32, logical: u64, entries: &[u32], blocks: u32) {
        let end = HEADER + entries.len() as u64 * 4;
        let data = align(end, sector as u64);
        let mut f = File::create(path).unwrap();
        let mut h = [0_u8; HEADER as usize];
        h[..4].copy_from_slice(&MAGIC0);
        h[4..8].copy_from_slice(&MAGIC1.to_le_bytes());
        h[8..12].copy_from_slice(&sector.to_le_bytes());
        h[16..24].copy_from_slice(&logical.to_le_bytes());
        f.write_all(&h).unwrap();
        for e in entries {
            f.write_all(&e.to_le_bytes()).unwrap();
        }
        f.write_all(&vec![0; (data - end) as usize]).unwrap();
        f.write_all(&vec![0; sector as usize * blocks as usize])
            .unwrap();
    }
    #[test]
    fn valid_wud_wux_and_no_mutation() {
        let d = tempdir().unwrap();
        let p = d.path().join("game.wud");
        std::fs::write(&p, [0_u8; 4096]).unwrap();
        let before = std::fs::read(&p).unwrap();
        assert_eq!(inspect_wii_u_disc(&p).format, WiiUDiscFormat::Wud);
        assert_eq!(std::fs::read(&p).unwrap(), before);
        let p = d.path().join("game.wux");
        make_wux(&p, 0x100, 0x200, &[0, 1], 2);
        let r = inspect_wii_u_disc(&p);
        assert_eq!(r.format, WiiUDiscFormat::Wux);
        assert!(r.structural_complete);
    }
    #[test]
    fn malformed_and_split_cases_fail_closed() {
        let d = tempdir().unwrap();
        let p = d.path().join("bad.wux");
        make_wux(&p, 0x100, u64::MAX, &[0], 1);
        assert!(!inspect_wii_u_disc(&p).structural_complete);
        std::fs::write(&p, b"WUX0").unwrap();
        assert!(
            inspect_wii_u_disc(&p)
                .issues
                .contains(&WiiUDiscIssue::TruncatedHeader)
        );
        let a = d.path().join("split_part1.wud");
        let c = d.path().join("split_part3.wud");
        std::fs::write(&a, [0_u8; 16]).unwrap();
        std::fs::write(&c, [0_u8; 16]).unwrap();
        assert!(
            inspect_wii_u_disc(&a)
                .issues
                .contains(&WiiUDiscIssue::SplitMissingPart { index: 2 })
        );
    }
    #[test]
    fn out_of_range_wux_block_is_reported() {
        let d = tempdir().unwrap();
        let p = d.path().join("bad.wux");
        make_wux(&p, 0x100, 0x100, &[99], 1);
        assert!(
            inspect_wii_u_disc(&p)
                .issues
                .iter()
                .any(|i| matches!(i, WiiUDiscIssue::BlockOutsideContainer { .. }))
        );
    }

    #[test]
    fn empty_wua_and_invalid_wux_are_distinct_refusals() {
        let d = tempdir().unwrap();
        let empty = d.path().join("empty.wud");
        File::create(&empty).unwrap();
        assert_eq!(
            inspect_wii_u_disc(&empty).issues,
            vec![WiiUDiscIssue::EmptyInput]
        );

        let wua = d.path().join("game.wua");
        std::fs::write(&wua, [1_u8; 32]).unwrap();
        let wua_report = inspect_wii_u_disc(&wua);
        assert_eq!(wua_report.format, WiiUDiscFormat::Wua);
        assert_eq!(
            wua_report.readiness,
            WiiUDiscReadiness::UnsupportedRepresentation
        );

        let wux = d.path().join("bad.wux");
        std::fs::write(&wux, [0_u8; HEADER as usize]).unwrap();
        assert!(
            inspect_wii_u_disc(&wux)
                .issues
                .contains(&WiiUDiscIssue::InvalidMagic)
        );
    }

    #[test]
    fn split_sequence_and_duplicate_are_reported_without_identity_claims() {
        let d = tempdir().unwrap();
        let part1 = d.path().join("title_part1.wud");
        let part2 = d.path().join("title_part2.wud");
        std::fs::write(&part1, [1_u8; 16]).unwrap();
        std::fs::write(&part2, [2_u8; 24]).unwrap();
        let complete = inspect_wii_u_disc(&part1);
        assert!(complete.structural_complete);
        assert_eq!(complete.structure.unwrap().parts.len(), 2);

        let duplicate = d.path().join("title.part1.wud");
        std::fs::write(&duplicate, [3_u8; 16]).unwrap();
        let duplicate_report = inspect_wii_u_disc(&part1);
        assert!(
            duplicate_report
                .issues
                .contains(&WiiUDiscIssue::SplitDuplicatePart { index: 1 })
        );
    }
}
