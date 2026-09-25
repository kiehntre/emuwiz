//! Pure, read-only Sega Dreamcast `IP.BIN` boot-sector evidence extraction.
//!
//! `IP.BIN` is not a file inside the ISO9660 filesystem - it is the raw
//! boot sector occupying the very start of a GD-ROM's data area, before
//! the Primary Volume Descriptor (at logical sector 16). This module reads
//! it directly from a [`crate::logical_media::LogicalMedia`] at offset 0 -
//! no filesystem lookup is needed, unlike [`crate::playstation_boot_evidence`]'s
//! `SYSTEM.CNF`.
//!
//! # Format verified, not assumed
//!
//! The 256-byte ("meta information") field layout below was verified
//! against two independent, authoritative sources that agree exactly:
//!
//! - Marcus Comstedt's Dreamcast programming documentation
//!   (`https://mc.pp.se/dc/ip0000.bin.html`), long-cited by the Dreamcast
//!   homebrew/preservation community;
//! - KallistiOS's own `makeip` tool source
//!   (`https://github.com/KallistiOS/KallistiOS/blob/master/utils/makeip/src/field.c`) -
//!   the reference tool actually used to *write* real `IP.BIN` files, so
//!   its field table is definitionally correct for what it produces.
//!
//! ```text
//! [0x00..0x10]  hardware_id           16 bytes  "SEGA SEGAKATANA" (padded)
//! [0x10..0x20]  maker_id              16 bytes  "SEGA ENTERPRISES"
//! [0x20..0x30]  device_info           16 bytes  CRC + " GD-ROM" + disc count
//! [0x30..0x38]  area_symbols           8 bytes  region flag characters
//! [0x38..0x40]  peripherals            8 bytes  hex device-support bitfield
//! [0x40..0x4A]  product_number        10 bytes  e.g. "T-8109N   "
//! [0x4A..0x50]  product_version        6 bytes  e.g. "V1.000"
//! [0x50..0x60]  release_date          16 bytes  e.g. "20000101"
//! [0x60..0x70]  boot_filename         16 bytes  usually "1ST_READ.BIN"
//! [0x70..0x80]  software_maker_name   16 bytes
//! [0x80..0x100] software_name        128 bytes  game title
//! ```
//!
//! `PS-X EXE`'s equivalent verified layout lives in
//! [`crate::playstation_boot_evidence`].
//!
//! # Collision safety - read before treating any of this as identity
//!
//! - The hardware ID field is the actual identifying signature here - not
//!   `IP.BIN`'s mere presence (there is no format byte outside this field
//!   that says "this is IP.BIN" at all; the leading bytes of *any* boot
//!   sector could coincidentally look plausible). This module therefore
//!   only emits [`crate::content_evidence::ContentEvidenceKind::BootStructure`]
//!   evidence when `hardware_id` actually matches a recognised string -
//!   see [`IpBinMetaFact::hardware_id_recognized`].
//! - `SEGA SEGAKATANA` is Sega's own internal codename for the Dreamcast
//!   development platform; `SEGA SEGAMARIO` is a second string this module
//!   also recognises (seen on some dev/debug media) without asserting
//!   exactly what distinguishes it - both are treated identically here as
//!   a recognised Sega boot signature, never a claim about which specific
//!   hardware revision produced it.
//! - `1ST_READ.BIN` existing as a *root filesystem entry* is already
//!   covered by [`crate::iso9660::INTERESTING_ROOT_PATHS`] - a different,
//!   complementary fact from `boot_filename` *declaring* that name inside
//!   `IP.BIN`. Neither alone, nor both together, is proof of Dreamcast on
//!   its own - see the module documentation's final principle.
//! - A GD-ROM's high-density area, a recognised boot signature, and
//!   `1ST_READ.BIN` presence are three *independent* legs of evidence.
//!   This module produces exactly one of them and combines none.
//!
//! Parsing fails closed: a buffer shorter than [`IP_BIN_META_BYTES`]
//! returns `None` rather than a partial/guessed struct.

use serde::{Deserialize, Serialize};

use crate::content_detector::{ContentDetectionOutcome, ContentDetector};
use crate::content_evidence::{ContentEvidence, ContentEvidenceConfidence, ContentEvidenceKind};

/// The size of the `IP.BIN` "meta information" area this module reads and
/// interprets. `IP.BIN` itself continues for much longer (boot code,
/// license screen data), none of which this module touches.
pub const IP_BIN_META_BYTES: usize = 0x100;

