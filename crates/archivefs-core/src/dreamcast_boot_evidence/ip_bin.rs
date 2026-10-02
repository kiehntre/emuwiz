//! Complete 32 KiB IP.BIN tooling, extending the existing metadata inspector.
//! See docs/research/DREAMCAST_IP_BIN_TOOLING.md for verified format references.
//! Bootstrap/code bytes are opaque and preserved. No image reader or writer.

use super::*;
use crate::dreamcast_patch_readiness::safe_relative_path;
use crate::optical_patch_tree::{digest, refuse};
use crate::patch_manager::{DestinationRootState, validate_destination_root};
use crate::safe_read::{TrustedRoots, open_bounded_read};
use std::collections::BTreeSet;
use std::fs::{self, File, Metadata};
use std::io::{self, Read, Write};
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

pub const IP_BIN_BYTES: usize = 0x8000;
pub const KNOWN_PERIPHERAL_MASK: u32 = 0x0FFF_FF11;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IpBinStatus {
    Valid,
    Truncated,
    Malformed,
    UnsupportedVariant,
    SuspiciousButParseable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamcastRegion {
    Japan,
    UsaCanada,
    Europe,
}
impl DreamcastRegion {
    fn slot(self) -> usize {
        match self {
            Self::Japan => 0,
            Self::UsaCanada => 1,
            Self::Europe => 2,
        }
    }
    fn symbol(self) -> u8 {
        b"JUE"[self.slot()]
    }
    fn protection_text(self) -> &'static [u8] {
        match self {
            Self::Japan => b"For JAPAN,TAIWAN,PHILIPINES.",
            Self::UsaCanada => b"For USA and CANADA.         ",
            Self::Europe => b"For EUROPE.                 ",
        }
    }
    fn protection_range(self) -> Range<usize> {
        let start = 0x3704 + self.slot() * 32;
        start..start + 28
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamcastRegionSymbol {
    Blank,
    Known(DreamcastRegion),
    Unknown(u8),
}

/// Documented bits only. A declaration does not prove runtime support.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum DreamcastPeripheral {
    WindowsCe = 0,
    Vga = 4,
    OtherExpansions = 8,
    PuruPuruPack = 9,
    Microphone = 10,
    MemoryCard = 11,
    StartABDirections = 12,
    CButton = 13,
    DButton = 14,
    XButton = 15,
    YButton = 16,
    ZButton = 17,
    ExpandedDirections = 18,
    AnalogRTrigger = 19,
    AnalogLTrigger = 20,
    AnalogHorizontal = 21,
    AnalogVertical = 22,
    ExpandedAnalogHorizontal = 23,
    ExpandedAnalogVertical = 24,
    Gun = 25,
    Keyboard = 26,
    Mouse = 27,
}
const PERIPHERAL_CAPABILITIES: [DreamcastPeripheral; 22] = [
    DreamcastPeripheral::WindowsCe,
    DreamcastPeripheral::Vga,
    DreamcastPeripheral::OtherExpansions,
    DreamcastPeripheral::PuruPuruPack,
    DreamcastPeripheral::Microphone,
    DreamcastPeripheral::MemoryCard,
    DreamcastPeripheral::StartABDirections,
    DreamcastPeripheral::CButton,
    DreamcastPeripheral::DButton,
    DreamcastPeripheral::XButton,
    DreamcastPeripheral::YButton,
    DreamcastPeripheral::ZButton,
    DreamcastPeripheral::ExpandedDirections,
    DreamcastPeripheral::AnalogRTrigger,
    DreamcastPeripheral::AnalogLTrigger,
    DreamcastPeripheral::AnalogHorizontal,
    DreamcastPeripheral::AnalogVertical,
    DreamcastPeripheral::ExpandedAnalogHorizontal,
    DreamcastPeripheral::ExpandedAnalogVertical,
    DreamcastPeripheral::Gun,
    DreamcastPeripheral::Keyboard,
    DreamcastPeripheral::Mouse,
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DreamcastPeripheralDeclaration {
    pub bits: Option<u32>,
    pub declared: Vec<DreamcastPeripheral>,
    pub unknown_bits: u32,
    pub documented_encoding: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamcastMediaType {
    GdRom,
    CdRom,
    MilCd,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DreamcastDeviceInformation {
    pub media: DreamcastMediaType,
    pub disc_number: u8,
    pub disc_count: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IpBinProductCrcStatus {
    Matched,
    PlaceholderZero,
    Mismatch,
    Malformed,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IpBinProductCrc {
    pub stored: Option<u16>,
    pub computed: u16,
    pub status: IpBinProductCrcStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DreamcastIpBin {
    pub metadata: DreamcastIpBinInspection,
    pub regions: [DreamcastRegionSymbol; 8],
    pub peripherals: DreamcastPeripheralDeclaration,
    pub device: Option<DreamcastDeviceInformation>,
    pub product_crc: IpBinProductCrc,
    raw_bytes: Vec<u8>,
}
impl DreamcastIpBin {
    /// Rendering without edits is this exact preserved buffer, never a rebuild.
    pub fn raw_bytes(&self) -> &[u8] {
        &self.raw_bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IpBinInspection {
    pub status: IpBinStatus,
    pub ip_bin: Option<DreamcastIpBin>,
    pub issues: Vec<String>,
}

/// CRC-16/IBM-3740 (CCITT-FALSE): poly 0x1021, init 0xFFFF, no reflection
/// or final XOR. IP.BIN stores it as four ASCII hex digits, covering 0x40..0x50.
pub fn product_crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for &byte in bytes {
        crc ^= u16::from(byte) << 8;
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

fn check_text(field: &mut DreamcastIpBinField, label: &str, issues: &mut Vec<String>) -> bool {
    // NUL padding is observable but not documented as standard IP.BIN padding.
    let value = &field.raw_bytes;
    let end = value
        .iter()
        .rposition(|b| !matches!(b, b' ' | 0))
        .map_or(0, |i| i + 1);
    let valid = value[..end].iter().all(|b| (0x20..=0x7E).contains(b))
        && value[end..].iter().all(|b| matches!(b, b' ' | 0));
    field.value = String::from_utf8_lossy(&value[..end]).into_owned();
    if !valid {
        field.validity = IpBinFieldValidity::Invalid;
        let issue = format!("{label}: non-ASCII or embedded control bytes");
        field.warnings.push(issue.clone());
        issues.push(issue);
    } else if value.contains(&0) {
        field.validity = IpBinFieldValidity::Warning;
        let issue = format!("{label}: nonstandard NUL padding preserved");
        field.warnings.push(issue.clone());
        issues.push(issue);
    }
    valid
}

fn valid_date(value: &str) -> bool {
    if value.len() != 8 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let year = value[..4].parse::<u32>().unwrap();
    let month = value[4..6].parse::<usize>().unwrap();
    let day = value[6..].parse::<u32>().unwrap();
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    year != 0 && (1..=12).contains(&month) && day != 0 && day <= days[month - 1]
}

fn valid_boot_filename(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= BOOT_FILENAME.1
        && value.bytes().all(|b| (0x21..=0x7E).contains(&b))
        && !value.contains(['/', '\\', ':'])
        && !value.contains("..")
        && safe_relative_path(value).is_ok()
}

/// Content-based, bounded inspection. Size and signature do not prove that
/// opaque licence/bootstrap code will run on hardware. At most 32 KiB retained.
pub fn inspect_ip_bin(bytes: &[u8]) -> IpBinInspection {
    let Ok(mut metadata) = inspect_ip_bin_meta(bytes) else {
        return IpBinInspection {
            status: IpBinStatus::Truncated,
            ip_bin: None,
            issues: vec!["fewer than 256 metadata bytes".into()],
        };
    };
    let mut issues = Vec::new();
    let mut malformed = false;
    for (field, label) in [
        (&mut metadata.hardware_id, "hardware ID"),
        (&mut metadata.maker_id, "maker ID"),
        (&mut metadata.device_information, "device information"),
        (&mut metadata.product_number, "product number"),
        (&mut metadata.product_version, "product version"),
        (&mut metadata.release_date, "release date"),
        (&mut metadata.boot_filename, "boot filename"),
        (&mut metadata.software_maker_name, "company"),
        (&mut metadata.software_title, "title"),
    ] {
        malformed |= !check_text(field, label, &mut issues);
    }
    let hardware = &bytes[..16];
    let mut unsupported = hardware != b"SEGA SEGAKATANA " && hardware.starts_with(b"SEGA ");
    if unsupported {
        metadata.hardware_id.validity = IpBinFieldValidity::Unknown;
    }
    if hardware != b"SEGA SEGAKATANA " && !unsupported {
        malformed = true;
        metadata.hardware_id.validity = IpBinFieldValidity::Invalid;
        issues.push("malformed Dreamcast hardware identifier".into());
    }
    if &bytes[0x10..0x20] != b"SEGA ENTERPRISES" {
        malformed = true;
        metadata.maker_id.validity = IpBinFieldValidity::Invalid;
        issues.push("maker identifier must be SEGA ENTERPRISES".into());
    }
    let regions = std::array::from_fn(|slot| match bytes[0x30 + slot] {
        b' ' => DreamcastRegionSymbol::Blank,
        symbol if slot < 3 && symbol == b"JUE"[slot] => DreamcastRegionSymbol::Known(
            [
                DreamcastRegion::Japan,
                DreamcastRegion::UsaCanada,
                DreamcastRegion::Europe,
            ][slot],
        ),
        symbol => {
            let issue = format!("unknown area symbol 0x{symbol:02X} at slot {slot}");
            metadata.area_symbols.warnings.push(issue.clone());
            issues.push(issue);
            if !symbol.is_ascii_alphabetic() {
                malformed = true;
                metadata.area_symbols.validity = IpBinFieldValidity::Invalid;
            } else if metadata.area_symbols.validity != IpBinFieldValidity::Invalid {
                metadata.area_symbols.validity = IpBinFieldValidity::Warning;
            }
            DreamcastRegionSymbol::Unknown(symbol)
        }
    });
    let raw_peripherals = &bytes[0x38..0x40];
    let documented_encoding =
        raw_peripherals[..7].iter().all(u8::is_ascii_hexdigit) && raw_peripherals[7] == b' ';
    let bits = std::str::from_utf8(raw_peripherals)
        .ok()
        .and_then(|s| u32::from_str_radix(s.trim_end_matches(' '), 16).ok());
    let unknown_bits = bits.map_or(0, |b| b & !KNOWN_PERIPHERAL_MASK);
    if bits.is_none() {
        malformed = true;
        metadata.peripheral_flags.validity = IpBinFieldValidity::Invalid;
        issues.push("peripherals must be hexadecimal".into());
    } else if !documented_encoding || unknown_bits != 0 {
        metadata.peripheral_flags.validity = IpBinFieldValidity::Warning;
        issues.push(format!(
            "nonstandard peripheral encoding/reserved bits 0x{unknown_bits:08X} preserved"
        ));
    }
    let peripherals = DreamcastPeripheralDeclaration {
        bits,
        declared: PERIPHERAL_CAPABILITIES
            .into_iter()
            .filter(|p| bits.is_some_and(|b| b & (1 << *p as u8) != 0))
            .collect(),
        unknown_bits,
        documented_encoding,
    };
    let stored = std::str::from_utf8(&bytes[0x20..0x24])
        .ok()
        .filter(|s| s.bytes().all(|b| b.is_ascii_hexdigit()))
        .and_then(|s| u16::from_str_radix(s, 16).ok());
    let computed = product_crc16(&bytes[0x40..0x50]);
    let crc_status = match stored {
        Some(crc) if crc == computed => IpBinProductCrcStatus::Matched,
        Some(0) => IpBinProductCrcStatus::PlaceholderZero,
        Some(_) => IpBinProductCrcStatus::Mismatch,
        None => IpBinProductCrcStatus::Malformed,
    };
    metadata.checksum_status = match crc_status {
        IpBinProductCrcStatus::Matched => IpBinChecksumStatus::ProductCrcMatched,
        IpBinProductCrcStatus::PlaceholderZero => IpBinChecksumStatus::ProductCrcPlaceholderZero,
        IpBinProductCrcStatus::Mismatch => IpBinChecksumStatus::ProductCrcMismatch,
        IpBinProductCrcStatus::Malformed => IpBinChecksumStatus::MalformedProductCrc,
    };
    if crc_status == IpBinProductCrcStatus::Malformed {
        malformed = true;
        metadata.device_information.validity = IpBinFieldValidity::Invalid;
        issues.push("product CRC is not four ASCII hexadecimal digits".into());
    } else if crc_status != IpBinProductCrcStatus::Matched {
        metadata.device_information.validity = IpBinFieldValidity::Warning;
        issues.push("product CRC mismatch/zero placeholder; not a bootstrap checksum".into());
    }
    let device = (|| {
        let text = std::str::from_utf8(&bytes[0x25..0x30])
            .ok()?
            .trim_end_matches(' ');
        let (media, number) = match text.get(..6)? {
            "GD-ROM" => (DreamcastMediaType::GdRom, &text[6..]),
            "CD-ROM" => (DreamcastMediaType::CdRom, &text[6..]),
            "MIL CD" => (DreamcastMediaType::MilCd, &text[6..]),
            _ => {
                unsupported = true;
                return None;
            }
        };
        let (disc, count) = number.split_once('/')?;
        if !disc.bytes().all(|b| b.is_ascii_digit()) || !count.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let disc_number = disc.parse::<u8>().ok()?;
        let disc_count = count.parse::<u8>().ok()?;
        (disc_number > 0 && disc_number <= disc_count).then_some(DreamcastDeviceInformation {
            media,
            disc_number,
            disc_count,
        })
    })();
    if bytes[0x24] != b' ' || (device.is_none() && !unsupported) {
        malformed = true;
        metadata.device_information.validity = IpBinFieldValidity::Invalid;
        issues.push("malformed media/disc-number information".into());
    }
    let version = metadata.product_version.value.as_bytes();
    for (valid, field, label) in [
        (
            validate_product_number(&metadata.product_number.value).0 == IpBinFieldValidity::Valid,
            &mut metadata.product_number,
            "product number",
        ),
        (
            version.len() == 6
                && version[0] == b'V'
                && version[1].is_ascii_digit()
                && version[2] == b'.'
                && version[3..].iter().all(u8::is_ascii_digit),
            &mut metadata.product_version,
            "version must be Vx.yyy",
        ),
        (
            valid_date(&metadata.release_date.value),
            &mut metadata.release_date,
            "Gregorian YYYYMMDD release date",
        ),
        (
            valid_boot_filename(&metadata.boot_filename.value),
            &mut metadata.boot_filename,
            "boot filename must be a safe root filename",
        ),
    ] {
        if !valid {
            malformed = true;
            field.validity = IpBinFieldValidity::Invalid;
            issues.push(format!("invalid {label}"));
        }
    }
    if bytes.len() >= IP_BIN_BYTES {
        for symbol in regions {
            if let DreamcastRegionSymbol::Known(region) = symbol
                && &bytes[region.protection_range()] != region.protection_text()
            {
                issues.push(format!(
                    "{region:?} enabled without its documented area-protection text"
                ));
            }
        }
    }
    let status = if bytes.len() < IP_BIN_BYTES {
        issues.push("complete IP.BIN requires 32768 bytes".into());
        IpBinStatus::Truncated
    } else if malformed {
        IpBinStatus::Malformed
    } else if unsupported || bytes.len() != IP_BIN_BYTES {
        issues.push("unsupported hardware/media/length variant; edits refused".into());
        IpBinStatus::UnsupportedVariant
    } else if !issues.is_empty() {
        IpBinStatus::SuspiciousButParseable
    } else {
        IpBinStatus::Valid
    };
    metadata.validation_status = if malformed {
        DreamcastIpBinValidationStatus::Invalid
    } else if issues.is_empty() {
        DreamcastIpBinValidationStatus::Valid
    } else {
        DreamcastIpBinValidationStatus::ValidWithWarnings
    };
    metadata.warnings = issues.clone();
    IpBinInspection {
        status,
        issues,
        ip_bin: Some(DreamcastIpBin {
            metadata,
            regions,
            peripherals,
            device,
            product_crc: IpBinProductCrc {
                stored,
                computed,
                status: crc_status,
            },
            raw_bytes: bytes[..bytes.len().min(IP_BIN_BYTES)].to_vec(),
        }),
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IpBinSourceEvidence {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
    pub modified: SystemTime,
    #[cfg(unix)]
    pub device: u64,
    #[cfg(unix)]
    pub inode: u64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IpBinFileInspection {
    pub size_bytes: u64,
    /// Absent for oversized/unsupported input: a whole-disc hash is never read.
    pub source: Option<IpBinSourceEvidence>,
    pub inspection: IpBinInspection,
}

fn same_file(before: &Metadata, after: &Metadata) -> io::Result<bool> {
    let same = before.is_file()
        && after.is_file()
        && before.len() == after.len()
        && before.modified()? == after.modified()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(same
            && (
                before.dev(),
                before.ino(),
                before.ctime(),
                before.ctime_nsec(),
            ) == (after.dev(), after.ino(), after.ctime(), after.ctime_nsec()))
    }
    #[cfg(not(unix))]
    {
        Ok(same)
    }
}

/// Reads at most 32769 bytes, irrespective of extension or source size.
pub fn inspect_ip_bin_file(path: &Path) -> io::Result<IpBinFileInspection> {
    let safe = open_bounded_read(path, &TrustedRoots::none()).map_err(|e| refuse(e.detail()))?;
    let mut file = safe.into_file();
    let before = file.metadata()?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take((IP_BIN_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != before.len().min((IP_BIN_BYTES + 1) as u64)
        || !same_file(&before, &file.metadata()?)?
        || !same_file(&before, &fs::symlink_metadata(path)?)?
    {
        return Err(refuse("IP.BIN changed during bounded inspection"));
    }
    let source = if before.len() <= IP_BIN_BYTES as u64 {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Some(IpBinSourceEvidence {
            path: path.to_owned(),
            size_bytes: before.len(),
            sha256: digest(&bytes),
            modified: before.modified()?,
            #[cfg(unix)]
            device: before.dev(),
            #[cfg(unix)]
            inode: before.ino(),
        })
    } else {
        None
    };
    Ok(IpBinFileInspection {
        size_bytes: before.len(),
        source,
        inspection: inspect_ip_bin(&bytes),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IpBinTextField {
    ProductNumber,
    ProductVersion,
    ReleaseDate,
    BootFilename,
    Company,
    Title,
}
impl IpBinTextField {
    fn range(self) -> Range<usize> {
        let (offset, length) = match self {
            Self::ProductNumber => PRODUCT_NUMBER,
            Self::ProductVersion => PRODUCT_VERSION,
            Self::ReleaseDate => RELEASE_DATE,
            Self::BootFilename => BOOT_FILENAME,
            Self::Company => SOFTWARE_MAKER_NAME,
            Self::Title => SOFTWARE_NAME,
        };
        offset..offset + length
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IpBinEdit {
    Text {
        field: IpBinTextField,
        value: String,
    },
    Region {
        region: DreamcastRegion,
        enabled: bool,
    },
    Peripheral {
        capability: DreamcastPeripheral,
        declared: bool,
    },
    RecalculateProductCrc,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IpBinFieldChange {
    pub field: String,
    pub original_value: String,
    pub proposed_value: String,
    pub byte_range: Range<usize>,
    pub original_bytes: Vec<u8>,
    pub proposed_bytes: Vec<u8>,
    pub validation: IpBinFieldValidity,
    pub integrity_bytes: bool,
}
#[derive(Clone, Debug)]
pub struct IpBinPreview {
    source: IpBinSourceEvidence,
    original: IpBinInspection,
    expected: IpBinInspection,
    changes: Vec<IpBinFieldChange>,
    expected_sha256: String,
}
impl IpBinPreview {
    pub fn source(&self) -> &IpBinSourceEvidence {
        &self.source
    }
    pub fn changes(&self) -> &[IpBinFieldChange] {
        &self.changes
    }
    pub fn expected(&self) -> &IpBinInspection {
        &self.expected
    }
    pub fn expected_sha256(&self) -> &str {
        &self.expected_sha256
    }
    pub fn integrity_bytes_change(&self) -> bool {
        self.changes.iter().any(|c| c.integrity_bytes)
    }
    pub(crate) fn expected_bytes(&self) -> &[u8] {
        self.expected.ip_bin.as_ref().unwrap().raw_bytes()
    }
    pub(crate) fn verify_source(&self) -> io::Result<()> {
        let current = inspect_ip_bin_file(&self.source.path)
            .map_err(|e| refuse(format!("STALE PLAN: {e}")))?;
        if current.source.as_ref() != Some(&self.source) || current.inspection != self.original {
            return Err(refuse(
                "STALE PLAN: IP.BIN identity/size/hash/original fields changed",
            ));
        }
        Ok(())
    }
    pub(crate) fn verify_output(&self, path: &Path) -> io::Result<()> {
        let current = inspect_ip_bin_file(path)?;
        if current.inspection != self.expected
            || current.source.as_ref().map(|s| s.sha256.as_str()) != Some(&self.expected_sha256)
            || !matches!(
                current.inspection.status,
                IpBinStatus::Valid | IpBinStatus::SuspiciousButParseable
            )
        {
            return Err(refuse(
                "staged IP.BIN failed semantic/byte/integrity verification",
            ));
        }
        Ok(())
    }
}

fn record_change(
    bytes: &mut [u8],
    changes: &mut Vec<IpBinFieldChange>,
    field: String,
    range: Range<usize>,
    replacement: &[u8],
    integrity_bytes: bool,
) {
    let original = &bytes[range.clone()];
    if original == replacement {
        return;
    }
    changes.push(IpBinFieldChange {
        field,
        original_value: String::from_utf8_lossy(original)
            .trim_end_matches([' ', '\0'])
            .into(),
        proposed_value: String::from_utf8_lossy(replacement)
            .trim_end_matches(' ')
            .into(),
        byte_range: range.clone(),
        original_bytes: original.to_vec(),
        proposed_bytes: replacement.to_vec(),
        validation: IpBinFieldValidity::Valid,
        integrity_bytes,
    });
    bytes[range].copy_from_slice(replacement);
}

/// Pure: no filesystem calls. Invalid/overlong/non-ASCII edits return a refusal.
/// Unedited unknown symbols, reserved bits, padding and code remain byte-identical.
pub fn preview_ip_bin_edits(
    source: &IpBinFileInspection,
    edits: &[IpBinEdit],
) -> io::Result<IpBinPreview> {
    if !matches!(
        source.inspection.status,
        IpBinStatus::Valid | IpBinStatus::SuspiciousButParseable
    ) {
        return Err(refuse(
            "edits require a complete supported, structurally parseable IP.BIN",
        ));
    }
    let evidence = source
        .source
        .clone()
        .ok_or_else(|| refuse("source evidence required"))?;
    let ip = source
        .inspection
        .ip_bin
        .as_ref()
        .ok_or_else(|| refuse("parsed IP.BIN snapshot required"))?;
    if ip.raw_bytes.len() != IP_BIN_BYTES
        || evidence.size_bytes != IP_BIN_BYTES as u64
        || evidence.sha256 != digest(ip.raw_bytes())
    {
        return Err(refuse("incomplete or inconsistent IP.BIN source evidence"));
    }
    let mut bytes = ip.raw_bytes.clone();
    let mut changes = Vec::new();
    let mut fields = BTreeSet::new();
    let mut repair_crc = false;
    let mut peripheral_bits = ip.peripherals.bits;
    for edit in edits {
        match edit {
            IpBinEdit::Text { field, value } => {
                let range = field.range();
                if !fields.insert(range.start) {
                    return Err(refuse("duplicate field edit"));
                }
                if value.len() > range.len() || !value.bytes().all(|b| (0x20..=0x7E).contains(&b)) {
                    return Err(refuse(
                        "replacement exceeds fixed byte width or contains non-ASCII/control bytes",
                    ));
                }
                // An explicit semantic no-op must preserve even unusual padding.
                if value
                    == String::from_utf8_lossy(&bytes[range.clone()]).trim_end_matches([' ', '\0'])
                {
                    continue;
                }
                let mut replacement = vec![b' '; range.len()];
                replacement[..value.len()].copy_from_slice(value.as_bytes());
                record_change(
                    &mut bytes,
                    &mut changes,
                    format!("{field:?}"),
                    range,
                    &replacement,
                    false,
                );
            }
            IpBinEdit::Region { region, enabled } => {
                let offset = 0x30 + region.slot();
                if !fields.insert(offset) {
                    return Err(refuse("duplicate region edit"));
                }
                if !matches!(bytes[offset], b' ') && bytes[offset] != region.symbol() {
                    return Err(refuse("region edit would overwrite an unknown symbol"));
                }
                let replacement = if *enabled { region.symbol() } else { b' ' };
                if bytes[offset] != replacement
                    && *enabled
                    && &bytes[region.protection_range()] != region.protection_text()
                {
                    return Err(refuse(
                        "region cannot be enabled: documented area-protection text is absent; bootstrap edits unsupported",
                    ));
                }
                record_change(
                    &mut bytes,
                    &mut changes,
                    format!("Region {region:?}"),
                    offset..offset + 1,
                    &[replacement],
                    false,
                );
            }
            IpBinEdit::Peripheral {
                capability,
                declared,
            } => {
                let mask = 1u32 << *capability as u8;
                if !fields.insert(0x100 + *capability as usize) {
                    return Err(refuse("duplicate peripheral edit"));
                }
                if !ip.peripherals.documented_encoding {
                    return Err(refuse(
                        "peripheral edits require documented seven-digit, space-padded encoding",
                    ));
                }
                let original = peripheral_bits.unwrap();
                let proposed = if *declared {
                    original | mask
                } else {
                    original & !mask
                };
                peripheral_bits = Some(proposed);
            }
            IpBinEdit::RecalculateProductCrc => {
                if repair_crc {
                    return Err(refuse("duplicate CRC repair"));
                }
                repair_crc = true;
            }
        }
    }
    if peripheral_bits != ip.peripherals.bits {
        let replacement = format!("{:07X} ", peripheral_bits.unwrap());
        record_change(
            &mut bytes,
            &mut changes,
            "Peripherals".into(),
            0x38..0x40,
            replacement.as_bytes(),
            false,
        );
    }
    if repair_crc || bytes[0x40..0x50] != ip.raw_bytes[0x40..0x50] {
        let replacement = format!("{:04X}", product_crc16(&bytes[0x40..0x50]));
        record_change(
            &mut bytes,
            &mut changes,
            "Product CRC".into(),
            0x20..0x24,
            replacement.as_bytes(),
            true,
        );
    }
    let expected = inspect_ip_bin(&bytes);
    if !matches!(
        expected.status,
        IpBinStatus::Valid | IpBinStatus::SuspiciousButParseable
    ) {
        return Err(refuse(format!(
            "edited IP.BIN failed validation: {:?}",
            expected.issues
        )));
    }
    Ok(IpBinPreview {
        source: evidence,
        original: source.inspection.clone(),
        expected,
        changes,
        expected_sha256: digest(&bytes),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OutputParent {
    path: PathBuf,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}
impl OutputParent {
    fn inspect(path: &Path) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(refuse("absolute output parent required"));
        }
        let root = validate_destination_root(path).map_err(refuse)?;
        if root.state() != DestinationRootState::ExistingDirectory || root.path() != path {
            return Err(refuse("existing confined output parent required"));
        }
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        let m = fs::symlink_metadata(path)?;
        Ok(Self {
            path: path.to_owned(),
            #[cfg(unix)]
            device: m.dev(),
            #[cfg(unix)]
            inode: m.ino(),
        })
    }
}
fn absent(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
        Ok(_) => Err(refuse("destination already exists; no-clobber")),
    }
}

/// Reviewed loose-file plan. Tree edits use the existing DCP tree transaction.
#[derive(Clone, Debug)]
pub struct IpBinFilePlan {
    preview: IpBinPreview,
    destination: PathBuf,
    parent: OutputParent,
}
pub fn review_ip_bin_file(preview: &IpBinPreview, destination: &Path) -> io::Result<IpBinFilePlan> {
    preview.verify_source()?;
    if destination.file_name().is_none()
        || !destination.is_absolute()
        || destination
            .components()
            .any(|p| !matches!(p, Component::RootDir | Component::Normal(_)))
    {
        return Err(refuse("absolute confined output filename required"));
    }
    let parent = OutputParent::inspect(
        destination
            .parent()
            .ok_or_else(|| refuse("output parent required"))?,
    )?;
    absent(destination)?;
    Ok(IpBinFilePlan {
        preview: preview.clone(),
        destination: destination.to_owned(),
        parent,
    })
}
impl IpBinFilePlan {
    /// Stage -> reparse/verify -> recheck source -> atomic no-replace rename.
    /// The source is never opened for writing. Failed publication cannot clobber.
    pub fn apply(&self) -> io::Result<PathBuf> {
        self.preview.verify_source()?;
        if OutputParent::inspect(&self.parent.path)? != self.parent {
            return Err(refuse("output parent changed after review"));
        }
        absent(&self.destination)?;
        let mut staged = tempfile::NamedTempFile::new_in(&self.parent.path)?;
        staged.write_all(self.preview.expected_bytes())?;
        staged.as_file().sync_all()?;
        self.preview.verify_output(staged.path())?;
        self.preview.verify_source()?;
        if OutputParent::inspect(&self.parent.path)? != self.parent {
            return Err(refuse("output parent changed before publication"));
        }
        crate::dat::rename_apply::noclobber::rename_noreplace(staged.path(), &self.destination)
            .map_err(refuse)?;
        File::open(&self.parent.path)?.sync_all()?;
        self.preview.verify_output(&self.destination)?;
        Ok(self.destination.clone())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IpBinBootTargetStatus {
    Present(PathBuf),
    Missing,
    CaseMismatch(Vec<PathBuf>),
    Ambiguous(Vec<PathBuf>),
    Unsafe(PathBuf),
    NotChecked,
}
/// Read-only root membership check, bounded to the DCP tree's entry ceiling.
pub fn check_boot_target(root: &Path, boot_filename: &str) -> io::Result<IpBinBootTargetStatus> {
    if !valid_boot_filename(boot_filename) {
        return Ok(IpBinBootTargetStatus::NotChecked);
    }
    OutputParent::inspect(root)?;
    let mut candidates = Vec::new();
    for (index, entry) in fs::read_dir(root)?.enumerate() {
        if index >= 16_384 {
            return Err(refuse("boot-target directory entry bound exceeded"));
        }
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|s| s.eq_ignore_ascii_case(boot_filename))
        {
            candidates.push(entry.path());
        }
    }
    candidates.sort();
    match candidates.len() {
        0 => Ok(IpBinBootTargetStatus::Missing),
        1 => {
            let candidate = candidates.pop().unwrap();
            if !fs::symlink_metadata(&candidate)?.is_file() {
                return Ok(IpBinBootTargetStatus::Unsafe(candidate));
            }
            if candidate.file_name().unwrap() == boot_filename {
                Ok(IpBinBootTargetStatus::Present(candidate))
            } else {
                Ok(IpBinBootTargetStatus::CaseMismatch(vec![candidate]))
            }
        }
        _ => Ok(IpBinBootTargetStatus::Ambiguous(candidates)),
    }
}

#[cfg(test)]
mod tests;
