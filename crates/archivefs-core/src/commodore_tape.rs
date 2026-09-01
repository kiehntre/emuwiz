//! Bounded, read-only structural inspection for Commodore tape containers.
//!
//! This module deliberately observes only the parts of Commodore TAP and T64
//! that are safe to validate without interpreting a tape. A TAP pulse stream
//! is opaque here: the parser does not decode timings or infer a title. A T64
//! directory is enumerated without extracting or reading its members; names
//! and address ranges are descriptive metadata, not verified game identity.
//!
//! The file adapters use [`crate::safe_read`] and read only a fixed prefix.
//! Declared lengths, counts, offsets, and ranges are checked before they are
//! used. No untrusted field controls an allocation or a read size.

use std::fmt;
use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::path::Path;

use crate::content_evidence::{ContentEvidence, ContentEvidenceConfidence, ContentEvidenceKind};
use crate::safe_read::TrustedRoots;

/// Maximum file size accepted for a Commodore tape structural inspection.
/// The parser does not read this many bytes; the limit prevents a file that
/// cannot be inspected as one bounded tape candidate from being accepted.
pub const MAX_TAPE_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Fixed TAP header size (`C64-TAPE-RAW` plus version and length fields).
pub const COMMODORE_TAP_HEADER_BYTES: usize = 20;
/// Maximum bytes read for a TAP inspection.
pub const COMMODORE_TAP_READ_BYTES: usize = COMMODORE_TAP_HEADER_BYTES;

/// T64 tape record/header size.
pub const T64_HEADER_BYTES: usize = 64;
/// T64 directory record size.
pub const T64_ENTRY_BYTES: usize = 32;
/// A normal T64 directory has at most 64 entries. Larger declarations are
/// rejected rather than used to size a read or allocation.
pub const MAX_T64_ENTRIES: usize = 64;
/// Maximum T64 directory bytes inspected (`64 * 32`).
pub const MAX_T64_DIRECTORY_BYTES: usize = MAX_T64_ENTRIES * T64_ENTRY_BYTES;
/// Maximum prefix read for a T64 header and its bounded directory.
pub const T64_READ_BYTES: usize = T64_HEADER_BYTES + MAX_T64_DIRECTORY_BYTES;

const TAP_SIGNATURE: &[u8; 12] = b"C64-TAPE-RAW";
const T64_SIGNATURE: &[u8; 20] = b"C64S tape image file";
const T64_VERSIONS: &[u16] = &[0x0100, 0x0101, 0x0200];

/// The machine byte from a Commodore TAP header. C16/Plus4 is represented so
/// a valid TAP is not mislabelled as C64, even though EmuWiz has no canonical
/// C16 platform entry in this release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommodoreTapeMachine {
    C64,
    Vic20,
    C16Plus4,
}

impl CommodoreTapeMachine {
    pub fn label(self) -> &'static str {
        match self {
            Self::C64 => "Commodore 64",
            Self::Vic20 => "VIC-20",
            Self::C16Plus4 => "C16/Plus4",
        }
    }

    /// The canonical platform ids that exist in this build. C16/Plus4 is
    /// intentionally absent because it has no registry entry yet.
    pub fn platform_id(self) -> Option<&'static str> {
        match self {
            Self::C64 => Some("Commodore 64"),
            Self::Vic20 => Some("VIC-20"),
            Self::C16Plus4 => None,
        }
    }
}

/// Fields proven by a valid Commodore TAP header. Pulse bytes are not decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommodoreTapObservation {
    pub version: u8,
    pub machine: CommodoreTapeMachine,
    pub video_standard: u8,
    pub declared_data_size: u32,
    pub payload_size: u64,
}

/// One bounded T64 directory entry. The member bytes are never read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct T64EntryObservation {
    pub entry_type: u8,
    pub c64_file_type: u8,
    pub load_address: u16,
    pub end_address: u16,
    pub payload_offset: u32,
    pub payload_size: u64,
    /// A safe descriptive rendering of the fixed-width PETSCII/name bytes.
    /// This is never promoted to a game title or verified identity.
    pub name: String,
}

/// Fields proven by a valid bounded T64 header and directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct T64Observation {
    pub version: u16,
    pub max_entries: u16,
    pub used_entries: u16,
    pub tape_name: String,
    pub entries: Vec<T64EntryObservation>,
}

