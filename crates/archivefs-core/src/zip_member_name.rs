//! One fail-closed path for ZIP member names.
//!
//! zip 9 decodes a member name on demand, so `ZipFile::name()` is fallible
//! (it was infallible in zip 8). Every caller that reads a name goes through
//! this module: an undecodable name is an error for that member and is never
//! replaced by a lossy or empty name, so it can neither match a trusted name
//! nor become a path.
//!
//! zip 9 decodes a name as UTF-8 when the bytes are valid UTF-8, otherwise as
//! CP437, ignoring general-purpose flag bit 11. zip 8.6 followed the flag
//! (bit set: lossy UTF-8; clear: always CP437). Pure-ASCII names and names
//! that were flag-clear and not UTF-8 decode identically. Two classes now
//! differ from earlier runs: flag-clear valid non-ASCII UTF-8 (formerly
//! CP437 mojibake, now the real name) and flag-set invalid UTF-8 (formerly
//! U+FFFD-lossy, now CP437). Names recorded earlier for those classes fail
//! closed as stale rather than matching.

use std::{borrow::Cow, fmt};

use zip::result::ZipResult;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipMemberNameError {
    detail: String,
}

impl fmt::Display for ZipMemberNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ZIP member name cannot be decoded: {}", self.detail)
    }
}

impl std::error::Error for ZipMemberNameError {}

impl From<ZipMemberNameError> for std::io::Error {
    fn from(error: ZipMemberNameError) -> Self {
        std::io::Error::new(std::io::ErrorKind::InvalidData, error)
    }
}

/// Pass `file.name()`; get the name or a refusal.
pub fn checked(name: ZipResult<Cow<'_, str>>) -> Result<Cow<'_, str>, ZipMemberNameError> {
    name.map_err(|error| ZipMemberNameError {
        detail: error.to_string(),
    })
}

/// [`checked`], owned.
pub fn owned(name: ZipResult<Cow<'_, str>>) -> Result<String, ZipMemberNameError> {
    checked(name).map(Cow::into_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

    fn archive_with(name: &str) -> ZipArchive<Cursor<Vec<u8>>> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(name, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"x").unwrap();
        ZipArchive::new(Cursor::new(writer.finish().unwrap().into_inner())).unwrap()
    }

    #[test]
    fn decodable_names_are_unchanged() {
        let mut archive = archive_with("Café/Tetris (World).gb");
        let member = archive.by_index(0).unwrap();
        assert_eq!(owned(member.name()).unwrap(), "Café/Tetris (World).gb");
    }

    /// Writes `name` (ASCII placeholder bytes patched by the caller) and sets or
    /// clears general-purpose flag bit 11 in the local and central headers.
    fn archive_bytes(name: &str, utf8_flag: bool) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(name, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"x").unwrap();
        let mut bytes = writer.finish().unwrap().into_inner();
        let mut headers = 0;
        for at in 0..bytes.len() - 10 {
            let flag_at = match &bytes[at..at + 4] {
                b"PK\x03\x04" => at + 7,
                b"PK\x01\x02" => at + 9,
                _ => continue,
            };
            if utf8_flag {
                bytes[flag_at] |= 0x08;
            } else {
                bytes[flag_at] &= !0x08;
            }
            headers += 1;
        }
        assert_eq!(headers, 2);
        bytes
    }

    fn replace_name_bytes(bytes: &mut [u8], from: &[u8], to: &[u8]) {
        let mut patched = 0;
        for at in 0..bytes.len() - from.len() {
            if &bytes[at..at + from.len()] == from {
                bytes[at..at + from.len()].copy_from_slice(to);
                patched += 1;
            }
        }
        assert_eq!(patched, 2);
    }

    #[test]
    fn flag_clear_valid_utf8_is_the_real_name_not_cp437_mojibake() {
        // zip 8.6 gave "CafÃ©" here; zip 9 gives the name that was written.
        let bytes = archive_bytes("Café.gb", false);
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let member = archive.by_index(0).unwrap();
        assert_eq!(owned(member.name()).unwrap(), "Café.gb");
    }

    #[test]
    fn flag_set_invalid_utf8_is_cp437_with_no_replacement_character() {
        // zip 8.6 gave a lossy U+FFFD name here, which could collide.
        let mut bytes = archive_bytes("CuraXao.gb", true);
        replace_name_bytes(&mut bytes, b"CuraXao", b"Cura\x87ao");
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let member = archive.by_index(0).unwrap();
        let name = owned(member.name()).unwrap();
        assert_eq!(name, "Curaçao.gb");
        assert!(!name.contains('\u{fffd}'));
    }

    #[test]
    fn non_utf8_names_decode_as_cp437_exactly_as_before() {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("CuraXao.gb", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"x").unwrap();
        let mut bytes = writer.finish().unwrap().into_inner();
        // Local and central headers both carry the name; make it non-UTF-8.
        let mut patched = 0;
        for at in 0..bytes.len() - 6 {
            if &bytes[at..at + 6] == b"CuraXa" {
                bytes[at + 4] = 0x87;
                patched += 1;
            }
        }
        assert_eq!(patched, 2);
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let member = archive.by_index(0).unwrap();
        assert_eq!(owned(member.name()).unwrap(), "Curaçao.gb");
    }

    #[test]
    fn a_decode_failure_is_an_error_never_a_substitute_name() {
        let failure: ZipResult<Cow<'_, str>> = Err(zip::result::ZipError::Io(
            std::io::Error::other("Cannot translate path from cp437"),
        ));
        let error = checked(failure).unwrap_err();
        assert!(error.to_string().contains("cannot be decoded"));
        let io: std::io::Error = error.into();
        assert_eq!(io.kind(), std::io::ErrorKind::InvalidData);
    }
}
