//! PSP ISO to CSO v1 reversible-shrink proof.
//!
//! The source is opened read-only and streamed in 2048-byte sectors. A CSO is
//! accepted only after its header/index/data can be read, the decoded stream
//! has the original length, and its SHA-256 equals the source hash. The
//! output is never replaced or deleted automatically; callers must inspect
//! [`PspShrinkResult::trust`] before presenting it as usable.

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const CSO_BLOCK_SIZE: usize = 2048;
const CSO_HEADER_SIZE: u64 = 24;
const CSO_INDEX_ENTRY_SIZE: u64 = 4;
const CSO_COMPRESSED_FLAG: u32 = 0x8000_0000;
const CSO_OFFSET_MASK: u32 = 0x7fff_ffff;
const TOOL_NAME: &str = "EmuWiz in-process CSO v1 writer";
const TOOL_VERSION: &str = "emuwiz-poc/1; flate2/zlib-rs";

#[derive(Debug)]
pub enum PspShrinkError {
    Io(io::Error),
    InvalidSource(String),
    OutputExists(PathBuf),
    InvalidCso(String),
    Verification(String),
}

impl fmt::Display for PspShrinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::InvalidSource(message) => write!(f, "invalid PSP ISO source: {message}"),
            Self::OutputExists(path) => write!(f, "output already exists: {}", path.display()),
            Self::InvalidCso(message) => write!(f, "invalid CSO: {message}"),
            Self::Verification(message) => write!(f, "reversible verification failed: {message}"),
        }
    }
}

impl std::error::Error for PspShrinkError {}