/// Why a tape observation was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommodoreTapeError {
    /// The bytes do not carry this format's identifying header.
    NotRecognized,
    /// The header identifies the format, but its bounded structure is invalid.
    Malformed(&'static str),
    /// A declared size exceeds an explicit parser limit.
    ResourceLimit(&'static str),
    /// The supplied prefix or file is shorter than a required field/range.
    Truncated(&'static str),
    /// A checked offset/size calculation could not be represented.
    ArithmeticOverflow(&'static str),
    /// The safe read layer refused the path or the bounded read failed.
    Read(String),
}

impl fmt::Display for CommodoreTapeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRecognized => formatter.write_str("not a recognized Commodore tape format"),
            Self::Malformed(detail) => write!(formatter, "malformed Commodore tape: {detail}"),
            Self::ResourceLimit(detail) => {
                write!(formatter, "Commodore tape limit exceeded: {detail}")
            }
            Self::Truncated(detail) => write!(formatter, "truncated Commodore tape: {detail}"),
            Self::ArithmeticOverflow(detail) => {
                write!(formatter, "Commodore tape arithmetic overflow: {detail}")
            }
            Self::Read(detail) => write!(formatter, "could not read Commodore tape: {detail}"),
        }
    }
}

impl std::error::Error for CommodoreTapeError {}

/// Parses the fixed Commodore TAP header and checks its declared payload
/// against the actual file length. The payload is intentionally not read.
pub fn parse_commodore_tap(
    header: &[u8],
    file_len: u64,
) -> Result<CommodoreTapObservation, CommodoreTapeError> {
    if header.len() < COMMODORE_TAP_HEADER_BYTES {
        return Err(CommodoreTapeError::Truncated("TAP header"));
    }
    if &header[..TAP_SIGNATURE.len()] != TAP_SIGNATURE {
        return Err(CommodoreTapeError::NotRecognized);
    }

    let version = header[12];
    if version > 2 {
        return Err(CommodoreTapeError::Malformed("unsupported TAP version"));
    }
    let machine = match header[13] {
        0 => CommodoreTapeMachine::C64,
        1 => CommodoreTapeMachine::Vic20,
        2 => CommodoreTapeMachine::C16Plus4,
        _ => return Err(CommodoreTapeError::Malformed("unknown machine byte")),
    };
    let video_standard = header[14];
    if video_standard > 1 {
        return Err(CommodoreTapeError::Malformed("unknown video-standard byte"));
    }
    if header[15] != 0 {
        return Err(CommodoreTapeError::Malformed(
            "reserved TAP header byte is not zero",
        ));
    }

    let declared_data_size = u32::from_le_bytes(
        header[16..20]
            .try_into()
            .expect("TAP header length checked above"),
    );
    if file_len > MAX_TAPE_FILE_BYTES {
        return Err(CommodoreTapeError::ResourceLimit(
            "file is larger than 8 MiB",
        ));
    }
    if declared_data_size == 0 {
        return Err(CommodoreTapeError::Malformed("TAP payload is empty"));
    }
    let expected_len = u64::from(COMMODORE_TAP_HEADER_BYTES as u32)
        .checked_add(u64::from(declared_data_size))
        .ok_or(CommodoreTapeError::ArithmeticOverflow("TAP file length"))?;
    if expected_len > MAX_TAPE_FILE_BYTES {
        return Err(CommodoreTapeError::ResourceLimit(
            "declared payload is larger than 8 MiB",
        ));
    }
    if expected_len != file_len {
        return Err(CommodoreTapeError::Malformed(
            "declared TAP payload does not match file length",
        ));
    }

    Ok(CommodoreTapObservation {
        version,
        machine,
        video_standard,
        declared_data_size,
        payload_size: file_len - COMMODORE_TAP_HEADER_BYTES as u64,
    })
}

