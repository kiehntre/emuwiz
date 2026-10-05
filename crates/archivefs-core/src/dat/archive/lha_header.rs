//! Bounded, metadata-only LHA/LZH raw-header inspector.
//!
//! 7-Zip lists and decodes LHA, but its `-slt` listing carries no member type
//! (a Unix symlink is listed as `Folder = -` and streams its link text like a
//! file), and for level-0 headers it does not even report the Unix host.  The
//! only trustworthy source of the member type is the archive header itself, so
//! this module walks the header chain - and nothing else - to classify every
//! entry.  It never decompresses, never reads member payloads, and never
//! trusts an unchecked length: every size is bounded by the real file length,
//! header chains by [`MAX_EXTENDED_BYTES`]/[`MAX_EXTENDED_HEADERS`], and the
//! entry count by the caller's limit.
//!
//! # Type model
//!
//! LHA has no hard-link representation and no type byte of its own.  A type
//! exists only as (a) the Unix `st_mode` (level-0 `U` extension or the `0x50`
//! extended header), (b) the `-lhd-` directory method, and (c) DOS attribute
//! bits.  Libarchive uses the same sources; its symlink convention is the
//! `S_IFLNK` mode with a `name|target` name.  Anything the header does not
//! prove is [`LhaEntryKind::Unknown`] - never assumed regular.
//!
//! This inspector coexists with 7-Zip: it classifies, 7-Zip decodes, and
//! `lha.rs` refuses unless both describe the same member.

use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;

/// Hard ceiling on one level-3 header (level 2 is bounded by its `u16`).
const MAX_LEVEL3_HEADER_BYTES: u64 = 1024 * 1024;
/// Total bytes of one level-1 extended-header chain.
pub const MAX_EXTENDED_BYTES: u64 = 1024 * 1024;
/// Extended headers walked for one entry.
pub const MAX_EXTENDED_HEADERS: usize = 1024;

const S_IFMT: u16 = 0o170000;
const S_IFREG: u16 = 0o100000;
const S_IFDIR: u16 = 0o040000;
const S_IFLNK: u16 = 0o120000;
const S_IFIFO: u16 = 0o010000;
const S_IFCHR: u16 = 0o020000;
const S_IFBLK: u16 = 0o060000;
const S_IFSOCK: u16 = 0o140000;

/// What the raw header proves an entry to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LhaEntryKind {
    Regular,
    Directory,
    Symlink,
    /// FIFO, device node, socket, or a DOS volume label.
    Special,
    /// The header does not prove a type (or contradicts itself).
    Unknown(&'static str),
}

impl LhaEntryKind {
    pub fn is_regular(&self) -> bool {
        matches!(self, Self::Regular)
    }