impl From<io::Error> for PspShrinkError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PspShrinkTrust {
    Untrusted,
    ReversibleVerified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PspShrinkInspection {
    pub source: PathBuf,
    pub source_size: u64,
    pub source_sha256: String,
    pub identity_evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PspShrinkProvenance {
    pub source: PathBuf,
    pub output: PathBuf,
    pub source_sha256: String,
    pub restored_sha256: Option<String>,
    pub tool: String,
    pub tool_version: String,
    pub options: String,
    pub format: String,
    pub block_size: u32,
    pub identity_evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PspShrinkResult {
    pub inspection: PspShrinkInspection,
    pub compressed_size: Option<u64>,
    pub restored_sha256: Option<String>,
    pub trust: PspShrinkTrust,
    pub provenance: PspShrinkProvenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestFailure {
    None,
    Tool,
    HashMismatch,
}

/// Inspect only. No output is opened for writing and the source is never
/// modified. A PSP ISO must be sector aligned; boot evidence is retained as
/// conservative source facts for the later PSP identity adapter.
pub fn inspect_psp_iso(source: &Path) -> Result<PspShrinkInspection, PspShrinkError> {
    let mut input = File::open(source)?;
    let size = input.metadata()?.len();
    if size == 0 || size % CSO_BLOCK_SIZE as u64 != 0 {
        return Err(PspShrinkError::InvalidSource(format!(
            "size {size} is not a non-zero multiple of {CSO_BLOCK_SIZE}"
        )));
    }
    let source_sha256 = hash_reader(&mut input)?;
    Ok(PspShrinkInspection {
        source: source.to_path_buf(),
        source_size: size,
        source_sha256,
        identity_evidence: vec![
            "source format: PSP ISO candidate".to_string(),
            "source opened read-only".to_string(),
            "existing PSP boot/layout evidence remains the identity integration point".to_string(),
        ],
    })
}

/// Convert an ISO to CSO v1 and prove byte-exact restoration.
pub fn convert_psp_iso_to_cso(
    source: &Path,
    output: &Path,
) -> Result<PspShrinkResult, PspShrinkError> {
    convert_internal(source, output, TestFailure::None)
}

fn convert_internal(
    source: &Path,
    output: &Path,
    failure: TestFailure,
) -> Result<PspShrinkResult, PspShrinkError> {
    let inspection = inspect_psp_iso(source)?;
    if output.exists() {
        return Err(PspShrinkError::OutputExists(output.to_path_buf()));
    }
    if failure == TestFailure::Tool {
        return Err(PspShrinkError::Verification(
            "conversion tool failed before producing trusted output".to_string(),
        ));
    }

    let mut input = File::open(source)?;
    let mut cso = OpenOptions::new()
        .create_new(true)
        .write(true)
        .read(true)
        .open(output)?;
    write_cso_v1(&mut input, &mut cso, inspection.source_size)?;
    cso.flush()?;
    cso.sync_all()?;
    let compressed_size = cso.metadata()?.len();

    let temp_dir = tempfile::tempdir_in(output.parent().unwrap_or_else(|| Path::new(".")))?;
    let temp_iso = temp_dir.path().join("restored.iso");
    verify_and_restore(output, &temp_iso, inspection.source_size)?;
    if failure == TestFailure::HashMismatch {
        let mut altered = OpenOptions::new().write(true).open(&temp_iso)?;
        altered.seek(SeekFrom::Start(0))?;
        altered.write_all(&[0xff])?;
        altered.sync_all()?;
    }
    let mut restored = File::open(&temp_iso)?;
    let restored_sha256 = hash_reader(&mut restored)?;
    let trust = if restored_sha256 == inspection.source_sha256 {
        PspShrinkTrust::ReversibleVerified
    } else {
        PspShrinkTrust::Untrusted
    };
    let provenance = PspShrinkProvenance {
        source: source.to_path_buf(),
        output: output.to_path_buf(),
        source_sha256: inspection.source_sha256.clone(),
        restored_sha256: Some(restored_sha256.clone()),
        tool: TOOL_NAME.to_string(),
        tool_version: TOOL_VERSION.to_string(),
        options: "CSO v1; independent 2048-byte blocks; zlib; compression level 6".to_string(),
        format: "CSO v1".to_string(),
        block_size: CSO_BLOCK_SIZE as u32,
        identity_evidence: inspection.identity_evidence.clone(),
    };
    Ok(PspShrinkResult {
        inspection,
        compressed_size: Some(compressed_size),
        restored_sha256: Some(restored_sha256),
        trust,
        provenance,
    })
}

fn hash_reader(reader: &mut File) -> io::Result<String> {
    reader.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn write_cso_v1(
    input: &mut File,
    output: &mut File,
    source_size: u64,
) -> Result<(), PspShrinkError> {
    let block_count = source_size / CSO_BLOCK_SIZE as u64;
    let table_bytes = (block_count + 1)
        .checked_mul(CSO_INDEX_ENTRY_SIZE)
        .ok_or_else(|| PspShrinkError::InvalidSource("CSO index table overflow".to_string()))?;
    let data_start = CSO_HEADER_SIZE + table_bytes;
    let mut indexes = vec![
        0u32;
        usize::try_from(block_count + 1).map_err(|_| {
            PspShrinkError::InvalidSource("too many ISO blocks for CSO v1".to_string())
        })?
    ];
    output.write_all(b"CISO")?;
    output.write_all(&24u32.to_le_bytes())?;
    output.write_all(&source_size.to_le_bytes())?;
    output.write_all(&(CSO_BLOCK_SIZE as u32).to_le_bytes())?;
    output.write_all(&[1, 0, 0, 0])?;
    output.seek(SeekFrom::Start(data_start))?;
    let mut block = vec![0u8; CSO_BLOCK_SIZE];
    let mut position = data_start;
    let data_index_count = indexes.len() - 1;
    for index in &mut indexes[..data_index_count] {
        input.read_exact(&mut block)?;
        let mut compressed = ZlibEncoder::new(Vec::new(), Compression::new(6));
        compressed.write_all(&block)?;
        let compressed = compressed.finish()?;
        let (payload, flag) = if compressed.len() < block.len() {
            (compressed.as_slice(), CSO_COMPRESSED_FLAG)
        } else {
            (block.as_slice(), 0)
        };
        if !position.is_multiple_of(4) {
            return Err(PspShrinkError::InvalidCso(
                "unaligned CSO data position".to_string(),
            ));
        }
        let offset = position / 4;
        let offset = u32::try_from(offset)
            .map_err(|_| PspShrinkError::InvalidCso("CSO v1 offset overflow".to_string()))?;
        if offset & !CSO_OFFSET_MASK != 0 {
            return Err(PspShrinkError::InvalidCso(
                "CSO v1 offset exceeds 31 bits".to_string(),
            ));
        }
        *index = offset | flag;
        output.write_all(payload)?;
        position += payload.len() as u64;
        if !position.is_multiple_of(4) {
            let padding = [0u8; 4];
            output.write_all(&padding[..(4 - position as usize % 4)])?;
            position += 4 - position % 4;
        }
    }
    let final_index = indexes.len() - 1;
    indexes[final_index] = u32::try_from(position / 4)
        .map_err(|_| PspShrinkError::InvalidCso("CSO v1 final offset overflow".to_string()))?;
    output.seek(SeekFrom::Start(CSO_HEADER_SIZE))?;
    for index in indexes {
        output.write_all(&index.to_le_bytes())?;
    }
    Ok(())
}

fn verify_and_restore(
    cso_path: &Path,
    restored_path: &Path,
    source_size: u64,
) -> Result<(), PspShrinkError> {
    let mut cso = File::open(cso_path)?;
    let mut header = [0u8; 24];
    cso.read_exact(&mut header)
        .map_err(|_| PspShrinkError::InvalidCso("truncated CSO header".to_string()))?;
    if &header[0..4] != b"CISO" || u32::from_le_bytes(header[4..8].try_into().unwrap()) != 24 {
        return Err(PspShrinkError::InvalidCso(
            "bad CSO magic or header size".to_string(),
        ));
    }
    let declared_size = u64::from_le_bytes(header[8..16].try_into().unwrap());
    let block_size = u32::from_le_bytes(header[16..20].try_into().unwrap());
    if declared_size != source_size
        || block_size != CSO_BLOCK_SIZE as u32
        || header[20] != 1
        || header[21] != 0
    {
        return Err(PspShrinkError::InvalidCso(
            "unsupported CSO v1 header".to_string(),
        ));
    }
    let block_count = source_size / CSO_BLOCK_SIZE as u64;
    let table_len = usize::try_from(block_count + 1)
        .map_err(|_| PspShrinkError::InvalidCso("CSO index table too large".to_string()))?;
    let mut indexes = vec![0u32; table_len];
    for index in &mut indexes {
        let mut raw = [0u8; 4];
        cso.read_exact(&mut raw)
            .map_err(|_| PspShrinkError::InvalidCso("truncated CSO index table".to_string()))?;
        *index = u32::from_le_bytes(raw);
    }
    let cso_len = cso.metadata()?.len();
    let data_start = CSO_HEADER_SIZE + (table_len as u64 * CSO_INDEX_ENTRY_SIZE);
    let mut restored = File::create(restored_path)?;
    let mut output_size = 0u64;
    for pair in indexes.windows(2) {
        let start = (u64::from(pair[0] & CSO_OFFSET_MASK))
            .checked_mul(4)
            .ok_or_else(|| PspShrinkError::InvalidCso("CSO offset overflow".to_string()))?;
        let end = (u64::from(pair[1] & CSO_OFFSET_MASK))
            .checked_mul(4)
            .ok_or_else(|| PspShrinkError::InvalidCso("CSO offset overflow".to_string()))?;
        if start < data_start || end < start || end > cso_len {
            return Err(PspShrinkError::InvalidCso(
                "CSO block range is outside the file".to_string(),
            ));
        }
        let len = usize::try_from(end - start)
            .map_err(|_| PspShrinkError::InvalidCso("CSO block is too large".to_string()))?;
        cso.seek(SeekFrom::Start(start))?;
        let mut payload = vec![0u8; len];
        cso.read_exact(&mut payload)?;
        let mut block = if pair[0] & CSO_COMPRESSED_FLAG != 0 {
            let mut decoder = ZlibDecoder::new(payload.as_slice());
            let mut expanded = Vec::with_capacity(CSO_BLOCK_SIZE);
            decoder.read_to_end(&mut expanded).map_err(|_| {
                PspShrinkError::InvalidCso("compressed CSO block cannot be decoded".to_string())
            })?;
            expanded
        } else {
            payload
        };
        if block.len() != CSO_BLOCK_SIZE {
            return Err(PspShrinkError::InvalidCso(
                "CSO block does not expand to 2048 bytes".to_string(),
            ));
        }
        restored.write_all(&block)?;
        output_size += block.len() as u64;
        block.clear();
    }
    if output_size != source_size {
        return Err(PspShrinkError::Verification(
            "restored length differs from source".to_string(),
        ));
    }
    restored.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture(dir: &Path) -> (PathBuf, Vec<u8>) {
        let source = dir.join("source.iso");
        let mut bytes = vec![0u8; CSO_BLOCK_SIZE * 3];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(31);
        }
        fs::write(&source, &bytes).unwrap();
        (source, bytes)
    }

    #[test]
    fn successful_round_trip_is_byte_exact_and_provenanced() {
        let dir = tempfile::tempdir().unwrap();
        let (source, bytes) = fixture(dir.path());
        let output = dir.path().join("out.cso");
        let result = convert_psp_iso_to_cso(&source, &output).unwrap();
        assert_eq!(result.trust, PspShrinkTrust::ReversibleVerified);
        assert_eq!(result.inspection.source_size, bytes.len() as u64);
        assert_eq!(result.provenance.format, "CSO v1");
        assert!(result.provenance.tool_version.contains("flate2"));
        assert_eq!(
            result.provenance.source_sha256,
            result.restored_sha256.unwrap()
        );
        assert_eq!(fs::read(&source).unwrap(), bytes);
    }

    #[test]
    fn hash_mismatch_is_untrusted_and_source_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let (source, bytes) = fixture(dir.path());
        let output = dir.path().join("mismatch.cso");
        let result = convert_internal(&source, &output, TestFailure::HashMismatch).unwrap();
        assert_eq!(result.trust, PspShrinkTrust::Untrusted);
        assert_eq!(fs::read(&source).unwrap(), bytes);
        assert!(output.exists());
    }

    #[test]
    fn truncated_cso_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let (source, _) = fixture(dir.path());
        let output = dir.path().join("truncated.cso");
        convert_psp_iso_to_cso(&source, &output).unwrap();
        let length = fs::metadata(&output).unwrap().len();
        let file = OpenOptions::new().write(true).open(&output).unwrap();
        file.set_len(length - 1).unwrap();
        let restored = dir.path().join("restored.iso");
        assert!(verify_and_restore(&output, &restored, CSO_BLOCK_SIZE as u64 * 3).is_err());
        assert!(
            !restored.exists() || fs::metadata(restored).unwrap().len() < CSO_BLOCK_SIZE as u64 * 3
        );
    }

    #[test]
    fn tool_failure_does_not_touch_source_or_create_output() {
        let dir = tempfile::tempdir().unwrap();
        let (source, bytes) = fixture(dir.path());
        let output = dir.path().join("failed.cso");
        let error = convert_internal(&source, &output, TestFailure::Tool).unwrap_err();
        assert!(error.to_string().contains("tool failed"));
        assert!(!output.exists());
        assert_eq!(fs::read(&source).unwrap(), bytes);
    }

    #[test]
    fn existing_output_collision_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (source, _) = fixture(dir.path());
        let output = dir.path().join("existing.cso");
        fs::write(&output, b"keep me").unwrap();
        assert!(matches!(
            convert_psp_iso_to_cso(&source, &output),
            Err(PspShrinkError::OutputExists(_))
        ));
        assert_eq!(fs::read(&output).unwrap(), b"keep me");
    }

    #[test]
    fn temporary_restore_is_cleaned_after_success() {
        let dir = tempfile::tempdir().unwrap();
        let (source, _) = fixture(dir.path());
        let output = dir.path().join("cleaned.cso");
        convert_psp_iso_to_cso(&source, &output).unwrap();
        assert!(!dir.path().join("restored.iso").exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}