/// Parses a bounded T64 header and directory. `data` needs to contain only
/// the 64-byte header plus the declared, capped directory; member bytes are
/// not required and are never read.
pub fn parse_t64(data: &[u8], file_len: u64) -> Result<T64Observation, CommodoreTapeError> {
    if data.len() < T64_HEADER_BYTES {
        return Err(CommodoreTapeError::Truncated("T64 header"));
    }
    if &data[..T64_SIGNATURE.len()] != T64_SIGNATURE {
        return Err(CommodoreTapeError::NotRecognized);
    }
    if data[20..32].iter().any(|byte| *byte != 0) {
        return Err(CommodoreTapeError::Malformed(
            "T64 signature padding is not zero-filled",
        ));
    }
    if file_len > MAX_TAPE_FILE_BYTES {
        return Err(CommodoreTapeError::ResourceLimit(
            "file is larger than 8 MiB",
        ));
    }

    let version = u16::from_le_bytes([data[32], data[33]]);
    if !T64_VERSIONS.contains(&version) {
        return Err(CommodoreTapeError::Malformed("unsupported T64 version"));
    }
    let max_entries = u16::from_le_bytes([data[34], data[35]]);
    let used_entries = u16::from_le_bytes([data[36], data[37]]);
    let max_entries_usize = usize::from(max_entries);
    if max_entries_usize > MAX_T64_ENTRIES {
        return Err(CommodoreTapeError::ResourceLimit(
            "declared T64 entry count exceeds 64",
        ));
    }
    if used_entries > max_entries {
        return Err(CommodoreTapeError::Malformed(
            "used T64 entries exceed maximum entries",
        ));
    }
    let directory_bytes = max_entries_usize
        .checked_mul(T64_ENTRY_BYTES)
        .ok_or(CommodoreTapeError::ArithmeticOverflow("T64 directory size"))?;
    if directory_bytes > MAX_T64_DIRECTORY_BYTES {
        return Err(CommodoreTapeError::ResourceLimit(
            "T64 directory is too large",
        ));
    }
    let table_end = T64_HEADER_BYTES
        .checked_add(directory_bytes)
        .ok_or(CommodoreTapeError::ArithmeticOverflow("T64 directory end"))?;
    if file_len < table_end as u64 || data.len() < table_end {
        return Err(CommodoreTapeError::Truncated("T64 directory"));
    }

    let tape_name = decode_t64_name(&data[40..64]);
    let mut entries = Vec::new();
    let mut ranges = Vec::new();
    for index in 0..max_entries_usize {
        let start = T64_HEADER_BYTES + index * T64_ENTRY_BYTES;
        let record = &data[start..start + T64_ENTRY_BYTES];
        let entry_type = record[0];
        if entry_type == 0 {
            continue;
        }
        if entry_type > 5 {
            return Err(CommodoreTapeError::Malformed("reserved T64 entry type"));
        }
        let load_address = u16::from_le_bytes([record[2], record[3]]);
        let end_address = u16::from_le_bytes([record[4], record[5]]);
        if end_address < load_address {
            return Err(CommodoreTapeError::Malformed(
                "T64 load/end address range is reversed",
            ));
        }
        let payload_offset =
            u32::from_le_bytes(record[8..12].try_into().expect("record is 32 bytes"));
        let payload_size = u64::from(end_address - load_address);
        let payload_end = u64::from(payload_offset)
            .checked_add(payload_size)
            .ok_or(CommodoreTapeError::ArithmeticOverflow("T64 member range"))?;
        if u64::from(payload_offset) < table_end as u64 || payload_end > file_len {
            return Err(CommodoreTapeError::Malformed(
                "T64 member range lies outside the file",
            ));
        }
        ranges.push((u64::from(payload_offset), payload_end));
        entries.push(T64EntryObservation {
            entry_type,
            c64_file_type: record[1],
            load_address,
            end_address,
            payload_offset,
            payload_size,
            name: decode_t64_name(&record[16..32]),
        });
    }
    if used_entries != 0 && entries.len() > usize::from(used_entries) {
        return Err(CommodoreTapeError::Malformed(
            "active T64 entries exceed the used-entry count",
        ));
    }
    ranges.sort_unstable_by_key(|range| range.0);
    for pair in ranges.windows(2) {
        if pair[1].0 < pair[0].1 {
            return Err(CommodoreTapeError::Malformed("T64 member ranges overlap"));
        }
    }

    Ok(T64Observation {
        version,
        max_entries,
        used_entries,
        tape_name,
        entries,
    })
}

