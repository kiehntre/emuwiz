//! Library Play for a Vita package (`.vpk`) whose title is already installed
//! in Vita3K.
//!
//! A `.vpk` is never given to Vita3K. It is only *read* (one bounded
//! `sce_sys/param.sfo` member of the ZIP, nothing extracted) to learn which
//! `TITLE_ID` it claims. Play is offered only when exactly one usable Vita3K
//! installation already holds a title with that id, and then it starts that
//! installed title through the unchanged installed-title path
//! ([`preflight_and_launch_vita3k_installed`]): fresh rescan, installed
//! identity checks, the existing Vita3K preflight and spawn.
//!
//! The binding is *title id only*. It does not claim the installed copy is
//! byte-identical to the package (updates change `param.sfo`; that case is
//! reported as a difference, not refused). It is never a catalogue match,
//! never persisted, and never shown as Verified.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::launch::process_spawn::{CapturedFileIdentity, WatchedProcess};
use crate::launch::vita3k_installed::{
    Vita3kInstalledEntry, Vita3kInstalledLaunchError, Vita3kInstalledLaunchRequest,
    Vita3kInventory, preflight_and_launch_vita3k_installed, scan_vita3k_installed_titles,
};
use crate::patch_manager::{VITA3K_MAX_SFO_BYTES, Vita3kProfileDiscoveryRoots};

const MAX_MEMBERS: usize = 200_000;
const PARAM_MEMBER: &str = "sce_sys/param.sfo";
const EBOOT_MEMBER: &str = "eboot.bin";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpkRefusalKind {
    NotVpkExtension,
    NotRegularFile,
    Unreadable,
    NotAPackage,
    MissingParam,
    InvalidParam,
    ChangedWhileReading,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VpkRefusal {
    pub kind: VpkRefusalKind,
    pub detail: String,
}

fn refuse(kind: VpkRefusalKind, detail: impl Into<String>) -> VpkRefusal {
    VpkRefusal {
        kind,
        detail: detail.into(),
    }
}

/// What one bounded read of one `.vpk` established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VpkEvidence {
    pub path: PathBuf,
    pub file_identity: CapturedFileIdentity,
    pub title_id: String,
    pub param_sha256: String,
    /// Display-only; never part of the identity.
    pub display_title: Option<String>,
}

pub fn is_vpk(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("vpk"))
}

fn safe_title_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
}

/// Reads only the ZIP directory and the one `sce_sys/param.sfo` member.
pub fn inspect_vpk(path: &Path) -> Result<VpkEvidence, VpkRefusal> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    if !is_vpk(path) {
        return Err(refuse(
            VpkRefusalKind::NotVpkExtension,
            "the file is not a .vpk",
        ));
    }
    let unreadable = |e: std::io::Error| refuse(VpkRefusalKind::Unreadable, e.to_string());
    let stat = fs::symlink_metadata(path).map_err(unreadable)?;
    if stat.file_type().is_symlink() || !stat.is_file() {
        return Err(refuse(
            VpkRefusalKind::NotRegularFile,
            "the .vpk is not a regular file",
        ));
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(unreadable)?;
    let handle = file.metadata().map_err(unreadable)?;
    if !handle.is_file()
        || (handle.dev(), handle.ino(), handle.len()) != (stat.dev(), stat.ino(), stat.len())
    {
        return Err(refuse(
            VpkRefusalKind::ChangedWhileReading,
            "the .vpk changed while it was being opened",
        ));
    }
    let identity = CapturedFileIdentity::capture(&handle);
    let mut archive = zip::ZipArchive::new(file).map_err(|e| {
        refuse(
            VpkRefusalKind::NotAPackage,
            format!("not a readable ZIP: {e}"),
        )
    })?;
    if archive.len() > MAX_MEMBERS {
        return Err(refuse(
            VpkRefusalKind::NotAPackage,
            "too many package members",
        ));
    }
    let (mut param_index, mut has_eboot) = (None, false);
    for index in 0..archive.len() {
        let member = archive.by_index_raw(index).map_err(|e| {
            refuse(
                VpkRefusalKind::NotAPackage,
                format!("member unreadable: {e}"),
            )
        })?;
        let name = member.name().map(|n| n.into_owned()).unwrap_or_default();
        if member.is_dir() || member.is_symlink() {
            continue;
        }
        if name.as_str() == EBOOT_MEMBER {
            has_eboot = true;
        } else if name.as_str() == PARAM_MEMBER {
            if param_index.replace(index).is_some() {
                return Err(refuse(
                    VpkRefusalKind::InvalidParam,
                    "the package has more than one param.sfo",
                ));
            }
            if member.encrypted() || member.size() > VITA3K_MAX_SFO_BYTES {
                return Err(refuse(
                    VpkRefusalKind::InvalidParam,
                    "param.sfo is encrypted or too large",
                ));
            }
        }
    }
    if !has_eboot {
        return Err(refuse(
            VpkRefusalKind::NotAPackage,
            "the package has no eboot.bin at its root",
        ));
    }
    let index = param_index.ok_or_else(|| {
        refuse(
            VpkRefusalKind::MissingParam,
            "the package has no sce_sys/param.sfo",
        )
    })?;
    let mut bytes = Vec::new();
    let mut member = archive.by_index(index).map_err(|e| {
        refuse(
            VpkRefusalKind::InvalidParam,
            format!("param.sfo unreadable: {e}"),
        )
    })?;
    (&mut member)
        .take(VITA3K_MAX_SFO_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| {
            refuse(
                VpkRefusalKind::InvalidParam,
                format!("param.sfo unreadable: {e}"),
            )
        })?;
    if bytes.len() as u64 > VITA3K_MAX_SFO_BYTES {
        return Err(refuse(
            VpkRefusalKind::InvalidParam,
            "param.sfo is too large",
        ));
    }
    let sfo = crate::param_sfo::parse_param_sfo(&bytes)
        .ok_or_else(|| refuse(VpkRefusalKind::InvalidParam, "param.sfo is not a valid SFO"))?;
    let title_id = sfo
        .get_text("TITLE_ID")
        .filter(|id| safe_title_id(id))
        .ok_or_else(|| {
            refuse(
                VpkRefusalKind::InvalidParam,
                "param.sfo has no usable TITLE_ID",
            )
        })?
        .to_string();
    let param_sha256 = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(VpkEvidence {
        path: path.to_path_buf(),
        file_identity: identity,
        title_id,
        param_sha256,
        display_title: sfo.get_text("TITLE").map(str::to_string),
    })
}

