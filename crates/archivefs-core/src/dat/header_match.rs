//! S03-05: header-aware DAT matching for NES, Atari Lynx and Atari 7800.
//!
//! No-Intro catalogues for these platforms describe the *headerless* ROM, so a
//! correctly dumped file that still carries its iNES / Lynx / 7800 header never
//! matches by whole-file hash. This module lets the combined audit compare the
//! bytes *after* a verified header instead, without ever weakening what a
//! match means:
//!
//! - The original file is only read, never modified. The whole-file identity
//!   (size, CRC32, MD5, SHA-1 and the freshness fingerprint) stays recorded
//!   exactly as before; the headerless hashes are recorded *separately* in a
//!   [`HeaderRemovalMatch`].
//! - A header is stripped only after its format is verified: the magic must
//!   belong to the one platform the catalogue was assigned, no other known
//!   header magic may also match, and the declared payload length must equal
//!   the actual payload length. Anything else is rejected, not guessed at.
//! - The catalogue must have been assigned the platform explicitly, and the
//!   file's extension must be an extension of that same platform. A DAT of
//!   another platform can never claim a headerless match.
//! - The result is presented as "Matched after header removal", never as an
//!   ordinary whole-file match.
//!
//! The header routines themselves ([`crate::header_normalization`]) are reused;
//! this module only adds the length verification and the bounded hashing.

use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use crate::header_normalization::{
    HeaderNormalizationKind, recognize_atari7800, recognize_fds, recognize_ines, recognize_lynx,
};
use crate::identity_source::hashing::{
    Crc32, FileFingerprint, HASH_CHUNK_BYTES, MAX_AUTOMATIC_HASH_BYTES,
};
use crate::platform::{platform_candidates_for_extension, platform_for_alias};
use crate::safe_read::{TrustedRoots, open_bounded_read};

/// The display text a header-removal match is always presented with.
pub const MATCHED_AFTER_HEADER_REMOVAL: &str = "Matched after header removal";

/// What a header-removal match records, separately from the whole-file
/// identity held alongside it in `AuditedFileHashes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HeaderRemovalMatch {
    /// The canonical platform the catalogue was assigned.
    pub platform: String,
    /// The stable transform identifier (for example `ines-header-strip-16`).
    pub transform_id: String,
    pub header_label: String,
    pub header_len: u64,
    pub headerless_size: u64,
    pub headerless_crc32: String,
    pub headerless_md5: String,
    pub headerless_sha1: String,
}

impl HeaderRemovalMatch {
    /// One sentence for a person: what was removed and that the file itself
    /// was not changed.
    pub fn explanation(&self) -> String {
        format!(
            "{MATCHED_AFTER_HEADER_REMOVAL}: the {} ({} bytes) was ignored when comparing; \
             the file itself is unchanged and its whole-file identity is kept",
            self.header_label, self.header_len
        )
    }
}

/// Why a header was not accepted. Every variant fails closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderRejection {
    /// The file is shorter than the header it would need to carry.
    TooShort,
    /// The expected magic is not there.
    MagicMissing,
    /// More than one known header magic matches; the format is not certain.
    AmbiguousMagic,
    /// The header declares a payload size the file does not have.
    LengthMismatch { declared: u64, actual: u64 },
    /// The header declares nothing usable (for example zero PRG banks).
    Malformed,
    /// A header revision this module does not interpret (NES 2.0).
    UnsupportedRevision,
}

impl HeaderRejection {
    pub fn detail(&self) -> String {
        match self {
            Self::TooShort => "the file is shorter than its header".to_string(),
            Self::MagicMissing => "the header magic is not present".to_string(),
            Self::AmbiguousMagic => "more than one known header signature matches".to_string(),
            Self::LengthMismatch { declared, actual } => {
                format!("the header declares {declared} bytes but {actual} bytes follow it")
            }
            Self::Malformed => "the header declares no usable content".to_string(),
            Self::UnsupportedRevision => "this header revision is not interpreted".to_string(),
        }
    }
}

