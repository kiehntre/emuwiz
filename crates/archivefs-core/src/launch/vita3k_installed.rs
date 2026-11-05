//! Installed-title launch identity for Vita3K, from fresh emulator inventory.
//!
//! Vita3K runs titles that are *installed* into its own emulated filesystem
//! (`<ux0>/app/<TITLE_ID>`); EmuWiz never installs, moves or edits them. This
//! module turns the existing, already-reviewed Vita3K discovery
//! ([`discover_vita3k_profiles`]) and installed-title inspector
//! ([`inspect_installed_title`], which validates `param.sfo` and checks that its
//! `TITLE_ID` equals the directory name) into launchable entries, binding each
//! to the selected Vita3K installation and to the title's own bytes.
//!
//! This is an *installation* identity, not a catalogue/DAT identity: the game
//! key is namespaced `vita3k-installed:<TITLE_ID>`, nothing here says the title
//! is a verified dump, and the SFO `TITLE` is display-only (a misleading name
//! never selects or authorises anything). Nothing is persisted.
//!
//! Fail-closed: a title present under more than one Vita3K profile is
//! ambiguous and refused (EmuWiz will not pick an installation), as are a
//! missing licence, missing firmware, an ineligible profile, a `param.sfo` that
//! is not a regular file, and any drift between the listing a person saw and
//! the state at launch time. Vita3K itself resolves `--installed-path <ID>` from
//! its own configuration; EmuWiz binds the installation it scanned (profile,
//! executable and config identity) but cannot pass it a different data folder.
//! The launch reuses [`preflight_vita3k_launch`] and
//! [`spawn_vita3k`] unchanged.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::launch::planning::{
    CanonicalIdentityStatus, LaunchContainerKind, LaunchContentKind, LaunchContentRef,
    ResolvedIdentity,
};
use crate::launch::process_spawn::{CapturedFileIdentity, WatchedProcess};
use crate::launch::vita3k_command::VITA3K_SUPPORTED_PLATFORM_ID;
use crate::launch::vita3k_execution::{
    Vita3kLaunchPreflightError, Vita3kLaunchRequest, preflight_vita3k_launch, spawn_vita3k,
};
use crate::patch_manager::{
    VITA3K_MAX_SFO_BYTES, Vita3kFirmwareState, Vita3kLicenseState, Vita3kProfile,
    Vita3kProfileDiscoveryRoots, discover_vita3k_profiles, inspect_installed_title,
    resolve_vita3k_native_launch_binding,
};

/// Namespace of the installation-only game key. Catalogue keys never use it.
pub const VITA3K_INSTALLED_KEY_PREFIX: &str = "vita3k-installed:";
const MAX_TITLE_DIRECTORIES: usize = 4096;

/// One installed title under one Vita3K installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vita3kInstalledEntry {
    pub profile_id: String,
    pub executable: PathBuf,
    pub executable_identity: CapturedFileIdentity,
    pub config_identity: Option<CapturedFileIdentity>,
    pub title_id: String,
    pub title_root: PathBuf,
    pub title_identity: CapturedFileIdentity,
    pub param_identity: CapturedFileIdentity,
    pub param_sha256: String,
    /// `param.sfo` `TITLE`: display only, never identity.
    pub display_title: Option<String>,
    pub category: Option<String>,
    pub license: Vita3kLicenseState,
    pub firmware: Vita3kFirmwareState,
    /// Why this title cannot be launched, if it cannot.
    pub blocker: Option<String>,
}

impl Vita3kInstalledEntry {
    pub fn game_key(&self) -> String {
        format!("{VITA3K_INSTALLED_KEY_PREFIX}{}", self.title_id)
    }