    /// Stable, display-safe reason a non-regular member is not verified.
    pub fn refusal_reason(&self) -> Option<&'static str> {
        match self {
            Self::Regular => None,
            Self::Directory => Some("LHA directory member"),
            Self::Symlink => Some("LHA symbolic-link member"),
            Self::Special => Some("LHA special member"),
            Self::Unknown(_) => Some("LHA member type unproven"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LhaRawEntry {
    pub header_level: u8,
    pub kind: LhaEntryKind,
    /// Raw member path bytes; `0xFF` path separators already mapped to `/`.
    pub name: Vec<u8>,
    pub method: [u8; 5],
    pub packed_size: u64,
    pub original_size: u64,
    pub crc16: u16,
    pub host_os: Option<u8>,
    pub unix_mode: Option<u16>,
    pub header_offset: u64,
    pub data_offset: u64,
}

impl LhaRawEntry {
    /// The name as UTF-8, when (and only when) it is exactly valid UTF-8.
    pub fn name_utf8(&self) -> Option<&str> {
        std::str::from_utf8(&self.name).ok()
    }

    pub fn method_str(&self) -> String {
        String::from_utf8_lossy(&self.method).into_owned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LhaHeaderError {
    Truncated { offset: u64 },
    Malformed { offset: u64, detail: &'static str },
    TooManyEntries,
    Io(String),
}

impl std::fmt::Display for LhaHeaderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

/// Positioned reads; implemented for files (the fd-pinned archive) and byte
/// slices (parser tests).
pub trait ReadAt {
    fn read_exact_at_offset(&self, buf: &mut [u8], offset: u64) -> io::Result<()>;
}

impl ReadAt for File {
    fn read_exact_at_offset(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        self.read_exact_at(buf, offset)
    }
}

impl ReadAt for [u8] {
    fn read_exact_at_offset(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        let start = usize::try_from(offset).map_err(|_| io::ErrorKind::UnexpectedEof)?;
        let end = start
            .checked_add(buf.len())
            .ok_or(io::ErrorKind::UnexpectedEof)?;
        buf.copy_from_slice(self.get(start..end).ok_or(io::ErrorKind::UnexpectedEof)?);
        Ok(())
    }
}

fn read_vec<R: ReadAt + ?Sized>(
    source: &R,
    len: u64,
    offset: u64,
    count: u64,
) -> Result<Vec<u8>, LhaHeaderError> {
    // `count` is always bounded by a caller-checked ceiling; the file length
    // check makes truncation explicit rather than an opaque I/O error.
    if offset.checked_add(count).is_none_or(|end| end > len) {
        return Err(LhaHeaderError::Truncated { offset });
    }
    let mut buf = vec![0_u8; count as usize];
    source
        .read_exact_at_offset(&mut buf, offset)
        .map_err(|error| LhaHeaderError::Io(error.to_string()))?;
    Ok(buf)
}

fn le16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

fn le32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// Walks every header in the archive without touching member payloads.
/// Fails closed on any structural problem; a clean `0x00` end marker (or the
/// exact end of the file) ends the walk.
pub fn scan_headers<R: ReadAt + ?Sized>(
    source: &R,
    len: u64,
    max_entries: usize,
) -> Result<Vec<LhaRawEntry>, LhaHeaderError> {
    let mut offset = 0_u64;
    let mut entries = Vec::new();
    while offset < len {
        if read_vec(source, len, offset, 1)?[0] == 0 {
            break;
        }
        if entries.len() >= max_entries {
            return Err(LhaHeaderError::TooManyEntries);
        }
        let entry = parse_entry(source, len, offset)?;
        offset = entry
            .data_offset
            .checked_add(entry.packed_size)
            .filter(|end| *end <= len)
            .ok_or(LhaHeaderError::Truncated { offset })?;
        entries.push(entry);
    }
    Ok(entries)
}

/// Facts gathered from the extended-header chain.
#[derive(Default)]
struct Extended {
    filename: Option<Vec<u8>>,
    directory: Option<Vec<u8>>,
    header_crc: Option<u16>,
    dos_attr: Option<u8>,
    unix_mode: Option<u16>,
    /// A structural irregularity that forbids trusting the entry's type.
    doubt: Option<&'static str>,
}

impl Extended {
    fn absorb(&mut self, kind: u8, data: &[u8]) {
        match kind {
            0x00 if data.len() >= 2 => set_once(&mut self.header_crc, le16(data), &mut self.doubt),
            0x01 => set_once(&mut self.filename, data.to_vec(), &mut self.doubt),
            0x02 => set_once(&mut self.directory, data.to_vec(), &mut self.doubt),
            0x40 if data.len() == 2 => set_once(&mut self.dos_attr, data[0], &mut self.doubt),
            0x50 if data.len() == 2 => set_once(&mut self.unix_mode, le16(data), &mut self.doubt),
            0x00 | 0x40 | 0x50 => self.doubt = Some("malformed extended header"),
            _ => {}
        }
    }
}

fn set_once<T>(slot: &mut Option<T>, value: T, doubt: &mut Option<&'static str>) {
    if slot.replace(value).is_some() {
        *doubt = Some("duplicate extended header");
    }
}

/// Walks one extended-header chain starting at `start`.  `first` is the
/// declared size of the first header; each header ends with the declared size
/// of the next (`width` bytes), `0` terminating.  Returns the bytes consumed.
fn walk_extended<R: ReadAt + ?Sized>(
    source: &R,
    len: u64,
    start: u64,
    end_limit: u64,
    width: usize,
    first: u64,
    extended: &mut Extended,
) -> Result<u64, LhaHeaderError> {
    let mut size = first;
    let mut consumed = 0_u64;
    let mut count = 0_usize;
    while size != 0 {
        count += 1;
        if count > MAX_EXTENDED_HEADERS {
            return Err(LhaHeaderError::Malformed {
                offset: start,
                detail: "too many extended headers",
            });
        }
        if size < 1 + width as u64 {
            return Err(LhaHeaderError::Malformed {
                offset: start + consumed,
                detail: "extended header smaller than its own framing",
            });
        }
        let next_consumed = consumed
            .checked_add(size)
            .filter(|total| *total <= MAX_EXTENDED_BYTES)
            .ok_or(LhaHeaderError::Malformed {
                offset: start,
                detail: "extended header chain exceeds bound",
            })?;
        if start + next_consumed > end_limit {
            return Err(LhaHeaderError::Malformed {
                offset: start + consumed,
                detail: "extended header overruns its container",
            });
        }
        let bytes = read_vec(source, len, start + consumed, size)?;
        let body_end = bytes.len() - width;
        extended.absorb(bytes[0], &bytes[1..body_end]);
        let next = &bytes[body_end..];
        size = if width == 2 {
            u64::from(le16(next))
        } else {
            u64::from(le32(next))
        };
        consumed = next_consumed;
    }
    Ok(consumed)
}

/// Streaming LHA CRC-16 (reflected polynomial `0xA001`, initial value 0).
#[derive(Debug, Default, Clone, Copy)]
pub struct Crc16(u16);

impl Crc16 {
    pub fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u16::from(*byte);
            for _ in 0..8 {
                self.0 = if self.0 & 1 != 0 {
                    (self.0 >> 1) ^ 0xa001
                } else {
                    self.0 >> 1
                };
            }
        }
    }

    pub fn finish(self) -> u16 {
        self.0
    }
}

fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = Crc16::default();
    crc.update(bytes);
    crc.finish()
}

fn separators(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .map(|byte| if *byte == 0xff { b'/' } else { *byte })
        .collect()
}

fn parse_entry<R: ReadAt + ?Sized>(
    source: &R,
    len: u64,
    offset: u64,
) -> Result<LhaRawEntry, LhaHeaderError> {
    let prefix = read_vec(source, len, offset, 22)?;
    let level = prefix[20];
    let mut method = [0_u8; 5];
    method.copy_from_slice(&prefix[2..7]);
    if method[0] != b'-' || method[4] != b'-' {
        return Err(LhaHeaderError::Malformed {
            offset,
            detail: "invalid compression method field",
        });
    }
    let packed = u64::from(le32(&prefix[7..11]));
    let original = u64::from(le32(&prefix[11..15]));
    let mut extended = Extended::default();
    let mut dos_attr_byte = None;
    let mut base_name = Vec::new();
    let crc;
    let host_os;
    let mut level0_mode = None;
    let data_offset;
    let packed_size;

    match level {
        0 | 1 => {
            let size = u64::from(prefix[0]);
            let name_len = u64::from(prefix[21]);
            let header_total = 2 + size;
            let minimum = 22 + name_len + 2 + if level == 1 { 1 + 2 } else { 0 };
            if header_total < minimum {
                return Err(LhaHeaderError::Malformed {
                    offset,
                    detail: "header size smaller than its fixed fields",
                });
            }
            let header = read_vec(source, len, offset, header_total)?;
            if header[2..]
                .iter()
                .fold(0_u8, |sum, byte| sum.wrapping_add(*byte))
                != header[1]
            {
                return Err(LhaHeaderError::Malformed {
                    offset,
                    detail: "header checksum mismatch",
                });
            }
            let name_end = 22 + name_len as usize;
            base_name = separators(&header[22..name_end]);
            crc = le16(&header[name_end..name_end + 2]);
            let after_crc = &header[name_end + 2..];
            if level == 0 {
                dos_attr_byte = Some(prefix[19]);
                host_os = after_crc.first().copied();
                // Unix extension: host, minor version, mtime, mode, uid, gid.
                if host_os == Some(b'U') && after_crc.len() >= 12 {
                    level0_mode = Some(le16(&after_crc[6..8]));
                }
                packed_size = packed;
                data_offset = offset + header_total;
            } else {
                host_os = Some(after_crc[0]);
                if after_crc.len() > 3 {
                    extended.doubt = Some("level 1 base header has unparsed trailing bytes");
                }
                let first = u64::from(le16(&after_crc[1..3]));
                let chain_start = offset + header_total;
                let consumed =
                    walk_extended(source, len, chain_start, len, 2, first, &mut extended)?;
                packed_size = packed
                    .checked_sub(consumed)
                    .ok_or(LhaHeaderError::Malformed {
                        offset,
                        detail: "skip size smaller than extended headers",
                    })?;
                data_offset = chain_start + consumed;
            }
        }
        2 | 3 => {
            let (fixed, width) = if level == 2 { (26_u64, 2) } else { (32_u64, 4) };
            let fixed_bytes = read_vec(source, len, offset, fixed)?;
            let total = if level == 2 {
                u64::from(le16(&fixed_bytes[0..2]))
            } else {
                if le16(&fixed_bytes[0..2]) != 4 {
                    return Err(LhaHeaderError::Malformed {
                        offset,
                        detail: "level 3 word size is not 4",
                    });
                }
                u64::from(le32(&fixed_bytes[24..28]))
            };
            if total < fixed || (level == 3 && total > MAX_LEVEL3_HEADER_BYTES) {
                return Err(LhaHeaderError::Malformed {
                    offset,
                    detail: "header size out of range",
                });
            }
            let mut header = read_vec(source, len, offset, total)?;
            crc = le16(&header[21..23]);
            host_os = Some(header[23]);
            let first = if level == 2 {
                u64::from(le16(&header[24..26]))
            } else {
                u64::from(le32(&header[28..32]))
            };
            walk_extended(
                source,
                len,
                offset + fixed,
                offset + total,
                width,
                first,
                &mut extended,
            )?;
            // The common header's CRC16 covers the whole header with its own
            // field zeroed; verify it as libarchive does.  A missing or
            // wrong CRC leaves the entry unproven rather than trusted.
            match (
                locate_header_crc(&header, fixed as usize, width),
                extended.header_crc,
            ) {
                (Some(position), Some(stored)) => {
                    header[position..position + 2].fill(0);
                    if crc16(&header) != stored {
                        extended.doubt = Some("header CRC mismatch");
                    }
                }
                _ => extended.doubt = Some("header CRC missing"),
            }
            packed_size = packed;
            data_offset = offset + total;
        }
        _ => {
            return Err(LhaHeaderError::Malformed {
                offset,
                detail: "unsupported header level",
            });
        }
    }
    let mut unix_mode = extended.unix_mode.or(level0_mode);
    if extended.unix_mode.is_some() && level0_mode.is_some() {
        extended.doubt = Some("conflicting Unix modes");
        unix_mode = None;
    }
    let (name, name_doubt) = assemble_name(base_name, &extended);
    let doubt = extended.doubt.or(name_doubt);
    let dos_attr = extended.dos_attr.or(dos_attr_byte);
    let kind = classify(
        &method,
        unix_mode,
        dos_attr,
        host_os,
        &name,
        doubt,
        packed_size,
        original,
    );
    Ok(LhaRawEntry {
        header_level: level,
        kind,
        name,
        method,
        packed_size,
        original_size: original,
        crc16: crc,
        host_os,
        unix_mode,
        header_offset: offset,
        data_offset,
    })
}

/// Finds the byte offset of the `0x00` common-header CRC field by re-walking
/// the already-validated in-memory chain.
fn locate_header_crc(header: &[u8], fixed: usize, width: usize) -> Option<usize> {
    let mut position = fixed;
    let mut size = if width == 2 {
        usize::from(le16(&header[fixed - 2..fixed]))
    } else {
        le32(&header[fixed - 4..fixed]) as usize
    };
    while size != 0 {
        let end = position.checked_add(size)?;
        if end > header.len() || size < 1 + width {
            return None;
        }
        if header[position] == 0x00 && size >= 1 + 2 + width {
            return Some(position + 1);
        }
        size = if width == 2 {
            usize::from(le16(&header[end - 2..end]))
        } else {
            le32(&header[end - 4..end]) as usize
        };
        position = end;
    }
    None
}

fn assemble_name(base_name: Vec<u8>, extended: &Extended) -> (Vec<u8>, Option<&'static str>) {
    let mut name = match &extended.filename {
        Some(filename) => filename.clone(),
        None => base_name,
    };
    if let Some(directory) = &extended.directory {
        let mut path = separators(directory);
        if path.last() != Some(&b'/') {
            path.push(b'/');
        }
        path.extend_from_slice(&name);
        name = path;
    }
    // A directory entry may be stored with a trailing separator.
    while name.len() > 1 && name.last() == Some(&b'/') {
        name.pop();
    }
    (name, None)
}

#[allow(clippy::too_many_arguments)]
fn classify(
    method: &[u8; 5],
    unix_mode: Option<u16>,
    dos_attr: Option<u8>,
    host_os: Option<u8>,
    name: &[u8],
    doubt: Option<&'static str>,
    packed: u64,
    original: u64,
) -> LhaEntryKind {
    use LhaEntryKind::*;
    if let Some(reason) = doubt {
        return Unknown(reason);
    }
    let directory_method = method == b"-lhd-";
    let attr_directory = dos_attr.is_some_and(|attr| attr & 0x10 != 0);
    let attr_volume = dos_attr.is_some_and(|attr| attr & 0x08 != 0);
    let kind = match unix_mode {
        Some(mode) => match mode & S_IFMT {
            S_IFREG if directory_method => Unknown("directory method with regular mode"),
            S_IFREG if attr_directory || attr_volume => {
                Unknown("regular mode with directory/volume attribute")
            }
            S_IFREG => Regular,
            S_IFDIR if directory_method => Directory,
            S_IFDIR => Unknown("directory mode without directory method"),
            S_IFLNK => Symlink,
            S_IFIFO | S_IFCHR | S_IFBLK | S_IFSOCK => Special,
            _ => Unknown("unrecognised Unix file type"),
        },
        None => match host_os {
            // A Unix host must carry a mode; its absence proves nothing.
            Some(b'U') => Unknown("Unix host without a mode"),
            // Hosts with no symlink encoding in the LHA header.
            None | Some(0) | Some(b'M') | Some(b'w') | Some(b'W') | Some(b'2') | Some(b'A') => {
                if directory_method {
                    Directory
                } else if attr_directory {
                    Unknown("directory attribute without directory method")
                } else if attr_volume {
                    Special
                } else if name.contains(&b'|') {
                    Unknown("link separator in a name with no Unix mode")
                } else {
                    Regular
                }
            }
            Some(_) => Unknown("unrecognised host OS"),
        },
    };
    if matches!(kind, Directory) && (packed != 0 || original != 0) {
        return Unknown("directory entry carries data");
    }
    kind
}

#[cfg(test)]
pub(crate) mod fixtures;
#[cfg(test)]
mod tests;