/// The header transform for a catalogue's assigned platform, if that platform
/// is one of the three this module handles. `None` for every other platform:
/// an unassigned or unrelated catalogue can never take part.
pub fn header_kind_for_platform(platform: &str) -> Option<HeaderNormalizationKind> {
    let id = match platform {
        "NES" | "Atari Lynx" | "Atari7800" => platform,
        other => platform_for_alias(other)?.id,
    };
    match id {
        "NES" => Some(HeaderNormalizationKind::INes16),
        "Atari Lynx" => Some(HeaderNormalizationKind::Lynx64),
        "Atari7800" => Some(HeaderNormalizationKind::Atari7800_128),
        _ => None,
    }
}

fn platform_id_for_kind(kind: HeaderNormalizationKind) -> &'static str {
    match kind {
        HeaderNormalizationKind::INes16 => "NES",
        HeaderNormalizationKind::Lynx64 => "Atari Lynx",
        HeaderNormalizationKind::Atari7800_128 => "Atari7800",
        // Never produced by `header_kind_for_platform`.
        HeaderNormalizationKind::Fds16 | HeaderNormalizationKind::SnesCopier512 => "",
    }
}

/// Whether `path`'s extension is one the kind's own platform lists. Extension
/// is corroboration only; it never identifies anything by itself.
pub fn extension_fits_kind(path: &Path, kind: HeaderNormalizationKind) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    let platform = platform_id_for_kind(kind);
    platform_candidates_for_extension(&extension.to_ascii_lowercase()).contains(&platform)
}

fn magic_matches(prefix: &[u8], kind: HeaderNormalizationKind) -> bool {
    match kind {
        HeaderNormalizationKind::INes16 => recognize_ines(prefix),
        HeaderNormalizationKind::Fds16 => recognize_fds(prefix),
        HeaderNormalizationKind::Lynx64 => recognize_lynx(prefix),
        HeaderNormalizationKind::Atari7800_128 => recognize_atari7800(prefix),
        HeaderNormalizationKind::SnesCopier512 => false,
    }
}

/// Verifies a header: the magic, that no other signature also matches, and
/// that the declared payload length equals `payload_len`. `prefix` must hold
/// at least the header's own bytes.
pub fn validate_header(
    prefix: &[u8],
    payload_len: u64,
    kind: HeaderNormalizationKind,
) -> Result<(), HeaderRejection> {
    if prefix.len() < kind.header_len() {
        return Err(HeaderRejection::TooShort);
    }
    if !magic_matches(prefix, kind) {
        return Err(HeaderRejection::MagicMissing);
    }
    let competing = [
        HeaderNormalizationKind::INes16,
        HeaderNormalizationKind::Fds16,
        HeaderNormalizationKind::Lynx64,
        HeaderNormalizationKind::Atari7800_128,
    ];
    if competing
        .iter()
        .any(|other| *other != kind && magic_matches(prefix, *other))
    {
        return Err(HeaderRejection::AmbiguousMagic);
    }
    let declared = match kind {
        HeaderNormalizationKind::INes16 => {
            // NES 2.0 encodes sizes differently; do not guess.
            if (prefix[7] >> 2) & 0b11 == 0b10 {
                return Err(HeaderRejection::UnsupportedRevision);
            }
            let prg = u64::from(prefix[4]) * 16 * 1024;
            let chr = u64::from(prefix[5]) * 8 * 1024;
            let trainer = if prefix[6] & 0b100 != 0 { 512 } else { 0 };
            if prg == 0 {
                return Err(HeaderRejection::Malformed);
            }
            trainer + prg + chr
        }
        HeaderNormalizationKind::Lynx64 => {
            let bank0 = u64::from(u16::from_le_bytes([prefix[4], prefix[5]]));
            let bank1 = u64::from(u16::from_le_bytes([prefix[6], prefix[7]]));
            let declared = (bank0 + bank1) * 256;
            if declared == 0 {
                return Err(HeaderRejection::Malformed);
            }
            declared
        }
        HeaderNormalizationKind::Atari7800_128 => {
            let declared = u64::from(u32::from_be_bytes([
                prefix[49], prefix[50], prefix[51], prefix[52],
            ]));
            if declared == 0 {
                return Err(HeaderRejection::Malformed);
            }
            declared
        }
        // Not handled by this module.
        HeaderNormalizationKind::Fds16 | HeaderNormalizationKind::SnesCopier512 => {
            return Err(HeaderRejection::MagicMissing);
        }
    };
    if declared != payload_len {
        return Err(HeaderRejection::LengthMismatch {
            declared,
            actual: payload_len,
        });
    }
    Ok(())
}