    /// Installation-only identity; see the module documentation.
    pub fn identity_status(&self) -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: VITA3K_SUPPORTED_PLATFORM_ID.into(),
            game_key: self.game_key(),
        })
    }

    pub fn content_ref(&self) -> LaunchContentRef {
        LaunchContentRef {
            kind: Some(LaunchContentKind::Executable),
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: Some(self.title_root.clone()),
            requires_mount: false,
            provenance: "title installed in a Vita3K installation (launch target only; not a catalogue match)"
                .into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vita3kProfileSummary {
    pub profile_id: String,
    pub eligible: bool,
    pub blocker: Option<String>,
    pub firmware: Vita3kFirmwareState,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Vita3kInventory {
    pub profiles: Vec<Vita3kProfileSummary>,
    pub entries: Vec<Vita3kInstalledEntry>,
    /// Directories under `app/` that are not valid installed titles.
    pub skipped: Vec<String>,
}

fn regular_identity(path: &Path) -> Option<CapturedFileIdentity> {
    let metadata = fs::symlink_metadata(path).ok()?;
    (metadata.is_file() && !metadata.file_type().is_symlink())
        .then(|| CapturedFileIdentity::capture(&metadata))
}

/// Reads `param.sfo` once; the hash and the `TITLE_ID` check use these bytes.
fn read_param(path: &Path) -> Option<(String, Option<String>)> {
    use std::fmt::Write;
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(VITA3K_MAX_SFO_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > VITA3K_MAX_SFO_BYTES {
        return None;
    }
    let hash = Sha256::digest(&bytes)
        .iter()
        .fold(String::new(), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        });
    let title_id = crate::param_sfo::parse_param_sfo(&bytes)
        .and_then(|sfo| sfo.get_text("TITLE_ID").map(str::to_string));
    Some((hash, title_id))
}

/// An id that cannot be mistaken for an option when passed as a value.
fn safe_title_id(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
}

fn real_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
}

fn scan_profile(profile: &Vita3kProfile, inventory: &mut Vita3kInventory) {
    let binding = resolve_vita3k_native_launch_binding(profile);
    let mut blocker = profile.blocker.clone();
    if let Err(error) = &binding {
        blocker.get_or_insert_with(|| error.detail.clone());
    }
    inventory.profiles.push(Vita3kProfileSummary {
        profile_id: profile.profile_id.clone(),
        eligible: profile.eligible && binding.is_ok(),
        blocker,
        firmware: profile.firmware,
    });
    let Ok(binding) = binding else { return };
    if !profile.eligible {
        return;
    }
    let Some(executable_identity) = regular_identity(&binding.executable) else {
        return;
    };
    let config_identity = profile.config_path.as_deref().and_then(regular_identity);
    let app_root = profile.vita_fs_path.join("app");
    let Ok(metadata) = fs::symlink_metadata(&app_root) else {
        return;
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return;
    }
    let Ok(read_dir) = fs::read_dir(&app_root) else {
        return;
    };
    let mut names: Vec<String> = read_dir
        .filter_map(Result::ok)
        .take(MAX_TITLE_DIRECTORIES)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    for name in names {
        let root = app_root.join(&name);
        let Some(title_identity) = fs::symlink_metadata(&root)
            .ok()
            .filter(|m| m.is_dir() && !m.file_type().is_symlink())
            .map(|m| CapturedFileIdentity::capture(&m))
        else {
            continue;
        };
        if !safe_title_id(&name) {
            inventory
                .skipped
                .push(format!("{name}: not a usable title id"));
            continue;
        }
        if !real_directory(&root.join("sce_sys")) {
            inventory
                .skipped
                .push(format!("{name}: sce_sys is not a real folder"));
            continue;
        }
        let title = match inspect_installed_title(profile, &name) {
            Ok(title) => title,
            Err(reason) => {
                inventory.skipped.push(format!("{name}: {reason}"));
                continue;
            }
        };
        let param = root.join("sce_sys/param.sfo");
        let (Some(param_identity), Some((param_sha256, hashed_title_id))) =
            (regular_identity(&param), read_param(&param))
        else {
            inventory
                .skipped
                .push(format!("{name}: param.sfo is not a regular file"));
            continue;
        };
        if hashed_title_id.as_deref() != Some(name.as_str()) {
            inventory
                .skipped
                .push(format!("{name}: param.sfo changed while it was being read"));
            continue;
        }
        let blocker = if title.license == Vita3kLicenseState::Missing {
            Some("the installed title's licence is missing in this Vita3K installation".into())
        } else if profile.firmware != Vita3kFirmwareState::PresentUnverified {
            Some("Vita3K firmware does not appear to be installed in this installation (install it inside Vita3K)".into())
        } else {
            None
        };
        inventory.entries.push(Vita3kInstalledEntry {
            profile_id: profile.profile_id.clone(),
            executable: binding.executable.clone(),
            executable_identity,
            config_identity,
            title_id: title.title_id,
            title_root: title.root,
            title_identity,
            param_identity,
            param_sha256,
            display_title: title.title,
            category: title.category,
            license: title.license,
            firmware: profile.firmware,
            blocker,
        });
    }
}

/// Fresh, read-only inventory of installed titles across all discovered Vita3K
/// installations. A title installed under more than one installation is marked
/// ambiguous (blocked) in every installation that holds it.
pub fn scan_vita3k_installed_titles(roots: &Vita3kProfileDiscoveryRoots) -> Vita3kInventory {
    let mut inventory = Vita3kInventory::default();
    for profile in &discover_vita3k_profiles(roots).profiles {
        scan_profile(profile, &mut inventory);
    }
    let mut holders: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in &inventory.entries {
        holders
            .entry(entry.title_id.clone())
            .or_default()
            .push(entry.profile_id.clone());
    }
    for entry in &mut inventory.entries {
        let profiles = &holders[&entry.title_id];
        if profiles.len() > 1 {
            entry.blocker = Some(format!(
                "this title is installed in more than one Vita3K installation ({}); EmuWiz will not choose one",
                profiles.join(", ")
            ));
        }
    }
    inventory.entries.sort_by(|a, b| {
        (&a.display_title, &a.title_id, &a.profile_id).cmp(&(
            &b.display_title,
            &b.title_id,
            &b.profile_id,
        ))
    });
    inventory
}

#[derive(Debug)]
pub enum Vita3kInstalledLaunchError {
    /// The title is no longer installed where it was listed.
    Missing(String),
    /// Several installations now hold it.
    Ambiguous(String),
    /// The installation or title is not launchable.
    Refused(String),
    /// Something changed since the listing the person saw.
    Changed(String),
    Preflight(Vita3kLaunchPreflightError),
    Spawn(std::io::Error),
}

/// One reviewed launch of one listed installed title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vita3kInstalledLaunchRequest {
    pub entry: Vita3kInstalledEntry,
}

/// Rescans, requires exactly this title in exactly this installation with
/// unchanged bytes, then runs the existing Vita3K preflight and spawn.
pub fn preflight_and_launch_vita3k_installed(
    request: &Vita3kInstalledLaunchRequest,
    roots: &Vita3kProfileDiscoveryRoots,
) -> Result<WatchedProcess, Vita3kInstalledLaunchError> {
    use Vita3kInstalledLaunchError as E;
    let reviewed = &request.entry;
    let fresh_inventory = scan_vita3k_installed_titles(roots);
    let mut holders = fresh_inventory
        .entries
        .iter()
        .filter(|entry| entry.title_id == reviewed.title_id);
    let fresh = holders.next().ok_or_else(|| {
        E::Missing(format!(
            "{} is no longer installed in a usable Vita3K installation",
            reviewed.title_id
        ))
    })?;
    if holders.next().is_some() {
        return Err(E::Ambiguous(format!(
            "{} is now installed in more than one Vita3K installation",
            reviewed.title_id
        )));
    }
    if let Some(blocker) = &fresh.blocker {
        return Err(E::Refused(blocker.clone()));
    }
    let changed = |what: &str| E::Changed(format!("{what} changed since the title was listed"));
    if fresh.profile_id != reviewed.profile_id {
        return Err(changed("the Vita3K installation holding the title"));
    }
    if fresh.executable != reviewed.executable
        || fresh.executable_identity != reviewed.executable_identity
    {
        return Err(changed("the Vita3K executable"));
    }
    if fresh.config_identity != reviewed.config_identity {
        return Err(changed("the Vita3K configuration"));
    }
    if fresh.title_root != reviewed.title_root
        || (fresh.title_identity.device, fresh.title_identity.inode)
            != (
                reviewed.title_identity.device,
                reviewed.title_identity.inode,
            )
    {
        return Err(changed("the installed title folder"));
    }
    if fresh.param_sha256 != reviewed.param_sha256
        || fresh.param_identity != reviewed.param_identity
    {
        return Err(changed("the title's param.sfo"));
    }
    let launch = Vita3kLaunchRequest {
        profile_id: fresh.profile_id.clone(),
        selected_title_path: fresh.title_root.clone(),
        expected_title_id: fresh.title_id.clone(),
        expected_platform_id: VITA3K_SUPPORTED_PLATFORM_ID.into(),
        expected_game_key: fresh.game_key(),
        expected_executable: fresh.executable.clone(),
        title_identity: fresh.title_identity,
        executable_identity: fresh.executable_identity,
        config_identity: fresh.config_identity,
    };
    let command =
        preflight_vita3k_launch(&launch, roots, &fresh.identity_status()).map_err(E::Preflight)?;
    spawn_vita3k(&command).map_err(E::Spawn)
}

#[cfg(test)]
mod tests;
