//! Launch-time extraction of the single verified ROM member of a zipped
//! cartridge. Kept apart from `game_identity`, whose production code must stay
//! read-only; this module writes only inside a fresh private temporary
//! directory and never touches the ZIP.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use zip::ZipArchive;

use crate::game_identity::{
    IdentityPlatform, StableFileMetadata, open_read_only_regular, still_same_file,
};
use crate::safe_read::TrustedRoots;
use crate::zip_cartridge_select::{is_loose_cartridge_platform, select_zip_cartridge_member};

/// The cartridge format whose member rules stage `platform_id`'s ZIPs. A
/// Sega Pico ROM is a Mega Drive-format cartridge (same header layout, `.md`
/// member), so its ZIP is staged under the Mega Drive member rules; the
/// launch itself stays Sega Pico, and the Pico-versus-Mega Drive decision is
/// the separate header check (`launch::unverified_media::is_sega_pico_rom`).
fn staging_platform(platform_id: &str) -> IdentityPlatform {
    if platform_id == crate::launch::unverified_media::SEGA_PICO_PLATFORM {
        return IdentityPlatform::MegaDrive;
    }
    IdentityPlatform::from_catalogue(Some(platform_id))
}

/// Whether a launch request is for a zipped cartridge, i.e. staging applies:
/// a `.zip` on a loose-cartridge platform, or an Amiga `.zip` (a floppy game
/// whose IPF disks are staged as one set, `zip_floppy_set`). Arcade, disc
/// and other platforms' ZIPs are native content and never match.
pub fn is_zipped_cartridge_request(content: &Path, platform_id: &str) -> bool {
    let platform = staging_platform(platform_id);
    content
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("zip"))
        && (is_loose_cartridge_platform(platform) || platform == IdentityPlatform::Amiga)
}

/// Whether `zip` holds exactly one ROM member that `platform_id`'s zipped
/// cartridge staging would accept: `None` when staging does not apply to that
/// system (not a `.zip`, or not a loose-cartridge platform), otherwise
/// whether [`select_zip_cartridge_member`] selects a member. Reads only the
/// central directory; nothing is decompressed or written.
pub fn zip_cartridge_member_fits(zip: &Path, platform_id: &str) -> Option<bool> {
    if !is_zipped_cartridge_request(zip, platform_id) {
        return None;
    }
    let platform = staging_platform(platform_id);
    let Ok(file) = open_read_only_regular(zip, &TrustedRoots::none()) else {
        return Some(false);
    };
    let Ok(mut archive) = ZipArchive::new(file) else {
        return Some(false);
    };
    if platform == IdentityPlatform::Amiga {
        return Some(
            crate::zip_floppy_set::select_amiga_ipf_disk_set(&mut archive, &mut 0).is_ok(),
        );
    }
    Some(select_zip_cartridge_member(&mut archive, platform, &mut 0).is_ok())
}

/// A zipped cartridge ROM (or a zipped Amiga game's disk set) extracted for
/// one launch into a private, owner-only temporary directory outside the
/// library. Dropping it removes the directory, so failure paths and process
/// exit clean up. The ZIP is never modified.
#[derive(Debug)]
pub struct StagedCartridge {
    _dir: tempfile::TempDir,
    path: PathBuf,
    /// For a disk set of more than one disk: the M3U playlist (beside the
    /// disks, naming them in boot order) the emulator is given instead of
    /// [`Self::path`], so every disk can be swapped in.
    playlist: Option<PathBuf>,
}

impl StagedCartridge {
    /// The extracted ROM, or a disk set's first (boot) disk: the file the
    /// launch's identity and preflight checks are held to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The disk-set playlist the emulator should load in place of
    /// [`Self::path`], when there is more than one disk.
    pub fn playlist(&self) -> Option<&Path> {
        self.playlist.as_deref()
    }