/// Why a headerless hash could not be produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderHashOutcome {
    Hashed(HeaderRemovalMatch),
    Rejected(HeaderRejection),
    /// The file changed, could not be opened by the shared read policy, or the
    /// run was cancelled. Nothing is claimed.
    Refused,
}

/// Hashes the bytes after `kind`'s header, bound to `audited`: the file must
/// still be the exact object the audit fingerprinted, before and after the
/// read, so the headerless hashes describe the same bytes the whole-file
/// identity does. Read-only; opens through the shared bounded-read policy.
pub fn hash_after_header(
    path: &Path,
    trusted: &TrustedRoots,
    cancel: &AtomicBool,
    kind: HeaderNormalizationKind,
    audited: &FileFingerprint,
) -> HeaderHashOutcome {
    use md5::Md5;
    use sha1::Sha1;
    use sha1::digest::Digest;

    let header_len = kind.header_len();
    let Some(before) = FileFingerprint::observe(path) else {
        return HeaderHashOutcome::Refused;
    };
    if &before != audited || before.size_bytes > MAX_AUTOMATIC_HASH_BYTES {
        return HeaderHashOutcome::Refused;
    }
    if before.size_bytes < header_len as u64 {
        return HeaderHashOutcome::Rejected(HeaderRejection::TooShort);
    }
    let Ok(file) = open_bounded_read(path, trusted) else {
        return HeaderHashOutcome::Refused;
    };
    let mut reader = file.into_file();
    #[cfg(unix)]
    {
        let Ok(opened) = reader.metadata() else {
            return HeaderHashOutcome::Refused;
        };
        if !before
            .object
            .is_some_and(|stamp| stamp.same_object(&opened))
        {
            return HeaderHashOutcome::Refused;
        }
    }
    let mut header = vec![0_u8; header_len];
    if reader.read_exact(&mut header).is_err() {
        return HeaderHashOutcome::Refused;
    }
    let payload_len = before.size_bytes - header_len as u64;
    if let Err(rejection) = validate_header(&header, payload_len, kind) {
        return HeaderHashOutcome::Rejected(rejection);
    }

    let mut crc = Crc32::new();
    let mut md5 = Md5::new();
    let mut sha1 = Sha1::new();
    let mut buffer = vec![0_u8; HASH_CHUNK_BYTES];
    let mut total = 0_u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return HeaderHashOutcome::Refused;
        }
        let Ok(read) = reader.read(&mut buffer) else {
            return HeaderHashOutcome::Refused;
        };
        if read == 0 {
            break;
        }
        let chunk = &buffer[..read];
        crc.update(chunk);
        md5.update(chunk);
        sha1.update(chunk);
        total += read as u64;
    }
    if total != payload_len || FileFingerprint::observe(path).as_ref() != Some(&before) {
        return HeaderHashOutcome::Refused;
    }
    HeaderHashOutcome::Hashed(HeaderRemovalMatch {
        platform: platform_id_for_kind(kind).to_string(),
        transform_id: kind.transform_id().to_string(),
        header_label: kind.label().to_string(),
        header_len: header_len as u64,
        headerless_size: payload_len,
        headerless_crc32: crc.finish_hex(),
        headerless_md5: hex(&md5.finalize()),
        headerless_sha1: hex(&sha1.finalize()),
    })
}

