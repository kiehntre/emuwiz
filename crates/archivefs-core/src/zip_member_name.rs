//! One fail-closed path for ZIP member names.
//!
//! zip 9 decodes a member name on demand, so `ZipFile::name()` is fallible
//! (it was infallible in zip 8). Every caller that reads a name goes through
//! this module: an undecodable name is an error for that member and is never
//! replaced by a lossy or empty name, so it can neither match a trusted name
//! nor become a path. Decoding itself is unchanged (UTF-8, else CP437), so
//! names that were valid before decode to the same string.

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
