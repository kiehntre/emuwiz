//! Strict, bounded parser for one WHDLoad `.slave` Amiga HUNK binary.

use std::io::Read;
use std::path::{Path, PathBuf};

use sha1::Sha1;
use sha2::{Digest, Sha256};

const HUNK_CODE: u32 = 0x0000_03e9;
const HUNK_END: u32 = 0x0000_03f2;
const HUNK_HEADER: u32 = 0x0000_03f3;
const HUNK_TYPE_MASK: u32 = 0x3fff_ffff;
const SECURITY: [u8; 4] = [0x70, 0xff, 0x4e, 0x75];
const ID: &[u8; 8] = b"WHDLOADS";
const MAX_SLAVE_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_STRING_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlaveError {
    Io { path: PathBuf, detail: String },
    TooLarge { bytes: u64, limit: u64 },
    Truncated,
    InvalidHunkHeader,
    MalformedHunk(&'static str),
    UnsupportedHunk(u32),
    MissingCodeHunk,
    MultipleCodeHunks,
    InvalidSecurity,
    InvalidId,
    UnsupportedVersion(u16),
    InvalidPointer { field: &'static str },
    UnterminatedString { field: &'static str },
    NonAsciiString { field: &'static str },
}

impl std::fmt::Display for SlaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WHDLoad slave parse error: {self:?}")
    }
}
impl std::error::Error for SlaveError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedWHDLoadSlave {
    pub runtime_version: u16,
    pub struct_size: usize,
    pub flags: u16,
    pub base_mem_size: u32,
    pub exec_install: u32,
    pub game_loader: u32,
    pub current_dir: Option<String>,
    pub dont_cache: Option<u8>,
    pub key_debug: Option<u8>,
    pub key_exit: Option<u8>,
    pub exp_mem: Option<u8>,
    pub name: Option<String>,
    pub copyright: Option<String>,
    pub info: Option<String>,
    pub kick_name: Option<String>,
    pub kick_size: Option<u32>,
    pub kick_crc: Option<u16>,
    pub config: Option<String>,
    /// Versioned trailing fields whose semantics are not needed in this batch.
    pub extension_bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlaveHashes {
    pub sha1: String,
    pub sha256: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlaveArtifact {
    pub path: PathBuf,
    pub name: String,
    pub size_bytes: u64,
    pub parsed: ParsedWHDLoadSlave,
    pub hashes: SlaveHashes,
}

pub fn parse_whdload_slave(bytes: &[u8]) -> Result<ParsedWHDLoadSlave, SlaveError> {
    let code = locate_code_hunk(bytes)?;
    if code.len() < 16 {
        return Err(SlaveError::Truncated);
    }
    if code[..4] != SECURITY {
        return Err(SlaveError::InvalidSecurity);
    }
    if &code[4..12] != ID {
        return Err(SlaveError::InvalidId);
    }
    let version = be16(code, 12)?;
    let size = struct_size(version).ok_or(SlaveError::UnsupportedVersion(version))?;
    if code.len() < size {
        return Err(SlaveError::Truncated);
    }
    let pointer = |offset, field| rptr(code, size, offset, field);
    Ok(ParsedWHDLoadSlave {
        runtime_version: version,
        struct_size: size,
        flags: be16(code, 14)?,
        base_mem_size: be32(code, 16)?,
        exec_install: be32(code, 20)?,
        game_loader: be32(code, 24)?,
        current_dir: pointer(28, "ws_CurrentDir")?,
        dont_cache: (version >= 4).then(|| code[30]),
        key_debug: (version >= 4).then(|| code[31]),
        key_exit: (version >= 8).then(|| code[32]),
        exp_mem: (version >= 8).then(|| code[33]),
        name: if version >= 8 {
            pointer(34, "ws_name")?
        } else {
            None
        },
        copyright: if version >= 10 {
            pointer(36, "ws_copy")?
        } else {
            None
        },
        info: if version >= 10 {
            pointer(38, "ws_info")?
        } else {
            None
        },
        kick_name: if version >= 10 {
            pointer(40, "ws_kickname")?
        } else {
            None
        },
        kick_size: (version >= 16).then(|| be32(code, 42)).transpose()?,
        kick_crc: (version >= 16).then(|| be16(code, 46)).transpose()?,
        config: if version >= 16 {
            pointer(48, "ws_config")?
        } else {
            None
        },
        extension_bytes: code.get(50..size).unwrap_or_default().to_vec(),
    })
}

pub fn inspect_whdload_slave_file(path: &Path) -> Result<SlaveArtifact, SlaveError> {
    let metadata = std::fs::metadata(path).map_err(|e| SlaveError::Io {
        path: path.into(),
        detail: e.to_string(),
    })?;
    if !metadata.is_file() {
        return Err(SlaveError::Io {
            path: path.into(),
            detail: "not a regular file".into(),
        });
    }
    if metadata.len() > MAX_SLAVE_FILE_BYTES {
        return Err(SlaveError::TooLarge {
            bytes: metadata.len(),
            limit: MAX_SLAVE_FILE_BYTES,
        });
    }
    let bytes = std::fs::read(path).map_err(|e| SlaveError::Io {
        path: path.into(),
        detail: e.to_string(),
    })?;
    let parsed = parse_whdload_slave(&bytes)?;
    let hashes = stream_hashes(path)?;
    Ok(SlaveArtifact {
        path: path.into(),
        name: path
            .file_name()
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_default(),
        size_bytes: metadata.len(),
        parsed,
        hashes,
    })
}

fn stream_hashes(path: &Path) -> Result<SlaveHashes, SlaveError> {
    let mut file = std::fs::File::open(path).map_err(|e| SlaveError::Io {
        path: path.into(),
        detail: e.to_string(),
    })?;
    let mut sha1 = Sha1::new();
    let mut sha256 = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|e| SlaveError::Io {
            path: path.into(),
            detail: e.to_string(),
        })?;
        if count == 0 {
            break;
        }
        sha1.update(&buffer[..count]);
        sha256.update(&buffer[..count]);
    }
    Ok(SlaveHashes {
        sha1: hex(sha1.finalize().as_slice()),
        sha256: hex(sha256.finalize().as_slice()),
    })
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn struct_size(v: u16) -> Option<usize> {
    match v {
        1..=3 => Some(30),
        4..=7 => Some(32),
        8..=9 => Some(36),
        10..=15 => Some(42),
        16 => Some(50),
        17..=19 => Some(52),
        20 => Some(54),
        _ => None,
    }
}
fn be16(bytes: &[u8], offset: usize) -> Result<u16, SlaveError> {
    let end = offset.checked_add(2).ok_or(SlaveError::Truncated)?;
    Ok(u16::from_be_bytes(
        bytes
            .get(offset..end)
            .ok_or(SlaveError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}
fn be32(bytes: &[u8], offset: usize) -> Result<u32, SlaveError> {
    let end = offset.checked_add(4).ok_or(SlaveError::Truncated)?;
    Ok(u32::from_be_bytes(
        bytes
            .get(offset..end)
            .ok_or(SlaveError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}
fn rptr(
    code: &[u8],
    struct_size: usize,
    offset: usize,
    field: &'static str,
) -> Result<Option<String>, SlaveError> {
    let raw = be16(code, offset)? as i16;
    if raw == 0 {
        return Ok(None);
    }
    let target = usize::try_from(raw).map_err(|_| SlaveError::InvalidPointer { field })?;
    if target < struct_size || target >= code.len() {
        return Err(SlaveError::InvalidPointer { field });
    }
    let end = target
        .checked_add(MAX_STRING_BYTES)
        .unwrap_or(code.len())
        .min(code.len());
    let value = &code[target..end];
    let nul = value
        .iter()
        .position(|b| *b == 0)
        .ok_or(SlaveError::UnterminatedString { field })?;
    if !value[..nul]
        .iter()
        .all(|b| b.is_ascii() && (*b >= b' ' || *b == b'\t'))
    {
        return Err(SlaveError::NonAsciiString { field });
    }
    Ok(Some(
        String::from_utf8(value[..nul].to_vec())
            .map_err(|_| SlaveError::NonAsciiString { field })?,
    ))
}
fn locate_code_hunk(bytes: &[u8]) -> Result<&[u8], SlaveError> {
    let mut at = 0;
    if be32(bytes, at)? != HUNK_HEADER {
        return Err(SlaveError::InvalidHunkHeader);
    }
    at += 4;
    loop {
        let words = be32(bytes, at)? as usize;
        at = at.checked_add(4).ok_or(SlaveError::Truncated)?;
        if words == 0 {
            break;
        }
        at = at
            .checked_add(
                words
                    .checked_mul(4)
                    .ok_or(SlaveError::MalformedHunk("header name overflow"))?,
            )
            .ok_or(SlaveError::Truncated)?;
        if at > bytes.len() {
            return Err(SlaveError::Truncated);
        }
    }
    let count = be32(bytes, at)? as usize;
    at += 4;
    let first = be32(bytes, at)? as usize;
    at += 4;
    let last = be32(bytes, at)? as usize;
    at += 4;
    if count
        != last
            .checked_sub(first)
            .and_then(|n| n.checked_add(1))
            .ok_or(SlaveError::MalformedHunk("hunk range"))?
    {
        return Err(SlaveError::MalformedHunk("hunk table"));
    }
    at = at
        .checked_add(
            count
                .checked_mul(4)
                .ok_or(SlaveError::MalformedHunk("table overflow"))?,
        )
        .ok_or(SlaveError::Truncated)?;
    if at > bytes.len() {
        return Err(SlaveError::Truncated);
    }
    let mut code = None;
    let mut ended = false;
    while at < bytes.len() {
        let kind = be32(bytes, at)? & HUNK_TYPE_MASK;
        at += 4;
        match kind {
            HUNK_CODE => {
                if code.is_some() {
                    return Err(SlaveError::MultipleCodeHunks);
                }
                let words = be32(bytes, at)? as usize;
                at += 4;
                let len = words
                    .checked_mul(4)
                    .ok_or(SlaveError::MalformedHunk("code overflow"))?;
                let end = at.checked_add(len).ok_or(SlaveError::Truncated)?;
                code = Some(bytes.get(at..end).ok_or(SlaveError::Truncated)?);
                at = end;
            }
            HUNK_END => {
                ended = true;
                break;
            }
            other => return Err(SlaveError::UnsupportedHunk(other)),
        }
    }
    if !ended {
        return Err(SlaveError::MalformedHunk("missing HUNK_END"));
    }
    code.ok_or(SlaveError::MissingCodeHunk)
}
