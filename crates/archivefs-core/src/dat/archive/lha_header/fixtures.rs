//! Synthetic, legal LHA header builders for tests.  Members are always
//! stored (`-lh0-`) so no compressor is needed; the point is the header.

use super::Crc16;

#[derive(Clone)]
pub(crate) struct Entry {
    pub name: Vec<u8>,
    pub payload: Vec<u8>,
    pub method: [u8; 5],
    pub level: u8,
    /// Host OS byte (`U` Unix, `M` MS-DOS, ...); `0` generic.
    pub host: u8,
    pub mode: Option<u16>,
    /// Level-0 attribute byte (levels 1-3 use the `0x40` extended header).
    pub dos_attr: u8,
    pub extra_extended: Vec<(u8, Vec<u8>)>,
    pub declared_crc: Option<u16>,
}

impl Entry {
    pub fn file(name: &str, payload: &[u8]) -> Self {
        Self {
            name: name.as_bytes().to_vec(),
            payload: payload.to_vec(),
            method: *b"-lh0-",
            level: 0,
            host: 0,
            mode: None,
            dos_attr: 0x20,
            extra_extended: Vec::new(),
            declared_crc: None,
        }
    }

    pub fn unix(name: &str, payload: &[u8], mode: u16) -> Self {
        Self {
            host: b'U',
            mode: Some(mode),
            ..Self::file(name, payload)
        }
    }

    pub fn directory(name: &str) -> Self {
        Self {
            method: *b"-lhd-",
            ..Self::unix(name, b"", 0o040755)
        }
    }

    pub fn level(mut self, level: u8) -> Self {
        self.level = level;
        self
    }
}

pub(crate) fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = Crc16::default();
    crc.update(bytes);
    crc.finish()
}

/// `(type, data)` extended headers -> chain bytes plus the first header size.
fn chain(headers: &[(u8, Vec<u8>)], width: usize) -> (Vec<u8>, u64) {
    let sizes: Vec<usize> = headers
        .iter()
        .map(|(_, data)| 1 + data.len() + width)
        .collect();
    let mut out = Vec::new();
    for (index, (kind, data)) in headers.iter().enumerate() {
        let next = sizes.get(index + 1).copied().unwrap_or(0) as u64;
        out.push(*kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&next.to_le_bytes()[..width]);
    }
    (out, sizes.first().copied().unwrap_or(0) as u64)
}

pub(crate) fn encode(entry: &Entry) -> Vec<u8> {
    let crc = entry.declared_crc.unwrap_or_else(|| crc16(&entry.payload));
    let packed = entry.payload.len() as u32;
    let mut extended: Vec<(u8, Vec<u8>)> = Vec::new();
    if let (Some(mode), true) = (entry.mode, entry.level != 0) {
        extended.push((0x50, mode.to_le_bytes().to_vec()));
    }
    extended.extend(entry.extra_extended.iter().cloned());
    let common = |bytes: &mut Vec<u8>| {
        bytes.extend_from_slice(&entry.method);
        bytes.extend_from_slice(&packed.to_le_bytes());
        bytes.extend_from_slice(&packed.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
    };
    let mut header = vec![0_u8, 0];
    match entry.level {
        0 => {
            common(&mut header);
            header.extend_from_slice(&[entry.dos_attr, 0, entry.name.len() as u8]);
            header.extend_from_slice(&entry.name);
            header.extend_from_slice(&crc.to_le_bytes());
            header.push(entry.host);
            if let (b'U', Some(mode)) = (entry.host, entry.mode) {
                header.push(0); // minor version
                header.extend_from_slice(&0_u32.to_le_bytes());
                header.extend_from_slice(&mode.to_le_bytes());
                header.extend_from_slice(&[0; 4]); // uid, gid
            }
            header[0] = (header.len() - 2) as u8;
            header[1] = header[2..]
                .iter()
                .fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
            [header, entry.payload.clone()].concat()
        }
        1 => {
            let (ext, first) = chain(&extended, 2);
            common(&mut header);
            header[7..11].copy_from_slice(&(packed + ext.len() as u32).to_le_bytes());
            header.extend_from_slice(&[0x20, 1, entry.name.len() as u8]);
            header.extend_from_slice(&entry.name);
            header.extend_from_slice(&crc.to_le_bytes());
            header.push(entry.host);
            header.extend_from_slice(&(first as u16).to_le_bytes());
            header[0] = (header.len() - 2) as u8;
            header[1] = header[2..]
                .iter()
                .fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
            [header, ext, entry.payload.clone()].concat()
        }
        level @ (2 | 3) => {
            let width = if level == 2 { 2 } else { 4 };
            let mut headers = vec![(0x00, vec![0, 0]), (0x01, entry.name.clone())];
            headers.extend(extended);
            let (ext, first) = chain(&headers, width);
            header = vec![0; 2];
            if level == 3 {
                header[..2].copy_from_slice(&4_u16.to_le_bytes());
            }
            common(&mut header);
            header.extend_from_slice(&[0x20, level]);
            header.extend_from_slice(&crc.to_le_bytes());
            header.push(entry.host);
            if level == 3 {
                header.extend_from_slice(&[0; 4]);
            }
            header.extend_from_slice(&first.to_le_bytes()[..width]);
            header.extend_from_slice(&ext);
            let total = header.len();
            if level == 2 {
                header[..2].copy_from_slice(&(total as u16).to_le_bytes());
            } else {
                header[24..28].copy_from_slice(&(total as u32).to_le_bytes());
            }
            // The common header's CRC field: first extended-header payload.
            let position = if level == 2 { 27 } else { 33 };
            let header_crc = crc16(&header);
            header[position..position + 2].copy_from_slice(&header_crc.to_le_bytes());
            [header, entry.payload.clone()].concat()
        }
        other => panic!("fixture level {other} is not a valid LHA header level"),
    }
}

pub(crate) fn archive(entries: &[Entry]) -> Vec<u8> {
    let mut bytes: Vec<u8> = entries.iter().flat_map(encode).collect();
    bytes.push(0);
    bytes
}
