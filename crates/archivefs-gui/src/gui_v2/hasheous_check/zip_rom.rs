//! The SHA-1 of the one cartridge ROM inside a ZIP, for Check Hasheous.
//!
//! Selection reads only the central directory (the DAT engine's bounded
//! [`preflight_zip`]) and applies the same member refusals as zipped-cartridge
//! identity (#333): encrypted, symbolic-link, unsafe or duplicate names refuse
//! the whole archive, and exactly one member may be a ROM that
//! [`supported_loose_rom_format`] accepts for the game's system. Anything else
//! (no ROM, several ROMs) is refused rather than picked.
//!
//! Hashing is the DAT engine's own [`ZipArchiveSource`] pass: streamed in
//! chunks, cancellable, each member checked against its stored CRC, bounded
//! by member, archive and compression-ratio limits (tightened here to
//! cartridge sizes), and stopped if the ZIP changes while it is read. Nothing
//! is extracted to disk. The hash is of the member's bytes as stored: no
//! header is removed or added, so it is the same whole-file identity a loose
//! ROM gets.
use archivefs_core::dat::archive::limits::ArchiveLimits;
use archivefs_core::dat::archive::zip::ZipArchiveSource;
use archivefs_core::dat::archive::zip_preflight::{ZipPreflightError, preflight_zip};
use archivefs_core::dat::archive::{
    ArchiveMemberSource, ArchiveMemberStatus, ArchivePassCompletion, ArchivePassStopReason,
    ArchiveRunBudget,
};
use archivefs_core::game_identity::{IdentityPlatform, supported_loose_rom_format};
use archivefs_core::safe_read::{TrustedRoots, open_bounded_read};
use std::path::Path;
use std::sync::atomic::AtomicBool;

/// Largest ROM member read for a lookup: the loose cartridge ROM ceiling.
pub(super) const MAX_ROM_BYTES: u64 = archivefs_core::game_identity::MAX_LOOSE_ROM_BYTES;
/// Every member is hashed by the pass, so the whole ZIP is bounded too.
pub(super) const MAX_ZIP_LOGICAL_BYTES: u64 = 4 * MAX_ROM_BYTES;
pub(super) const MAX_ZIP_MEMBERS: usize = 64;

pub(super) const NO_ROM: &str =
    "This ZIP holds no ROM for this game's system, so nothing was sent.";
pub(super) const SEVERAL_ROMS: &str =
    "This ZIP holds more than one ROM. EmuWiz does not pick one, so nothing was sent.";
pub(super) const UNSAFE_ENTRY: &str = "This ZIP has an encrypted, linked, duplicate or unsafely named entry, so it was not read and nothing was sent.";
pub(super) const DAMAGED: &str =
    "This ZIP is damaged or failed its integrity check, so nothing was sent.";