/// Opens and inspects only the fixed TAP header through the shared safe-read
/// policy. No payload byte is read.
pub fn inspect_commodore_tap_file(
    path: &Path,
    trusted: &TrustedRoots,
) -> Result<CommodoreTapObservation, CommodoreTapeError> {
    let mut file = open_tape_file(path, trusted)?;
    let file_len = file_len(&file)?;
    let header = read_prefix(&mut file, COMMODORE_TAP_READ_BYTES, file_len)?;
    parse_commodore_tap(&header, file_len)
}

/// Opens and inspects only the T64 header plus its capped directory.
pub fn inspect_t64_file(
    path: &Path,
    trusted: &TrustedRoots,
) -> Result<T64Observation, CommodoreTapeError> {
    let mut file = open_tape_file(path, trusted)?;
    let file_len = file_len(&file)?;
    let read_len = file_len.min(T64_READ_BYTES as u64) as usize;
    let data = read_prefix(&mut file, read_len, file_len)?;
    parse_t64(&data, file_len)
}

/// Content evidence for a validated TAP header. It proves a Commodore tape
/// container, not a program, title, release, or decoded pulse identity.
pub fn observe_commodore_tap_evidence(
    observation: &CommodoreTapObservation,
) -> Vec<ContentEvidence> {
    vec![
        ContentEvidence::new(
            ContentEvidenceKind::MediaClass,
            "Tape",
            ContentEvidenceConfidence::Strong,
            format!(
                "valid C64-TAPE-RAW container for {}; pulse data was not decoded",
                observation.machine.label()
            ),
        ),
        ContentEvidence::new(
            ContentEvidenceKind::TapeFormat,
            "Commodore TAP",
            ContentEvidenceConfidence::Strong,
            format!(
                "C64-TAPE-RAW header version {}, declared payload {} bytes",
                observation.version, observation.declared_data_size
            ),
        ),
    ]
}

/// Content evidence for a validated T64 header and bounded directory. Member
/// names are intentionally not emitted as identity facts.
pub fn observe_t64_evidence(observation: &T64Observation) -> Vec<ContentEvidence> {
    vec![
        ContentEvidence::new(
            ContentEvidenceKind::MediaClass,
            "Tape",
            ContentEvidenceConfidence::Strong,
            format!(
                "valid T64 directory container with {} active member(s); member bytes were not read",
                observation.entries.len()
            ),
        ),
        ContentEvidence::new(
            ContentEvidenceKind::TapeFormat,
            "T64",
            ContentEvidenceConfidence::Strong,
            format!(
                "C64S/T64 header version 0x{:04x}, {} directory slot(s)",
                observation.version, observation.max_entries
            ),
        ),
    ]
}

fn decode_t64_name(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let trimmed = bytes[..end]
        .iter()
        .copied()
        .skip_while(|byte| *byte == b' ');
    let mut chars: Vec<char> = trimmed
        .map(|byte| match byte {
            0x20..=0x7e => byte as char,
            _ => '\u{fffd}',
        })
        .collect();
    while chars.last() == Some(&' ') {
        chars.pop();
    }
    chars.into_iter().collect()
}

fn open_tape_file(path: &Path, trusted: &TrustedRoots) -> Result<File, CommodoreTapeError> {
    crate::safe_read::open_bounded_read(path, trusted)
        .map(crate::safe_read::SafeFile::into_file)
        .map_err(|error| CommodoreTapeError::Read(error.detail()))
}

fn file_len(file: &File) -> Result<u64, CommodoreTapeError> {
    file.metadata()
        .map(|metadata| metadata.len())
        .map_err(|error| CommodoreTapeError::Read(error.to_string()))
}