/// The one member of a ZIP archive whose already-verified decoded hashes are
/// `member`, hashed again after `kind`'s header. Streams through the shared
/// safe ZIP reader into memory only: nothing is extracted or written, and the
/// archive is checked to be the same object afterwards. The member is selected
/// by its exact name, size, SHA-1 and CRC32 from the completed member pass, so
/// a member that differs from the one audited is refused, never substituted.
pub fn hash_zip_member_after_header(
    archive: &Path,
    member_name: &str,
    member: &crate::dat::archive::ArchiveMemberHashes,
    member_size: u64,
    trusted: &TrustedRoots,
    cancel: &AtomicBool,
    kind: HeaderNormalizationKind,
) -> HeaderHashOutcome {
    use crate::dat::archive::limits::ArchiveLimits;
    use crate::dat::archive::zip::{ZipMemberRequest, stream_zip_member_into};
    let header_len = kind.header_len() as u64;
    if member_size < header_len {
        return HeaderHashOutcome::Rejected(HeaderRejection::TooShort);
    }
    if member_size > MAX_AUTOMATIC_HASH_BYTES {
        return HeaderHashOutcome::Refused;
    }
    let request = ZipMemberRequest {
        member_name: Some(member_name.to_string()),
        require_exact_name: true,
        size_bytes: Some(member_size),
        sha1: Some(member.sha1.clone()),
        crc32: Some(member.crc32.clone()),
    };
    let mut stream = HeaderlessStream::new(kind.header_len());
    if stream_zip_member_into(
        archive,
        trusted,
        &ArchiveLimits::default(),
        cancel,
        &request,
        &mut stream,
    )
    .is_err()
    {
        return HeaderHashOutcome::Refused;
    }
    stream.finish(kind, member_size)
}

/// An in-memory `Write` sink that keeps the first `header_len` bytes and
/// hashes everything after them.
struct HeaderlessStream {
    header_len: usize,
    header: Vec<u8>,
    crc: Crc32,
    md5: md5::Md5,
    sha1: sha1::Sha1,
    payload: u64,
}

impl HeaderlessStream {
    fn new(header_len: usize) -> Self {
        use sha1::digest::Digest;
        Self {
            header_len,
            header: Vec::with_capacity(header_len),
            crc: Crc32::new(),
            md5: md5::Md5::new(),
            sha1: sha1::Sha1::new(),
            payload: 0,
        }
    }

    fn finish(self, kind: HeaderNormalizationKind, total: u64) -> HeaderHashOutcome {
        use sha1::digest::Digest;
        let payload_len = total - self.header_len as u64;
        if self.header.len() != self.header_len || self.payload != payload_len {
            return HeaderHashOutcome::Refused;
        }
        if let Err(rejection) = validate_header(&self.header, payload_len, kind) {
            return HeaderHashOutcome::Rejected(rejection);
        }
        HeaderHashOutcome::Hashed(HeaderRemovalMatch {
            platform: platform_id_for_kind(kind).to_string(),
            transform_id: kind.transform_id().to_string(),
            header_label: kind.label().to_string(),
            header_len: self.header_len as u64,
            headerless_size: payload_len,
            headerless_crc32: self.crc.finish_hex(),
            headerless_md5: hex(&self.md5.finalize()),
            headerless_sha1: hex(&self.sha1.finalize()),
        })
    }
}