/// How confidently one fixed-width IP.BIN field can be interpreted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpBinFieldValidity {
    Valid,
    Warning,
    Invalid,
    Unknown,
}

/// Whether a future writer could change a field without changing the source
/// identity or coordinating another structure. This is descriptive only:
/// this task implements no writer.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpBinMutationSafety {
    SafeInStagedRebuild,
    PreserveOnly,
    UnsafeWithoutFilesystemCoordination,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DreamcastIpBinField {
    pub value: String,
    pub raw_bytes: Vec<u8>,
    pub validity: IpBinFieldValidity,
    pub warnings: Vec<String>,
    pub provenance: String,
    pub contributes_to_identity: bool,
    pub informational_only: bool,
    pub mutation_safety: IpBinMutationSafety,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamcastVgaCompatibility {
    Declared,
    NotDeclared,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpBinChecksumStatus {
    NotProven,
    NotPresentInInspectedMetadata,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamcastIpBinValidationStatus {
    Valid,
    ValidWithWarnings,
    Invalid,
    Truncated,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DreamcastIpBinInspection {
    pub validation_status: DreamcastIpBinValidationStatus,
    pub hardware_id: DreamcastIpBinField,
    pub maker_id: DreamcastIpBinField,
    pub device_information: DreamcastIpBinField,
    pub area_symbols: DreamcastIpBinField,
    pub peripheral_flags: DreamcastIpBinField,
    pub product_number: DreamcastIpBinField,
    pub product_version: DreamcastIpBinField,
    pub release_date: DreamcastIpBinField,
    pub boot_filename: DreamcastIpBinField,
    pub software_maker_name: DreamcastIpBinField,
    pub software_title: DreamcastIpBinField,
    pub vga_compatibility: DreamcastVgaCompatibility,
    pub checksum_status: IpBinChecksumStatus,
    pub warnings: Vec<String>,
    pub conflicts: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DreamcastIpBinInspectionError {
    pub status: DreamcastIpBinValidationStatus,
    pub message: String,
}

const HARDWARE_ID: (usize, usize) = (0x00, 0x10);
const MAKER_ID: (usize, usize) = (0x10, 0x10);
const DEVICE_INFO: (usize, usize) = (0x20, 0x10);
const AREA_SYMBOLS: (usize, usize) = (0x30, 0x08);
const PERIPHERALS: (usize, usize) = (0x38, 0x08);
const PRODUCT_NUMBER: (usize, usize) = (0x40, 0x0A);
const PRODUCT_VERSION: (usize, usize) = (0x4A, 0x06);
const RELEASE_DATE: (usize, usize) = (0x50, 0x10);
const BOOT_FILENAME: (usize, usize) = (0x60, 0x10);
const SOFTWARE_MAKER_NAME: (usize, usize) = (0x70, 0x10);
const SOFTWARE_NAME: (usize, usize) = (0x80, 0x80);

/// The recognised hardware ID strings - see the module documentation.
const RECOGNIZED_HARDWARE_IDS: &[&str] = &["SEGA SEGAKATANA", "SEGA SEGAMARIO"];

/// Every `IP.BIN` meta field, read verbatim and only whitespace/NUL-trimmed,
/// never reinterpreted, reformatted, or validated beyond that trim.
/// `hardware_id_recognized` is the one field this module actually
/// evaluates; every other field is exposed as-observed regardless of
/// whether the hardware ID was recognised, so a caller/probe can still see
/// what a not-quite-matching boot sector says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpBinMetaFact {
    pub hardware_id: String,
    pub hardware_id_recognized: bool,
    pub maker_id: String,
    pub device_info: String,
    pub area_symbols: String,
    pub peripherals: String,
    pub product_number: String,
    pub product_version: String,
    pub release_date: String,
    pub boot_filename: String,
    pub software_maker_name: String,
    pub software_name: String,
}

fn field(bytes: &[u8], (offset, length): (usize, usize)) -> String {
    String::from_utf8_lossy(&bytes[offset..offset + length])
        .trim_matches(|c: char| c == '\0' || c.is_whitespace())
        .to_string()
}

/// Parses the `IP.BIN` meta area from `bytes`, which must be at least
/// [`IP_BIN_META_BYTES`] long - `bytes` shorter than that (a truncated
/// read, or a disc with no real boot sector there at all) returns `None`
/// rather than a partial struct. `bytes` may be longer; only the first
/// [`IP_BIN_META_BYTES`] are read.
pub fn parse_ip_bin_meta(bytes: &[u8]) -> Option<IpBinMetaFact> {
    if bytes.len() < IP_BIN_META_BYTES {
        return None;
    }
    let hardware_id = field(bytes, HARDWARE_ID);
    let hardware_id_recognized = RECOGNIZED_HARDWARE_IDS.contains(&hardware_id.as_str());
    Some(IpBinMetaFact {
        hardware_id_recognized,
        hardware_id,
        maker_id: field(bytes, MAKER_ID),
        device_info: field(bytes, DEVICE_INFO),
        area_symbols: field(bytes, AREA_SYMBOLS),
        peripherals: field(bytes, PERIPHERALS),
        product_number: field(bytes, PRODUCT_NUMBER),
        product_version: field(bytes, PRODUCT_VERSION),
        release_date: field(bytes, RELEASE_DATE),
        boot_filename: field(bytes, BOOT_FILENAME),
        software_maker_name: field(bytes, SOFTWARE_MAKER_NAME),
        software_name: field(bytes, SOFTWARE_NAME),
    })
}

/// Performs preservation-safe validation over the bounded metadata area.
///
/// This function never interprets the rest of the bootstrap, never computes a
/// claimed checksum, and never changes the supplied bytes. Unknown peripheral
/// bits remain visible as warnings rather than being assigned invented names.
pub fn inspect_ip_bin_meta(
    bytes: &[u8],
) -> Result<DreamcastIpBinInspection, DreamcastIpBinInspectionError> {
    if bytes.len() < IP_BIN_META_BYTES {
        return Err(DreamcastIpBinInspectionError {
            status: DreamcastIpBinValidationStatus::Truncated,
            message: format!(
                "IP.BIN metadata is truncated: {} bytes available, {} required",
                bytes.len(),
                IP_BIN_META_BYTES
            ),
        });
    }
    let fact = parse_ip_bin_meta(bytes).expect("length checked above");
    let mut warnings = Vec::new();
    let mut invalid = false;
    let hardware = validated_field(
        "hardware ID",
        &bytes[HARDWARE_ID.0..HARDWARE_ID.0 + HARDWARE_ID.1],
        &fact.hardware_id,
        fact.hardware_id_recognized,
        true,
        false,
        IpBinMutationSafety::PreserveOnly,
        "bounded IP.BIN hardware ID field",
    );
    if !fact.hardware_id_recognized {
        invalid = true;
        warnings.push("IP.BIN hardware signature is not a recognised Dreamcast marker".into());
    }
    let maker = validated_field(
        "maker ID",
        &bytes[MAKER_ID.0..MAKER_ID.0 + MAKER_ID.1],
        &fact.maker_id,
        !fact.maker_id.is_empty(),
        false,
        true,
        IpBinMutationSafety::PreserveOnly,
        "bounded IP.BIN maker ID field",
    );
    let device = validated_field(
        "device information",
        &bytes[DEVICE_INFO.0..DEVICE_INFO.0 + DEVICE_INFO.1],
        &fact.device_info,
        !fact.device_info.is_empty(),
        false,
        true,
        IpBinMutationSafety::Unknown,
        "bounded IP.BIN device information field",
    );
    let (area_validity, area_warnings) = validate_area_symbols(&fact.area_symbols);
    warnings.extend(area_warnings.iter().cloned());
    let area = field_with(
        fact.area_symbols.clone(),
        raw(bytes, AREA_SYMBOLS),
        area_validity,
        area_warnings,
        "IP.BIN area-symbol field",
        true,
        false,
        IpBinMutationSafety::SafeInStagedRebuild,
    );
    let (peripheral_validity, peripheral_warnings, vga) = validate_peripherals(&fact.peripherals);
    warnings.extend(peripheral_warnings.iter().cloned());
    let peripheral = field_with(
        fact.peripherals.clone(),
        raw(bytes, PERIPHERALS),
        peripheral_validity,
        peripheral_warnings,
        "IP.BIN peripheral bitfield; known VGA bit is decoded, other unknown bits remain raw",
        false,
        false,
        IpBinMutationSafety::SafeInStagedRebuild,
    );
    let (product_validity, product_warnings) = validate_product_number(&fact.product_number);
    warnings.extend(product_warnings.iter().cloned());
    let product = field_with(
        fact.product_number.clone(),
        raw(bytes, PRODUCT_NUMBER),
        product_validity,
        product_warnings,
        "IP.BIN product-number field",
        fact.hardware_id_recognized && !fact.product_number.is_empty(),
        false,
        IpBinMutationSafety::SafeInStagedRebuild,
    );
    let (version_validity, version_warnings) = validate_version(&fact.product_version);
    warnings.extend(version_warnings.iter().cloned());
    let version = field_with(
        fact.product_version.clone(),
        raw(bytes, PRODUCT_VERSION),
        version_validity,
        version_warnings,
        "IP.BIN product-version field",
        false,
        true,
        IpBinMutationSafety::SafeInStagedRebuild,
    );
    let (date_validity, date_warnings) = validate_date(&fact.release_date);
    warnings.extend(date_warnings.iter().cloned());
    let date = field_with(
        fact.release_date.clone(),
        raw(bytes, RELEASE_DATE),
        date_validity,
        date_warnings,
        "IP.BIN release-date field",
        false,
        true,
        IpBinMutationSafety::SafeInStagedRebuild,
    );
    let (boot_validity, boot_warnings) = validate_boot_filename(&fact.boot_filename);
    warnings.extend(boot_warnings.iter().cloned());
    let boot = field_with(
        fact.boot_filename.clone(),
        raw(bytes, BOOT_FILENAME),
        boot_validity,
        boot_warnings,
        "IP.BIN boot-filename field; filesystem target not checked by this byte-only API",
        false,
        true,
        IpBinMutationSafety::UnsafeWithoutFilesystemCoordination,
    );
    let maker_name = informational_field(
        fact.software_maker_name.clone(),
        raw(bytes, SOFTWARE_MAKER_NAME),
        "IP.BIN software-maker field",
    );
    let title = informational_field(
        fact.software_name.clone(),
        raw(bytes, SOFTWARE_NAME),
        "IP.BIN software-title field",
    );
    if maker.validity == IpBinFieldValidity::Invalid
        || device.validity == IpBinFieldValidity::Invalid
        || area.validity == IpBinFieldValidity::Invalid
        || peripheral.validity == IpBinFieldValidity::Invalid
        || product.validity == IpBinFieldValidity::Invalid
        || version.validity == IpBinFieldValidity::Invalid
        || date.validity == IpBinFieldValidity::Invalid
        || boot.validity == IpBinFieldValidity::Invalid
    {
        invalid = true;
    }
    let validation_status = if invalid {
        DreamcastIpBinValidationStatus::Invalid
    } else if !warnings.is_empty() {
        DreamcastIpBinValidationStatus::ValidWithWarnings
    } else {
        DreamcastIpBinValidationStatus::Valid
    };
    Ok(DreamcastIpBinInspection {
        validation_status,
        hardware_id: hardware,
        maker_id: maker,
        device_information: device,
        area_symbols: area,
        peripheral_flags: peripheral,
        product_number: product,
        product_version: version,
        release_date: date,
        boot_filename: boot,
        software_maker_name: maker_name,
        software_title: title,
        vga_compatibility: vga,
        checksum_status: IpBinChecksumStatus::NotProven,
        warnings,
        conflicts: Vec::new(),
    })
}

/// Adds a conflict against stronger, already-resolved evidence. The stronger
/// evidence remains authoritative; this function only makes disagreement
/// visible.
pub fn add_stronger_evidence_conflicts(
    inspection: &mut DreamcastIpBinInspection,
    stronger_product_code: Option<&str>,
    stronger_region_symbols: Option<&str>,
) {
    if let Some(stronger) = stronger_product_code
        && !inspection.product_number.value.is_empty()
        && !inspection
            .product_number
            .value
            .eq_ignore_ascii_case(stronger)
    {
        inspection.conflicts.push(format!(
            "IP.BIN product code {} conflicts with stronger disc evidence {}",
            inspection.product_number.value, stronger
        ));
    }
    if let Some(stronger) = stronger_region_symbols
        && !inspection.area_symbols.value.is_empty()
        && inspection.area_symbols.value != stronger
    {
        inspection.conflicts.push(format!(
            "IP.BIN region symbols {} conflict with stronger disc evidence {}",
            inspection.area_symbols.value, stronger
        ));
    }
}

fn raw(bytes: &[u8], range: (usize, usize)) -> Vec<u8> {
    bytes[range.0..range.0 + range.1].to_vec()
}

fn field_with(
    value: String,
    raw_bytes: Vec<u8>,
    validity: IpBinFieldValidity,
    warnings: Vec<String>,
    provenance: &str,
    contributes_to_identity: bool,
    informational_only: bool,
    mutation_safety: IpBinMutationSafety,
) -> DreamcastIpBinField {
    DreamcastIpBinField {
        value,
        raw_bytes,
        validity,
        warnings,
        provenance: provenance.into(),
        contributes_to_identity,
        informational_only,
        mutation_safety,
    }
}

fn informational_field(value: String, raw_bytes: Vec<u8>, provenance: &str) -> DreamcastIpBinField {
    field_with(
        value,
        raw_bytes,
        IpBinFieldValidity::Valid,
        Vec::new(),
        provenance,
        false,
        true,
        IpBinMutationSafety::SafeInStagedRebuild,
    )
}

fn validated_field(
    label: &str,
    raw_bytes: &[u8],
    value: &str,
    required: bool,
    contributes_to_identity: bool,
    informational_only: bool,
    mutation_safety: IpBinMutationSafety,
    provenance: &str,
) -> DreamcastIpBinField {
    let mut warnings = Vec::new();
    let mut validity = IpBinFieldValidity::Valid;
    if !raw_bytes.is_ascii() {
        validity = IpBinFieldValidity::Invalid;
        warnings.push(format!("{label} contains non-ASCII bytes"));
    }
    if required && value.is_empty() {
        validity = IpBinFieldValidity::Invalid;
        warnings.push(format!("{label} is blank"));
    }
    field_with(
        value.to_string(),
        raw_bytes.to_vec(),
        validity,
        warnings,
        provenance,
        contributes_to_identity,
        informational_only,
        mutation_safety,
    )
}

fn validate_area_symbols(value: &str) -> (IpBinFieldValidity, Vec<String>) {
    let mut warnings = Vec::new();
    let unknown: String = value
        .chars()
        .filter(|character| !matches!(character, 'J' | 'U' | 'E' | 'A' | 'K'))
        .collect();
    if !unknown.is_empty() {
        warnings.push(format!("unknown IP.BIN area symbols: {unknown}"));
        (IpBinFieldValidity::Warning, warnings)
    } else if value.is_empty() {
        warnings.push("IP.BIN area symbols are blank".into());
        (IpBinFieldValidity::Warning, warnings)
    } else {
        (IpBinFieldValidity::Valid, warnings)
    }
}

fn validate_peripherals(
    value: &str,
) -> (IpBinFieldValidity, Vec<String>, DreamcastVgaCompatibility) {
    if value.is_empty() {
        return (
            IpBinFieldValidity::Warning,
            vec!["IP.BIN peripheral flags are blank".into()],
            DreamcastVgaCompatibility::Unknown,
        );
    }
    if value.len() > 8 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return (
            IpBinFieldValidity::Invalid,
            vec!["IP.BIN peripheral flags are not bounded hexadecimal".into()],
            DreamcastVgaCompatibility::Unknown,
        );
    }
    let parsed = u32::from_str_radix(value, 16).ok();
    let Some(bits) = parsed else {
        return (
            IpBinFieldValidity::Invalid,
            vec!["IP.BIN peripheral flags could not be decoded".into()],
            DreamcastVgaCompatibility::Unknown,
        );
    };
    let unknown = bits & !((1_u32 << 21) - 1);
    let mut warnings = Vec::new();
    if unknown != 0 {
        warnings.push(format!(
            "IP.BIN contains unknown peripheral bits 0x{unknown:08X}"
        ));
    }
    // The independent bodgit/dreamcast model assigns bit 1 to VGA. We expose
    // only that corroborated meaning and keep all other bits unnamed here.
    let vga = if bits & (1 << 1) != 0 {
        DreamcastVgaCompatibility::Declared
    } else {
        DreamcastVgaCompatibility::NotDeclared
    };
    (
        if warnings.is_empty() {
            IpBinFieldValidity::Valid
        } else {
            IpBinFieldValidity::Warning
        },
        warnings,
        vga,
    )
}

fn validate_product_number(value: &str) -> (IpBinFieldValidity, Vec<String>) {
    if value.is_empty() {
        return (
            IpBinFieldValidity::Warning,
            vec!["IP.BIN product number is blank".into()],
        );
    }
    if value.len() > 10 || !value.bytes().all(|byte| byte.is_ascii_graphic()) || value.contains(' ')
    {
        return (
            IpBinFieldValidity::Invalid,
            vec!["IP.BIN product number is not a compact printable identifier".into()],
        );
    }
    (IpBinFieldValidity::Valid, Vec::new())
}

fn validate_version(value: &str) -> (IpBinFieldValidity, Vec<String>) {
    if value.is_empty() {
        return (
            IpBinFieldValidity::Warning,
            vec!["IP.BIN product version is blank".into()],
        );
    }
    let valid = value.len() >= 3
        && value.as_bytes()[0] == b'V'
        && value[1..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.');
    if valid {
        (IpBinFieldValidity::Valid, Vec::new())
    } else {
        (
            IpBinFieldValidity::Invalid,
            vec!["IP.BIN product version has an unexpected format".into()],
        )
    }
}

fn validate_date(value: &str) -> (IpBinFieldValidity, Vec<String>) {
    if value.is_empty() {
        return (
            IpBinFieldValidity::Warning,
            vec!["IP.BIN release date is blank".into()],
        );
    }
    if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return (
            IpBinFieldValidity::Invalid,
            vec!["IP.BIN release date is not YYYYMMDD".into()],
        );
    }
    let month = value[4..6].parse::<u32>().unwrap_or_default();
    let day = value[6..8].parse::<u32>().unwrap_or_default();
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return (
            IpBinFieldValidity::Invalid,
            vec!["IP.BIN release date has an impossible month or day".into()],
        );
    }
    (IpBinFieldValidity::Valid, Vec::new())
}

fn validate_boot_filename(value: &str) -> (IpBinFieldValidity, Vec<String>) {
    if value.is_empty() {
        return (
            IpBinFieldValidity::Invalid,
            vec!["IP.BIN boot filename is blank".into()],
        );
    }
    if value.contains('/')
        || value.contains('\\')
        || value.contains("..")
        || value.bytes().any(|byte| byte < 0x20 || byte > 0x7E)
    {
        return (
            IpBinFieldValidity::Invalid,
            vec!["IP.BIN boot filename is suspicious or unsafe".into()],
        );
    }
    (IpBinFieldValidity::Valid, Vec::new())
}

/// Turns a parsed [`IpBinMetaFact`] into neutral evidence.
///
/// Emits nothing at all when `hardware_id` was not recognised - an
/// unrecognised boot sector's other fields (product number, etc.) are not
/// promoted to evidence, since without the hardware ID match there is no
/// basis to trust this region is really a Dreamcast-shaped `IP.BIN` in the
/// first place. When it *is* recognised: a `BootStructure` fact
/// (`Strong` - see the module documentation on why the hardware ID field
/// specifically is the trustworthy signature here), and, only if
/// `product_number` is non-empty, a `ProductCode` fact (`Corroborated` -
/// a candidate release identifier, not a platform claim).
pub fn observe_ip_bin_evidence(fact: &IpBinMetaFact) -> Vec<ContentEvidence> {
    if !fact.hardware_id_recognized {
        return Vec::new();
    }
    let mut evidence = vec![ContentEvidence::new(
        ContentEvidenceKind::BootStructure,
        fact.hardware_id.clone(),
        ContentEvidenceConfidence::Strong,
        "IP.BIN hardware ID field matches a recognised Sega boot signature",
    )];
    if !fact.product_number.is_empty() {
        evidence.push(ContentEvidence::new(
            ContentEvidenceKind::ProductCode,
            fact.product_number.clone(),
            ContentEvidenceConfidence::Corroborated,
            "candidate product/catalog number read from the IP.BIN product number field - not \
             verified against a canonical release list, and not proof of any one platform",
        ));
    }
    evidence
}

/// A [`ContentDetector`] operating on a bounded `IP.BIN`-shaped buffer (the
/// first [`IP_BIN_META_BYTES`] of a data track/area - a caller reads those
/// bytes via [`crate::logical_media::LogicalMedia::read_at`] at offset 0
/// first). `NotRecognized` when too short or the hardware ID does not
/// match; there is no separate `Malformed` state here - unlike a real
/// container format, a fixed-offset text-field region has no structural
/// validity to fail beyond "does the identifying field match".
pub struct IpBinDetector;

impl ContentDetector for IpBinDetector {
    fn id(&self) -> &'static str {
        "dreamcast_ip_bin"
    }

    fn detect(&self, data: &[u8]) -> ContentDetectionOutcome {
        match parse_ip_bin_meta(data) {
            Some(fact) if fact.hardware_id_recognized => ContentDetectionOutcome::Recognized {
                evidence: observe_ip_bin_evidence(&fact),
            },
            _ => ContentDetectionOutcome::NotRecognized,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(bytes: &mut [u8], (offset, length): (usize, usize), value: &[u8]) {
        let end = (offset + value.len()).min(offset + length);
        bytes[offset..end].copy_from_slice(&value[..end - offset]);
    }

    fn synthetic_ip_bin() -> Vec<u8> {
        let mut bytes = vec![b' '; IP_BIN_META_BYTES];
        put(&mut bytes, HARDWARE_ID, b"SEGA SEGAKATANA ");
        put(&mut bytes, MAKER_ID, b"SEGA ENTERPRISES");
        put(&mut bytes, DEVICE_INFO, b"8B40 GD-ROM1/1  ");
        put(&mut bytes, AREA_SYMBOLS, b"JUE     ");
        put(&mut bytes, PERIPHERALS, b"E0000010");
        put(&mut bytes, PRODUCT_NUMBER, b"T-8109N   ");
        put(&mut bytes, PRODUCT_VERSION, b"V1.000");
        put(&mut bytes, RELEASE_DATE, b"20000915        ");
        put(&mut bytes, BOOT_FILENAME, b"1ST_READ.BIN    ");
        put(&mut bytes, SOFTWARE_MAKER_NAME, b"SEGA            ");
        put(&mut bytes, SOFTWARE_NAME, b"TEST GAME");
        bytes
    }

    // ------------------------------------------------------------------
    // Boot signature
    // ------------------------------------------------------------------

    #[test]
    fn recognised_hardware_id_is_detected() {
        let fact = parse_ip_bin_meta(&synthetic_ip_bin()).unwrap();
        assert_eq!(fact.hardware_id, "SEGA SEGAKATANA");
        assert!(fact.hardware_id_recognized);
    }

    #[test]
    fn secondary_recognised_hardware_id_is_detected() {
        let mut bytes = synthetic_ip_bin();
        put(&mut bytes, HARDWARE_ID, b"SEGA SEGAMARIO  ");
        let fact = parse_ip_bin_meta(&bytes).unwrap();
        assert!(fact.hardware_id_recognized);
    }

    #[test]
    fn unrecognised_hardware_id_yields_no_evidence() {
        let mut bytes = synthetic_ip_bin();
        put(&mut bytes, HARDWARE_ID, b"NOT A SEGA DISC ");
        let fact = parse_ip_bin_meta(&bytes).unwrap();
        assert!(!fact.hardware_id_recognized);
        assert!(observe_ip_bin_evidence(&fact).is_empty());
    }

    #[test]
    fn malformed_truncated_ip_bin_fails_closed() {
        let bytes = synthetic_ip_bin();
        assert_eq!(parse_ip_bin_meta(&bytes[..0x50]), None);
        assert_eq!(
            IpBinDetector.detect(&bytes[..0x50]),
            ContentDetectionOutcome::NotRecognized
        );
    }

    // ------------------------------------------------------------------
    // Product/version/region
    // ------------------------------------------------------------------

    #[test]
    fn product_code_is_extracted() {
        let fact = parse_ip_bin_meta(&synthetic_ip_bin()).unwrap();
        assert_eq!(fact.product_number, "T-8109N");
    }

    #[test]
    fn version_is_extracted() {
        let fact = parse_ip_bin_meta(&synthetic_ip_bin()).unwrap();
        assert_eq!(fact.product_version, "V1.000");
    }

    #[test]
    fn region_area_symbols_are_extracted() {
        let fact = parse_ip_bin_meta(&synthetic_ip_bin()).unwrap();
        assert_eq!(fact.area_symbols, "JUE");
    }

    #[test]
    fn boot_filename_is_extracted() {
        let fact = parse_ip_bin_meta(&synthetic_ip_bin()).unwrap();
        assert_eq!(fact.boot_filename, "1ST_READ.BIN");
    }

    // ------------------------------------------------------------------
    // Evidence / platform-safety boundary
    // ------------------------------------------------------------------

    #[test]
    fn boot_signature_evidence_is_strong() {
        let fact = parse_ip_bin_meta(&synthetic_ip_bin()).unwrap();
        let evidence = observe_ip_bin_evidence(&fact);
        let boot = evidence
            .iter()
            .find(|item| item.kind == ContentEvidenceKind::BootStructure)
            .unwrap();
        assert_eq!(boot.confidence, ContentEvidenceConfidence::Strong);
    }

    #[test]
    fn product_code_evidence_is_corroborated() {
        let fact = parse_ip_bin_meta(&synthetic_ip_bin()).unwrap();
        let evidence = observe_ip_bin_evidence(&fact);
        let product = evidence
            .iter()
            .find(|item| item.kind == ContentEvidenceKind::ProductCode)
            .unwrap();
        assert_eq!(product.confidence, ContentEvidenceConfidence::Corroborated);
    }

    #[test]
    fn ip_bin_alone_does_not_assign_dreamcast() {
        // Structural: IpBinMetaFact and the evidence derived from it carry
        // no platform field, and this module imports nothing from
        // crate::platform or crate::dat::identity.
        let fact = parse_ip_bin_meta(&synthetic_ip_bin()).unwrap();
        for item in observe_ip_bin_evidence(&fact) {
            assert!(matches!(
                item.kind,
                ContentEvidenceKind::BootStructure | ContentEvidenceKind::ProductCode
            ));
        }
    }

    #[test]
    fn parsing_never_mutates_input() {
        let data = synthetic_ip_bin();
        let before = data.clone();
        let _ = parse_ip_bin_meta(&data);
        assert_eq!(data, before);
    }

    #[test]
    fn repeated_parse_is_deterministic() {
        let data = synthetic_ip_bin();
        assert_eq!(parse_ip_bin_meta(&data), parse_ip_bin_meta(&data));
    }

    #[test]
    fn empty_product_number_yields_no_product_code_evidence() {
        let mut bytes = synthetic_ip_bin();
        put(&mut bytes, PRODUCT_NUMBER, b"          ");
        let fact = parse_ip_bin_meta(&bytes).unwrap();
        assert!(
            !observe_ip_bin_evidence(&fact)
                .iter()
                .any(|item| item.kind == ContentEvidenceKind::ProductCode)
        );
    }

    #[test]
    fn inspection_validates_fields_and_decodes_vga_without_mutating_bytes() {
        let mut data = synthetic_ip_bin();
        put(&mut data, PERIPHERALS, b"00000000");
        let before = data.clone();
        let inspection = inspect_ip_bin_meta(&data).unwrap();
        assert_eq!(
            inspection.validation_status,
            DreamcastIpBinValidationStatus::Valid
        );
        assert_eq!(inspection.product_number.value, "T-8109N");
        assert_eq!(
            inspection.vga_compatibility,
            DreamcastVgaCompatibility::NotDeclared
        );
        assert_eq!(inspection.checksum_status, IpBinChecksumStatus::NotProven);
        assert_eq!(data, before);
        assert_eq!(inspection.product_number.raw_bytes.len(), 10);
    }

    #[test]
    fn inspection_reports_bad_date_regions_peripherals_and_boot_target() {
        let mut data = synthetic_ip_bin();
        put(&mut data, RELEASE_DATE, b"20241340        ");
        put(&mut data, AREA_SYMBOLS, b"JX      ");
        put(&mut data, PERIPHERALS, b"ZZZZZZZZ");
        put(&mut data, BOOT_FILENAME, b"../BAD.BIN       ");
        let inspection = inspect_ip_bin_meta(&data).unwrap();
        assert_eq!(
            inspection.validation_status,
            DreamcastIpBinValidationStatus::Invalid
        );
        assert!(
            inspection
                .warnings
                .iter()
                .any(|warning| warning.contains("release date"))
        );
        assert!(
            inspection
                .warnings
                .iter()
                .any(|warning| warning.contains("area symbols"))
        );
        assert!(
            inspection
                .warnings
                .iter()
                .any(|warning| warning.contains("peripheral"))
        );
        assert!(
            inspection
                .warnings
                .iter()
                .any(|warning| warning.contains("boot filename"))
        );
    }

    #[test]
    fn inspection_handles_vga_and_unknown_peripheral_bits_conservatively() {
        let mut data = synthetic_ip_bin();
        put(&mut data, PERIPHERALS, b"00200002");
        let inspection = inspect_ip_bin_meta(&data).unwrap();
        assert_eq!(
            inspection.vga_compatibility,
            DreamcastVgaCompatibility::Declared
        );
        assert!(
            inspection
                .peripheral_flags
                .warnings
                .iter()
                .any(|warning| warning.contains("unknown peripheral bits"))
        );
    }

    #[test]
    fn stronger_evidence_conflicts_are_exposed_without_overriding_it() {
        let mut inspection = inspect_ip_bin_meta(&synthetic_ip_bin()).unwrap();
        add_stronger_evidence_conflicts(&mut inspection, Some("OTHER"), Some("J"));
        assert_eq!(inspection.conflicts.len(), 2);
        assert_eq!(inspection.product_number.value, "T-8109N");
    }
}