/// How a package's title id relates to the Vita3K installations found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Vita3kLibraryResolution {
    /// Exactly one usable installation holds the title and it can launch.
    Installed {
        entry: Box<Vita3kInstalledEntry>,
        /// The installed `param.sfo` differs from the package's (e.g. updated).
        param_differs: bool,
    },
    /// Installed but blocked (licence, firmware); the reason is shown.
    Blocked(String),
    /// More than one installation holds the title.
    Ambiguous,
    /// Vita3K is usable but the title is not installed.
    NotInstalled,
    /// No usable Vita3K installation.
    NoInstallation(String),
}

pub fn resolve_vpk_against(
    evidence: &VpkEvidence,
    inventory: &Vita3kInventory,
) -> Vita3kLibraryResolution {
    let mut holders = inventory
        .entries
        .iter()
        .filter(|entry| entry.title_id == evidence.title_id);
    match (holders.next(), holders.next()) {
        (Some(_), Some(_)) => Vita3kLibraryResolution::Ambiguous,
        (Some(entry), None) => match &entry.blocker {
            Some(reason) => Vita3kLibraryResolution::Blocked(reason.clone()),
            None => Vita3kLibraryResolution::Installed {
                param_differs: entry.param_sha256 != evidence.param_sha256,
                entry: Box::new(entry.clone()),
            },
        },
        (None, _) => {
            if inventory.profiles.iter().any(|p| p.eligible) {
                Vita3kLibraryResolution::NotInstalled
            } else {
                Vita3kLibraryResolution::NoInstallation(
                    inventory
                        .profiles
                        .iter()
                        .find_map(|p| p.blocker.clone())
                        .unwrap_or_else(|| "no Vita3K installation was found".into()),
                )
            }
        }
    }
}

pub fn resolve_vpk(
    evidence: &VpkEvidence,
    roots: &Vita3kProfileDiscoveryRoots,
) -> Vita3kLibraryResolution {
    resolve_vpk_against(evidence, &scan_vita3k_installed_titles(roots))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vita3kLibraryLaunchRequest {
    pub vpk: VpkEvidence,
    pub entry: Vita3kInstalledEntry,
}

#[derive(Debug)]
pub enum Vita3kLibraryLaunchError {
    Package(VpkRefusal),
    /// The package changed or names another title since Play was offered.
    PackageChanged(String),
    Installed(Vita3kInstalledLaunchError),
}

/// Re-reads the package, requires it unchanged and still naming the offered
/// title, then runs the installed-title launch (fresh rescan and checks).
pub fn preflight_and_launch_vita3k_library(
    request: &Vita3kLibraryLaunchRequest,
    roots: &Vita3kProfileDiscoveryRoots,
) -> Result<WatchedProcess, Vita3kLibraryLaunchError> {
    use Vita3kLibraryLaunchError as E;
    let fresh = inspect_vpk(&request.vpk.path).map_err(E::Package)?;
    if fresh != request.vpk {
        return Err(E::PackageChanged(
            "the .vpk changed since Play was offered".into(),
        ));
    }
    if fresh.title_id != request.entry.title_id {
        return Err(E::PackageChanged(
            "the .vpk does not name the offered installed title".into(),
        ));
    }
    preflight_and_launch_vita3k_installed(
        &Vita3kInstalledLaunchRequest {
            entry: request.entry.clone(),
        },
        roots,
    )
    .map_err(E::Installed)
}

#[cfg(test)]
mod tests;