    /// Leaves the directory in place (used when the emulator may still be
    /// running); it is then the OS temp cleaner's to remove.
    pub fn keep(self) {
        std::mem::forget(self);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageZipError {
    /// Not a ZIP on a loose-cartridge platform: nothing to stage.
    NotApplicable,
    /// The ZIP is not safe to stage; the text says why.
    Refused(String),
    /// The caller's check of the opened ZIP (its content-bound key) failed:
    /// it is not the file that was approved.
    NotAccepted,
}

fn staged_file_name(member_name: &[u8]) -> Option<String> {
    let name = std::str::from_utf8(member_name)
        .ok()?
        .rsplit('/')
        .next()?
        .to_string();
    let safe = !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && name.chars().all(|c| {
            c.is_alphanumeric()
                || matches!(
                    c,
                    ' ' | '.' | '_' | '-' | '(' | ')' | '[' | ']' | ',' | '\'' | '&' | '!' | '+'
                )
        });
    safe.then_some(name)
}

/// Extracts the single validated cartridge ROM member of `zip` for one launch.
/// Applies the same member selection as identity inspection, counts the bytes
/// actually written against the declared size (aborting past it), writes only
/// a validated plain file name with `O_EXCL|O_NOFOLLOW` into a fresh 0700
/// directory, and re-checks that the ZIP itself did not change. The caller's
/// own preflight must still re-verify the staged file's identity.
#[cfg(unix)]
pub fn stage_zip_cartridge_for_launch(
    zip: &Path,
    platform_hint: &str,
) -> Result<StagedCartridge, StageZipError> {
    stage_zip_cartridge_for_launch_checked(zip, platform_hint, |_| true)
}

/// [`stage_zip_cartridge_for_launch`], with `accept` first run on the very
/// handle the member is then extracted from (before anything is read or
/// written). A launch whose authority is a content-bound key re-derives it
/// here, so a ZIP renamed over the approved one cannot be extracted in its
/// place. `accept` returning `false` refuses the launch.
#[cfg(unix)]
pub fn stage_zip_cartridge_for_launch_checked(
    zip: &Path,
    platform_hint: &str,
    accept: impl FnOnce(&mut std::fs::File) -> bool,
) -> Result<StagedCartridge, StageZipError> {
    use std::io::{Seek, SeekFrom};

    let platform = staging_platform(platform_hint);
    if !is_zipped_cartridge_request(zip, platform_hint) {
        return Err(StageZipError::NotApplicable);
    }
    let refuse = |message: String| StageZipError::Refused(message);
    let mut file = open_read_only_regular(zip, &TrustedRoots::none()).map_err(refuse)?;
    let before = StableFileMetadata::from_file(&file).map_err(|e| refuse(e.to_string()))?;
    if !accept(&mut file) {
        return Err(StageZipError::NotAccepted);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| refuse(format!("could not re-read the ZIP: {error}")))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| refuse(format!("invalid ZIP: {error}")))?;
    let mut seen = 0;
    let members = if platform == IdentityPlatform::Amiga {
        crate::zip_floppy_set::select_amiga_ipf_disk_set(&mut archive, &mut seen)
            .map_err(|(_, message)| refuse(message))?
    } else {
        vec![
            select_zip_cartridge_member(&mut archive, platform, &mut seen)
                .map_err(|(_, message)| refuse(message))?,
        ]
    };
    let dir = tempfile::Builder::new()
        .prefix("emuwiz-zip-rom-")
        .tempdir()
        .map_err(|error| refuse(format!("could not create the private directory: {error}")))?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
            .map_err(|error| refuse(format!("could not secure the private directory: {error}")))?;
    }
    let mut staged = Vec::with_capacity(members.len());
    for member in &members {
        staged.push(extract_member(&mut archive, member, dir.path()).map_err(refuse)?);
    }
    let playlist = if staged.len() > 1 {
        Some(write_disk_playlist(dir.path(), &staged).map_err(refuse)?)
    } else {
        None
    };
    if !still_same_file(zip, &TrustedRoots::none(), &before) {
        return Err(refuse(
            "ZIP changed while the ROM was being extracted".into(),
        ));
    }
    let path = staged.swap_remove(0);
    Ok(StagedCartridge {
        _dir: dir,
        path,
        playlist,
    })
}

/// Writes one selected member into `dir` under its validated plain name with
/// `O_EXCL|O_NOFOLLOW`, counting the bytes actually written against the
/// declared size (aborting past it).
#[cfg(unix)]
fn extract_member(
    archive: &mut ZipArchive<std::fs::File>,
    member: &crate::zip_cartridge_select::ZipCartridgeMember,
    dir: &Path,
) -> Result<PathBuf, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let name = staged_file_name(&member.name)
        .ok_or_else(|| "ROM member file name is not safe to extract".to_string())?;
    // Cores only accept headerless No-Intro NES dumps under `.nes`; the
    // bytes are not changed (no header is added).
    let name = if member.format == "unh" {
        format!(
            "{}.nes",
            name.rsplit_once('.')
                .map_or(name.as_str(), |(stem, _)| stem)
        )
    } else {
        name
    };
    let path = dir.join(&name);
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|error| format!("could not create the staged ROM: {error}"))?;
    {
        let entry = archive
            .by_index(member.index)
            .map_err(|error| error.to_string())?;
        // The entry read must be the one the selector validated.
        if entry.name_raw() != member.name.as_slice() {
            return Err("ZIP entry changed since it was selected".into());
        }
        let mut limited = entry.take(member.declared.saturating_add(1));
        let copied = io::copy(&mut limited, &mut out)
            .map_err(|error| format!("ROM member is corrupt or truncated: {error}"))?;
        if copied != member.declared {
            return Err(format!(
                "ROM member inflated to {copied} bytes but its header declares {}",
                member.declared
            ));
        }
    }
    out.sync_all().ok();
    Ok(path)
}

/// An M3U naming the staged disks (plain names, same directory) in order.
/// It is named after the boot disk (`<disk 1 stem>.m3u`) because RetroArch
/// names saves and states after the content it loads: one shared name would
/// mix different games' states.
#[cfg(unix)]
fn write_disk_playlist(dir: &Path, disks: &[PathBuf]) -> Result<PathBuf, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut text = String::new();
    for disk in disks {
        let name = disk
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| "staged disk name is not valid text".to_string())?;
        text.push_str(name);
        text.push('\n');
    }
    let stem = disks
        .first()
        .and_then(|d| d.file_stem())
        .and_then(|n| n.to_str())
        .ok_or_else(|| "staged disk name is not valid text".to_string())?;
    let path = dir.join(format!("{stem}.m3u"));
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|error| format!("could not create the disk playlist: {error}"))?;
    out.write_all(text.as_bytes())
        .map_err(|error| format!("could not write the disk playlist: {error}"))?;
    Ok(path)
}

#[cfg(not(unix))]
pub fn stage_zip_cartridge_for_launch(
    _zip: &Path,
    _platform_hint: &str,
) -> Result<StagedCartridge, StageZipError> {
    Err(StageZipError::NotApplicable)
}

#[cfg(not(unix))]
pub fn stage_zip_cartridge_for_launch_checked(
    _zip: &Path,
    _platform_hint: &str,
    _accept: impl FnOnce(&mut std::fs::File) -> bool,
) -> Result<StagedCartridge, StageZipError> {
    Err(StageZipError::NotApplicable)
}