fn read_prefix(
    file: &mut File,
    length: usize,
    file_len: u64,
) -> Result<Vec<u8>, CommodoreTapeError> {
    if length == 0 || length > T64_READ_BYTES {
        return Err(CommodoreTapeError::ResourceLimit(
            "prefix read exceeds bound",
        ));
    }
    let length_u64 = u64::try_from(length)
        .map_err(|_| CommodoreTapeError::ArithmeticOverflow("prefix length"))?;
    if length_u64 > file_len {
        return Err(CommodoreTapeError::Truncated("bounded prefix"));
    }
    file.seek(SeekFrom::Start(0))
        .and_then(|_| {
            let mut data = vec![0_u8; length];
            std::io::Read::read_exact(file, &mut data).map(|_| data)
        })
        .map_err(|error| CommodoreTapeError::Read(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tap_header(version: u8, machine: u8, payload_size: u32) -> Vec<u8> {
        let mut bytes = Vec::from(&b"C64-TAPE-RAW"[..]);
        bytes.extend([version, machine, 0, 0]);
        bytes.extend(payload_size.to_le_bytes());
        bytes
    }

    fn t64_header(version: u16, max_entries: u16, used_entries: u16) -> Vec<u8> {
        let mut bytes = vec![0_u8; T64_HEADER_BYTES + usize::from(max_entries) * T64_ENTRY_BYTES];
        bytes[..T64_SIGNATURE.len()].copy_from_slice(T64_SIGNATURE);
        bytes[32..34].copy_from_slice(&version.to_le_bytes());
        bytes[34..36].copy_from_slice(&max_entries.to_le_bytes());
        bytes[36..38].copy_from_slice(&used_entries.to_le_bytes());
        bytes
    }

    fn add_t64_entry(
        bytes: &mut [u8],
        index: usize,
        entry_type: u8,
        load: u16,
        end: u16,
        offset: u32,
        name: &[u8],
    ) {
        let start = T64_HEADER_BYTES + index * T64_ENTRY_BYTES;
        bytes[start] = entry_type;
        bytes[start + 1] = 0x82;
        bytes[start + 2..start + 4].copy_from_slice(&load.to_le_bytes());
        bytes[start + 4..start + 6].copy_from_slice(&end.to_le_bytes());
        bytes[start + 8..start + 12].copy_from_slice(&offset.to_le_bytes());
        bytes[start + 16..start + 16 + name.len().min(16)]
            .copy_from_slice(&name[..name.len().min(16)]);
    }

    #[test]
    fn valid_minimal_tap_header_parses_without_decoding_payload() {
        let header = tap_header(2, 0, 1);
        let fact = parse_commodore_tap(&header, 21).unwrap();
        assert_eq!(fact.machine, CommodoreTapeMachine::C64);
        assert_eq!(fact.payload_size, 1);
        assert_eq!(observe_commodore_tap_evidence(&fact).len(), 2);
    }

    #[test]
    fn tap_rejects_wrong_magic_truncation_and_empty_payload() {
        let mut wrong = tap_header(2, 0, 1);
        wrong[0] = b'X';
        assert_eq!(
            parse_commodore_tap(&wrong, 21),
            Err(CommodoreTapeError::NotRecognized)
        );
        assert!(matches!(
            parse_commodore_tap(&[0; COMMODORE_TAP_HEADER_BYTES - 1], 20),
            Err(CommodoreTapeError::Truncated(_))
        ));
        assert!(matches!(
            parse_commodore_tap(&tap_header(2, 0, 0), 20),
            Err(CommodoreTapeError::Malformed(_))
        ));
    }

    #[test]
    fn tap_rejects_unsupported_version_machine_video_and_reserved_bytes() {
        for (version, machine, video, reserved) in
            [(3, 0, 0, 0), (2, 3, 0, 0), (2, 0, 2, 0), (2, 0, 0, 1)]
        {
            let mut header = tap_header(version, machine, 1);
            header[14] = video;
            header[15] = reserved;
            assert!(parse_commodore_tap(&header, 21).is_err());
        }
    }

    #[test]
    fn tap_rejects_declared_size_mismatch_and_size_limit() {
        assert!(matches!(
            parse_commodore_tap(&tap_header(2, 0, 2), 21),
            Err(CommodoreTapeError::Malformed(_))
        ));
        assert!(matches!(
            parse_commodore_tap(&tap_header(2, 0, u32::MAX), u64::from(u32::MAX) + 20),
            Err(CommodoreTapeError::ResourceLimit(_))
        ));
    }

    #[test]
    fn random_tap_bytes_fail_closed() {
        assert!(matches!(
            parse_commodore_tap(&[0xa5; 32], 32),
            Err(CommodoreTapeError::NotRecognized)
        ));
    }

    #[test]
    fn valid_t64_parses_bounded_directory_metadata() {
        let mut bytes = t64_header(0x0200, 2, 1);
        let data_offset = bytes.len() as u32;
        add_t64_entry(&mut bytes, 0, 1, 0x0801, 0x0803, data_offset, b"GAME");
        bytes.extend([0xaa, 0xbb]);
        let fact = parse_t64(&bytes, bytes.len() as u64).unwrap();
        assert_eq!(fact.entries.len(), 1);
        assert_eq!(fact.entries[0].name, "GAME");
        assert_eq!(fact.entries[0].payload_size, 2);
        assert_eq!(observe_t64_evidence(&fact)[1].value, "T64");
    }

    #[test]
    fn t64_accepts_zero_padded_header_only_and_rejects_signature_padding() {
        let bytes = t64_header(0x0100, 0, 0);
        assert!(parse_t64(&bytes, bytes.len() as u64).is_ok());
        let mut padded = bytes;
        padded[31] = b' ';
        assert!(matches!(
            parse_t64(&padded, padded.len() as u64),
            Err(CommodoreTapeError::Malformed(_))
        ));
    }

    #[test]
    fn t64_rejects_truncated_bad_version_counts_and_directory_limits() {
        let bytes = t64_header(0x0100, 1, 0);
        assert!(matches!(
            parse_t64(&bytes[..63], 64),
            Err(CommodoreTapeError::Truncated(_))
        ));
        let mut bad_version = t64_header(0x0300, 0, 0);
        assert!(parse_t64(&bad_version, bad_version.len() as u64).is_err());
        bad_version[34..36].copy_from_slice(&2_u16.to_le_bytes());
        bad_version[36..38].copy_from_slice(&3_u16.to_le_bytes());
        assert!(parse_t64(&bad_version, bad_version.len() as u64).is_err());
        let excessive = t64_header(0x0100, (MAX_T64_ENTRIES as u16) + 1, 0);
        assert!(matches!(
            parse_t64(&excessive, excessive.len() as u64),
            Err(CommodoreTapeError::ResourceLimit(_))
        ));
    }

    #[test]
    fn t64_rejects_bad_member_ranges_overlap_and_reserved_types() {
        let mut bytes = t64_header(0x0100, 2, 2);
        let table_end = bytes.len() as u32;
        add_t64_entry(&mut bytes, 0, 1, 0x0801, 0x0804, table_end, b"A");
        add_t64_entry(&mut bytes, 1, 1, 0x0801, 0x0804, table_end + 1, b"B");
        bytes.extend([0, 0, 0, 0]);
        assert!(matches!(
            parse_t64(&bytes, bytes.len() as u64),
            Err(CommodoreTapeError::Malformed(_))
        ));

        let mut reversed = t64_header(0x0100, 1, 1);
        let reversed_offset = reversed.len() as u32;
        add_t64_entry(&mut reversed, 0, 1, 0x0900, 0x0800, reversed_offset, b"bad");
        assert!(parse_t64(&reversed, reversed.len() as u64).is_err());

        let mut reserved = t64_header(0x0100, 1, 1);
        let reserved_offset = reserved.len() as u32;
        add_t64_entry(&mut reserved, 0, 6, 0x0801, 0x0802, reserved_offset, b"bad");
        assert!(parse_t64(&reserved, reserved.len() as u64).is_err());
    }

    #[test]
    fn t64_malformed_name_bytes_are_safe_descriptive_metadata() {
        let mut bytes = t64_header(0x0100, 1, 1);
        let offset = bytes.len() as u32;
        add_t64_entry(&mut bytes, 0, 1, 0x0801, 0x0802, offset, &[0xff, b'X']);
        bytes.push(0);
        let fact = parse_t64(&bytes, bytes.len() as u64).unwrap();
        assert_eq!(fact.entries[0].name, "�X");
    }

    #[test]
    fn t64_rejects_offset_beyond_eof_and_random_bytes() {
        let mut bytes = t64_header(0x0100, 1, 1);
        add_t64_entry(&mut bytes, 0, 1, 0x0801, 0x0802, u32::MAX, b"bad");
        assert!(parse_t64(&bytes, bytes.len() as u64).is_err());
        assert!(matches!(
            parse_t64(&[0xa5; 100], 100),
            Err(CommodoreTapeError::NotRecognized)
        ));
    }
}
