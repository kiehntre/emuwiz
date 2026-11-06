//! The SHA-1 of the one cartridge ROM inside a ZIP, for Check Hasheous.
//!
//! Selection is core's shared [`select_zip_cartridge_member`] (the same rule
//! zipped-cartridge identity and launch staging use), behind the same
//! [`is_loose_cartridge_platform`] guard, so Hasheous accepts, refuses and
//! picks exactly what they do. There is no second selection rule here.
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
use archivefs_core::dat::archive::{
    ArchiveMemberSource, ArchiveMemberStatus, ArchivePassCompletion, ArchivePassStopReason,
    ArchiveRunBudget,
};
use archivefs_core::game_identity::{IdentityPlatform, IdentityStatus};
use archivefs_core::safe_read::{TrustedRoots, open_bounded_read};
use archivefs_core::zip_cartridge_select::{
    ZipCartridgeMember, is_loose_cartridge_platform, select_zip_cartridge_member,
};
use std::path::Path;
use std::sync::atomic::AtomicBool;

/// Largest ROM member read for a lookup: the loose cartridge ROM ceiling.
pub(super) const MAX_ROM_BYTES: u64 = archivefs_core::game_identity::MAX_LOOSE_ROM_BYTES;
/// Every member is hashed by the pass, so the whole ZIP is bounded too.
pub(super) const MAX_ZIP_LOGICAL_BYTES: u64 = 4 * MAX_ROM_BYTES;
pub(super) const MAX_ZIP_MEMBERS: usize = 64;

pub(super) const NOT_CARTRIDGE: &str =
    "EmuWiz reads a ZIP here only for cartridge systems, so nothing was sent.";
pub(super) const NO_ROM: &str =
    "This ZIP holds no ROM for this game's system, so nothing was sent.";
pub(super) const SEVERAL_ROMS: &str = "This ZIP holds more than one ROM, or entries with the same name. EmuWiz does not pick one, so nothing was sent.";
pub(super) const UNSAFE_ENTRY: &str = "This ZIP has an encrypted, linked or unsafely named entry, so it was not read and nothing was sent.";
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

fn refused(text: &str) -> ZipRefusal {
    ZipRefusal::Refused(text.to_string())
}

fn limits() -> ArchiveLimits {
    ArchiveLimits {
        max_members: MAX_ZIP_MEMBERS,
        max_member_logical_bytes: MAX_ROM_BYTES,
        max_archive_logical_bytes: MAX_ZIP_LOGICAL_BYTES,
        ..ArchiveLimits::default()
    }
}

/// Core's shared selection for `path`, read under the normal read policy.
pub(super) fn select(
    path: &Path,
    platform: &str,
    roots: &TrustedRoots,
) -> Result<ZipCartridgeMember, ZipRefusal> {
    let platform = IdentityPlatform::from_catalogue(Some(platform));
    if !is_loose_cartridge_platform(platform) {
        return Err(refused(NOT_CARTRIDGE));
    }
    let file = open_bounded_read(path, roots)
        .map_err(|error| {
            ZipRefusal::Refused(format!(
                "This ZIP could not be opened, so nothing was sent: {}",
                error.detail()
            ))
        })?
        .into_file();
    let mut archive = zip::ZipArchive::new(file).map_err(|_| refused(DAMAGED))?;
    let mut seen = 0;
    select_zip_cartridge_member(&mut archive, platform, &mut seen).map_err(|(status, _)| {
        refused(match status {
            IdentityStatus::Missing => NO_ROM,
            IdentityStatus::Ambiguous => SEVERAL_ROMS,
            IdentityStatus::Invalid | IdentityStatus::Unsupported => UNSAFE_ENTRY,
            IdentityStatus::ResourceLimitReached => TOO_LARGE,
            _ => DAMAGED,
        })
    })
}

/// Selects the ZIP's single ROM for `platform` and returns its SHA-1.
pub(super) fn sole_rom_sha1(
    path: &Path,
    platform: &str,
    roots: &TrustedRoots,
    cancel: &AtomicBool,
) -> Result<ZipRom, ZipRefusal> {
    let cancelled = || cancel.load(std::sync::atomic::Ordering::SeqCst);
    if cancelled() {
        return Err(ZipRefusal::Cancelled);
    }
    let chosen = select(path, platform, roots)?;
    let mut source = ZipArchiveSource::open(path, roots, limits(), cancel).map_err(|_| {
        if cancelled() {
            ZipRefusal::Cancelled
        } else {
            refused(DAMAGED)
        }
    })?;
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
    // The hashed member must be the selected one: same index and exact name.
    let member = pass
        .members
        .iter()
        .find(|member| member.index == chosen.index && member.member_name_raw == chosen.name)
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