impl std::io::Write for HeaderlessStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        use sha1::digest::Digest;
        let take = (self.header_len - self.header.len()).min(buf.len());
        self.header.extend_from_slice(&buf[..take]);
        let rest = &buf[take..];
        if !rest.is_empty() {
            self.crc.update(rest);
            self.md5.update(rest);
            self.sha1.update(rest);
            self.payload += rest.len() as u64;
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// The verdict label presented for a header-removal match.
pub fn headerless_algorithm_label(algorithm: &str) -> &'static str {
    match algorithm {
        "SHA-256" => "SHA-256 after header removal",
        "SHA-1" => "SHA-1 after header removal",
        "MD5" => "MD5 after header removal",
        _ => "hash after header removal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ines(prg: u8, chr: u8, flags6: u8, flags7: u8) -> Vec<u8> {
        let mut header = vec![0_u8; 16];
        header[..4].copy_from_slice(b"NES\x1a");
        header[4] = prg;
        header[5] = chr;
        header[6] = flags6;
        header[7] = flags7;
        header
    }

    const INES: HeaderNormalizationKind = HeaderNormalizationKind::INes16;

    #[test]
    fn ines_trainer_flag_adds_512_bytes_to_the_declared_payload() {
        let header = ines(1, 1, 0b100, 0);
        let declared = 16 * 1024 + 8 * 1024 + 512;
        assert_eq!(validate_header(&header, declared, INES), Ok(()));
        // Flag set but the trainer is absent: refused, not guessed at.
        assert_eq!(
            validate_header(&header, declared - 512, INES),
            Err(HeaderRejection::LengthMismatch {
                declared,
                actual: declared - 512
            })
        );
        // No flag but a trainer-sized surplus: refused as well.
        assert!(matches!(
            validate_header(&ines(1, 1, 0, 0), declared, INES),
            Err(HeaderRejection::LengthMismatch { .. })
        ));
    }

    #[test]
    fn nes2_headers_are_refused_even_when_the_length_would_fit() {
        let header = ines(1, 0, 0, 0b0000_1000);
        assert_eq!(
            validate_header(&header, 16 * 1024, INES),
            Err(HeaderRejection::UnsupportedRevision)
        );
    }

    #[test]
    fn lynx_declared_size_sums_both_banks() {
        let kind = HeaderNormalizationKind::Lynx64;
        let mut header = vec![0_u8; 64];
        header[..4].copy_from_slice(b"LYNX");
        header[4..6].copy_from_slice(&256_u16.to_le_bytes());
        header[6..8].copy_from_slice(&128_u16.to_le_bytes());
        let declared = (256 + 128) * 256;
        assert_eq!(validate_header(&header, declared, kind), Ok(()));
        assert!(matches!(
            validate_header(&header, 256 * 256, kind),
            Err(HeaderRejection::LengthMismatch { .. })
        ));
    }

    #[test]
    fn atari_7800_declared_size_must_equal_the_payload() {
        let kind = HeaderNormalizationKind::Atari7800_128;
        let mut header = vec![0_u8; 128];
        header[1..10].copy_from_slice(b"ATARI7800");
        header[49..53].copy_from_slice(&(32 * 1024_u32).to_be_bytes());
        assert_eq!(validate_header(&header, 32 * 1024, kind), Ok(()));
        assert!(matches!(
            validate_header(&header, 32 * 1024 + 1, kind),
            Err(HeaderRejection::LengthMismatch { .. })
        ));
        header[49..53].copy_from_slice(&0_u32.to_be_bytes());
        assert_eq!(
            validate_header(&header, 0, kind),
            Err(HeaderRejection::Malformed)
        );
    }

    #[test]
    fn only_the_three_platforms_have_a_header_kind() {
        assert_eq!(header_kind_for_platform("NES"), Some(INES));
        assert_eq!(
            header_kind_for_platform("Atari Lynx"),
            Some(HeaderNormalizationKind::Lynx64)
        );
        assert_eq!(
            header_kind_for_platform("Atari7800"),
            Some(HeaderNormalizationKind::Atari7800_128)
        );
        assert_eq!(header_kind_for_platform("Game Boy"), None);
        assert_eq!(header_kind_for_platform("not a platform"), None);
    }
}