pub(super) const TOO_LARGE: &str =
    "This ZIP is larger than EmuWiz reads for a cartridge lookup, so nothing was sent.";

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ZipRom {
    pub sha1: String,
    pub member: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ZipRefusal {
    Cancelled,
    /// The ZIP changed while it was being read.
    Changed,
    Refused(String),
}

fn limits() -> ArchiveLimits {
    ArchiveLimits {
        max_members: MAX_ZIP_MEMBERS,
        max_member_logical_bytes: MAX_ROM_BYTES,
        max_archive_logical_bytes: MAX_ZIP_LOGICAL_BYTES,
        ..ArchiveLimits::default()
    }
}

/// Unix symbolic-link entries, as recorded by Unix ZIP tools.
fn is_symlink(external_attributes: u32) -> bool {
    (external_attributes >> 16) & 0o170000 == 0o120000
}

fn safe_name(raw: &[u8]) -> Option<&str> {
    let name = std::str::from_utf8(raw).ok()?;
    let unsafe_name = name.is_empty()
        || name.len() > 4096
        || name.contains('\0')
        || name.contains('\\')
        || name.starts_with('/')
        || name.split('/').any(|part| part == "..");
    (!unsafe_name).then_some(name)
}

fn refused(text: &str) -> ZipRefusal {
    ZipRefusal::Refused(text.to_string())
}

/// Selects the ZIP's single ROM for `platform` and returns its SHA-1.
pub(super) fn sole_rom_sha1(
    path: &Path,
    platform: &str,
    roots: &TrustedRoots,
    cancel: &AtomicBool,
) -> Result<ZipRom, ZipRefusal> {
    let platform = IdentityPlatform::from_catalogue(Some(platform));
    let limits = limits();

    // 1. Choose the member from the central directory alone.
    let safe = open_bounded_read(path, roots).map_err(|error| {
        ZipRefusal::Refused(format!(
            "This ZIP could not be opened, so nothing was sent: {}",
            error.detail()
        ))
    })?;
    let len = safe.len();
    let mut file = safe.into_file();
    let preflight =
        preflight_zip(&mut file, len, &limits, cancel).map_err(|error| match error {
            ZipPreflightError::Cancelled => ZipRefusal::Cancelled,
            ZipPreflightError::Refused("member count" | "central directory size") => {
                refused(TOO_LARGE)
            }
            ZipPreflightError::Refused(_) => refused(DAMAGED),
            ZipPreflightError::Corrupt(_) => refused(DAMAGED),
        })?;
    drop(file);
    let mut names = std::collections::HashSet::new();
    let mut candidates = Vec::new();
    for entry in &preflight.entries {
        let Some(name) = safe_name(&entry.name_raw) else {
            return Err(refused(UNSAFE_ENTRY));
        };
        if entry.flags & 1 != 0
            || is_symlink(entry.external_attributes)
            || !names.insert(name.to_ascii_lowercase())
        {
            return Err(refused(UNSAFE_ENTRY));
        }
        if entry.is_directory {
            continue;
        }
        if supported_loose_rom_format(Path::new(name), platform).is_some() {
            candidates.push(entry.name_raw.clone());
        }
    }
    let chosen = match candidates.len() {
        1 => candidates.remove(0),
        0 => return Err(refused(NO_ROM)),
        _ => return Err(refused(SEVERAL_ROMS)),
    };

    // 2. Hash with the DAT engine's bounded, CRC-checked member pass.
    let mut source =
        ZipArchiveSource::open(path, roots, limits, cancel).map_err(|_| refused(DAMAGED))?;
    let mut budget = ArchiveRunBudget::new(MAX_ZIP_LOGICAL_BYTES);
    let pass = source.verify_all(cancel, &mut budget);
    if let ArchivePassCompletion::Incomplete { reason } = &pass.completion {
        return Err(match reason {
            ArchivePassStopReason::Cancelled => ZipRefusal::Cancelled,
            ArchivePassStopReason::OuterFileChanged => ZipRefusal::Changed,
            ArchivePassStopReason::RunLogicalBudget => refused(TOO_LARGE),
            _ => refused(DAMAGED),
        });
    }
    // Whole-archive integrity: any damaged or encrypted member refuses it.
    if pass.members.iter().any(|member| {
        matches!(
            member.status,
            ArchiveMemberStatus::Corrupt { .. } | ArchiveMemberStatus::Encrypted
        )
    }) {
        return Err(refused(DAMAGED));
    }
    let member = pass
        .members
        .iter()
        .find(|member| member.member_name_raw == chosen)
        .ok_or_else(|| refused(DAMAGED))?;
    match (&member.status, &member.hashes) {
        (ArchiveMemberStatus::HashComplete, Some(hashes)) => Ok(ZipRom {
            sha1: hashes.sha1.to_ascii_lowercase(),
            member: member.member_name_display.clone(),
        }),
        (ArchiveMemberStatus::RefusedLimits { .. }, _) => Err(refused(TOO_LARGE)),
        (ArchiveMemberStatus::EmptyFile, _) => Err(refused(NO_ROM)),
        (ArchiveMemberStatus::UnsupportedCodec { .. }, _) => Err(ZipRefusal::Refused(
            "This ZIP uses a compression method EmuWiz cannot read, so nothing was sent."
                .to_string(),
        )),
        _ => Err(refused(DAMAGED)),
    }
}
