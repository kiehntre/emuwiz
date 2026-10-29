//! Bounded platform recognition inside ZIP members. No member name is used as
//! a filesystem path, game title, or DAT claim.

use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::tape_identity::{TapeInspection, ZxTapBlockKind};

const MAX_MEMBERS: usize = 2_000;
const MAX_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_MEMBER_BYTES: u64 = 8 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

/// An incomplete pass must not withdraw a previous platform assignment.
pub(crate) enum MemberPlatformObservation {
    Complete(Option<&'static str>),
    Incomplete(String),
}

pub(crate) fn observe_zip_platform(path: &Path) -> MemberPlatformObservation {
    observe_zip_platform_in_temp_dir(path, &std::env::temp_dir())
}

fn observe_zip_platform_in_temp_dir(path: &Path, temp_dir: &Path) -> MemberPlatformObservation {
    let safe =
        match crate::safe_read::open_bounded_read(path, &crate::safe_read::TrustedRoots::none()) {
            Ok(file) => file,
            Err(error) => {
                return MemberPlatformObservation::Incomplete(format!(
                    "archive open refused: {error:?}"
                ));
            }
        };
    if safe.len() > MAX_ARCHIVE_BYTES {
        return MemberPlatformObservation::Incomplete(
            "ZIP container exceeds inspection limit".into(),
        );
    }
    let file = safe.into_file();
    let opened = match file.metadata() {
        Ok(metadata) => metadata,
        Err(error) => {
            return MemberPlatformObservation::Incomplete(format!(
                "ZIP metadata unreadable: {error}"
            ));
        }
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(archive) => archive,
        Err(error) => {
            return MemberPlatformObservation::Incomplete(format!(
                "ZIP directory unreadable: {error}"
            ));
        }
    };
    if archive.len() > MAX_MEMBERS {
        return MemberPlatformObservation::Incomplete(
            "ZIP member count exceeds inspection limit".into(),
        );
    }
    let mut platform = None;
    let mut total = 0_u64;
    let mut ambiguous = false;
    let mut shared_tape = false;
    for index in 0..archive.len() {
        let (name, size, symlink, directory, encrypted) = match archive.by_index_raw(index) {
            Ok(member) => (
                match crate::zip_member_name::owned(member.name()) {
                    Ok(name) => name,
                    Err(error) => {
                        return MemberPlatformObservation::Incomplete(format!(
                            "ZIP member name unreadable: {error}"
                        ));
                    }
                },
                member.size(),
                member
                    .unix_mode()
                    .is_some_and(|mode| mode & 0o170000 == 0o120000),
                member.is_dir(),
                member.encrypted(),
            ),
            Err(error) => {
                return MemberPlatformObservation::Incomplete(format!(
                    "ZIP member metadata unreadable: {error}"
                ));
            }
        };
        if directory {
            continue;
        }
        if symlink {
            return MemberPlatformObservation::Incomplete("ZIP contains a symlink member".into());
        }
        let extension = Path::new(&name)
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(
            extension.as_str(),
            "tap" | "z80" | "sna" | "adf" | "tzx" | "cdt"
        ) {
            continue;
        }
        if encrypted || size > MAX_MEMBER_BYTES || total.saturating_add(size) > MAX_TOTAL_BYTES {
            return MemberPlatformObservation::Incomplete(
                "ZIP candidate exceeds bounded inspection or is encrypted".into(),
            );
        }
        let mut member = match archive.by_index(index) {
            Ok(member) => member,
            Err(error) => {
                return MemberPlatformObservation::Incomplete(format!(
                    "ZIP member unreadable: {error}"
                ));
            }
        };
        let mut bytes = Vec::with_capacity(size as usize);
        if let Err(error) = (&mut member).take(size + 1).read_to_end(&mut bytes) {
            return MemberPlatformObservation::Incomplete(format!(
                "ZIP member decode failed: {error}"
            ));
        }
        if bytes.len() as u64 != size {
            return MemberPlatformObservation::Incomplete("ZIP member extent changed".into());
        }
        total += size;
        let found = match extension.as_str() {
            "tap" => match crate::tape_identity::inspect_tap(&bytes) {
                Ok(TapeInspection::ZxSpectrum(tape))
                    if tape
                        .blocks
                        .iter()
                        .any(|block| match (&block.kind, &block.header) {
                            (ZxTapBlockKind::Header, Some(header)) => header.file_type <= 3,
                            (ZxTapBlockKind::Data, _) => block.length >= 16,
                            _ => false,
                        }) =>
                {
                    Some("ZX Spectrum")
                }
                _ => None,
            },
            "z80" if crate::zx_spectrum_snapshot::parse_z80_snapshot(&bytes).is_ok() => {
                Some("ZX Spectrum")
            }
            "sna" if crate::zx_spectrum_snapshot::parse_sna_snapshot(&bytes).is_ok() => {
                Some("ZX Spectrum")
            }
            "adf" => match crate::amiga_adz::inspect_adf_member_bytes_in_temp_dir(&bytes, temp_dir)
            {
                Ok(inspection) => inspection.filesystem().map(|_| "Amiga"),
                Err(
                    crate::amiga_adz::AdzError::TempFileUnavailable(detail)
                    | crate::amiga_adz::AdzError::Io(detail),
                ) => {
                    return MemberPlatformObservation::Incomplete(format!(
                        "ADF staging failed: {detail}"
                    ));
                }
                Err(crate::amiga_adz::AdzError::NotAdf(
                    crate::amiga_disk::AmigaFloppyError::Container(
                        crate::amiga_disk::DiskError::Io(_)
                        | crate::amiga_disk::DiskError::ChangedWhileReading,
                    )
                    | crate::amiga_disk::AmigaFloppyError::Filesystem(
                        crate::amiga_disk::FilesystemError::Io(_)
                        | crate::amiga_disk::FilesystemError::Cancelled,
                    ),
                )) => {
                    return MemberPlatformObservation::Incomplete(
                        "ADF inspection stopped before validation".into(),
                    );
                }
                Err(_) => None,
            },
            "tzx" | "cdt" => {
                shared_tape |= if extension == "tzx" {
                    crate::tape_identity::parse_tzx(&bytes).is_ok()
                } else {
                    crate::tape_identity::parse_cdt(&bytes).is_ok()
                };
                None
            }
            _ => None,
        };
        if let Some(found) = found {
            if platform.is_some_and(|previous| previous != found) {
                ambiguous = true;
            }
            platform = Some(found);
        }
    }
    let same_file = archive
        .into_inner()
        .metadata()
        .ok()
        .zip(std::fs::symlink_metadata(path).ok())
        .is_some_and(|(descriptor, named)| {
            named.is_file()
                && descriptor.dev() == opened.dev()
                && descriptor.ino() == opened.ino()
                && named.dev() == opened.dev()
                && named.ino() == opened.ino()
                && descriptor.len() == opened.len()
                && named.len() == opened.len()
                && descriptor.ctime() == opened.ctime()
                && descriptor.ctime_nsec() == opened.ctime_nsec()
                && named.ctime() == opened.ctime()
                && named.ctime_nsec() == opened.ctime_nsec()
                && descriptor.modified().ok() == opened.modified().ok()
                && named.modified().ok() == opened.modified().ok()
        });
    if !same_file {
        return MemberPlatformObservation::Incomplete(
            "ZIP changed during member inspection".into(),
        );
    }
    MemberPlatformObservation::Complete(if ambiguous || shared_tape {
        None
    } else {
        platform
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn adf_staging_failure_keeps_member_inspection_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("disk.zip");
        let file = std::fs::File::create(&archive_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("disk.adf", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&crate::synthetic_media::amiga_adf(1, b"TEST"))
            .unwrap();
        zip.finish().unwrap();
        let unusable_temp = dir.path().join("ordinary-file");
        std::fs::write(&unusable_temp, b"not a directory").unwrap();
        assert!(matches!(
            observe_zip_platform_in_temp_dir(&archive_path, &unusable_temp),
            MemberPlatformObservation::Incomplete(_)
        ));
    }
}
