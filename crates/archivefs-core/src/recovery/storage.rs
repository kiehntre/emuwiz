//! Where prepared copies are kept.
//!
//! By default `<data>/prepared-media`. The person may choose another folder
//! (for example on a larger disk); the choice is one line in
//! `prepared_media_folder.txt` in EmuWiz's configuration folder and is
//! accepted only after [`check_folder`] passes. Copies already in another
//! folder are not moved and are not looked for there.

use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::prepared_media::PreparedMediaStore;

/// File name inside EmuWiz's configuration folder.
pub const STORAGE_SETTING_FILE: &str = "prepared_media_folder.txt";
const MAX_SETTING_BYTES: u64 = 8 * 1024;

fn setting_path() -> Result<PathBuf, String> {
    crate::app_dirs::config_path(STORAGE_SETTING_FILE).map_err(|error| error.to_string())
}

/// The chosen folder, or `None` for the default. A setting that is present
/// but unusable is an error, never silently replaced by the default: copies
/// must not start appearing somewhere the person did not choose.
pub fn configured_folder() -> Result<Option<PathBuf>, String> {
    configured_folder_at(&setting_path()?)
}

pub fn configured_folder_at(setting: &Path) -> Result<Option<PathBuf>, String> {
    use std::io::Read;
    let file = match crate::safe_read::open_regular_file_nonblocking(setting, false) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", setting.display())),
    };
    let mut text = String::new();
    file.take(MAX_SETTING_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| format!("{}: {error}", setting.display()))?;
    if text.len() as u64 > MAX_SETTING_BYTES {
        return Err(format!("{} is not a folder setting", setting.display()));
    }
    let line = text.trim_end_matches(['\n', '\r']);
    if line.is_empty() {
        return Ok(None);
    }
    let folder = PathBuf::from(line);
    if line.contains(['\n', '\r', '\0']) || !folder.is_absolute() {
        return Err(format!(
            "{} does not hold one absolute folder path",
            setting.display()
        ));
    }
    Ok(Some(folder))
}

/// The result of examining a folder proposed for prepared copies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StorageCheck {
    pub folder: PathBuf,
    /// Why the folder cannot be used. Empty means it can.
    pub problems: Vec<String>,
    /// Things worth knowing that do not prevent its use.
    pub notes: Vec<String>,
    pub available_bytes: Option<u64>,
    /// True when the folder is on the same filesystem as EmuWiz's data folder.
    pub same_disk_as_data: Option<bool>,
}

impl StorageCheck {
    pub fn usable(&self) -> bool {
        self.problems.is_empty()
    }
}

fn has_plain_components(path: &Path) -> bool {
    path.is_absolute()
        && path.components().all(|component| {
            matches!(
                component,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
}

/// Examines `folder` without changing anything except, when `create` is set
/// and it does not exist, creating that one folder (0700) and, inside it, a
/// probe that is removed again. `library_folders` are the game folders EmuWiz
/// scans: prepared copies inside one would be listed as games.
pub fn check_folder(folder: &Path, library_folders: &[PathBuf], create: bool) -> StorageCheck {
    let mut check = StorageCheck {
        folder: folder.to_path_buf(),
        problems: Vec::new(),
        notes: Vec::new(),
        available_bytes: None,
        same_disk_as_data: None,
    };
    if !has_plain_components(folder) {
        check
            .problems
            .push("Give the full path of a folder, without `..`.".into());
        return check;
    }
    let existing = match std::fs::symlink_metadata(folder) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            check
                .problems
                .push("This is a link to another folder. Choose the real folder itself.".into());
            return check;
        }
        Ok(metadata) if !metadata.is_dir() => {
            check.problems.push("This is a file, not a folder.".into());
            return check;
        }
        Ok(_) => true,
        Err(_) => false,
    };
    // Where the folder really is (or would be), before anything is created.
    let resolved = if existing {
        std::fs::canonicalize(folder).ok()
    } else {
        folder
            .parent()
            .filter(|parent| std::fs::metadata(parent).is_ok_and(|m| m.is_dir()))
            .and_then(|parent| std::fs::canonicalize(parent).ok())
            .zip(folder.file_name())
            .map(|(parent, name)| parent.join(name))
    };
    let Some(resolved) = resolved else {
        check.problems.push(if existing {
            "The folder could not be opened.".into()
        } else {
            "The folder above this one does not exist.".into()
        });
        return check;
    };
    check = location_checks(check, &resolved, library_folders);
    if !check.usable() {
        return check;
    }
    if !existing {
        if !create {
            check
                .notes
                .push("The folder does not exist yet; it will be created.".into());
            if let Some(parent) = resolved.parent() {
                check.available_bytes = super::prepared_media::available_bytes(parent).ok();
                note_disk(&mut check, parent);
            }
            return check;
        }
        if let Err(error) = std::fs::DirBuilder::new().mode(0o700).create(folder) {
            check
                .problems
                .push(format!("The folder could not be created: {error}."));
            return check;
        }
    }
    let Ok(metadata) = std::fs::metadata(&resolved) else {
        check
            .problems
            .push("The folder could not be opened.".into());
        return check;
    };
    // SAFETY: geteuid has no preconditions.
    if metadata.uid() != unsafe { libc::geteuid() } {
        check
            .problems
            .push("The folder belongs to another user.".into());
    }
    if metadata.mode() & 0o022 != 0 {
        check.problems.push(
            "Other users can write to this folder. Choose a folder only you can change.".into(),
        );
    }
    if check.problems.is_empty() {
        // The store publishes each copy with a no-replace rename; a
        // filesystem without it cannot hold the store.
        match tempfile::Builder::new()
            .prefix(".emuwiz-storage-check-")
            .tempdir_in(&resolved)
        {
            Ok(probe) => {
                let from = probe.path().join("a");
                let to = probe.path().join("b");
                let renamed = std::fs::create_dir(&from).is_ok()
                    && crate::dat::rename_apply::noclobber::rename_noreplace(&from, &to).is_ok();
                if !renamed {
                    check.problems.push(
                        "This disk's filesystem cannot publish a copy safely (no no-replace rename). Use a Linux filesystem such as ext4, XFS or Btrfs."
                            .into(),
                    );
                }
            }
            Err(error) => check
                .problems
                .push(format!("EmuWiz cannot write in this folder: {error}.")),
        }
    }
    check.available_bytes = super::prepared_media::available_bytes(&resolved).ok();
    note_disk(&mut check, &resolved);
    check
}

/// Refuses a folder inside a game folder, or one that contains a game folder.
fn location_checks(
    mut check: StorageCheck,
    resolved: &Path,
    library_folders: &[PathBuf],
) -> StorageCheck {
    for library in library_folders {
        let library = std::fs::canonicalize(library).unwrap_or_else(|_| library.clone());
        if resolved.starts_with(&library) || check.folder.starts_with(&library) {
            check.problems.push(format!(
                "This folder is inside your game folder {}. A scan would list prepared copies as games.",
                library.display()
            ));
        } else if library.starts_with(resolved) {
            check.problems.push(format!(
                "Your game folder {} is inside this folder.",
                library.display()
            ));
        }
    }
    check
}

fn note_disk(check: &mut StorageCheck, existing: &Path) {
    if let Ok(data) = crate::app_dirs::data_dir()
        && let (Ok(here), Ok(there)) = (std::fs::metadata(existing), std::fs::metadata(&data))
    {
        let same = here.dev() == there.dev();
        check.same_disk_as_data = Some(same);
        if same {
            check
                .notes
                .push("This folder is on the same disk as EmuWiz's own data.".into());
        }
    }
}

/// Saves `folder` as the place for prepared copies after it passes
/// [`check_folder`] (creating it if needed), or clears the choice with
/// `None`. Copies already prepared elsewhere stay where they are.
pub fn set_folder(
    folder: Option<&Path>,
    library_folders: &[PathBuf],
) -> Result<Option<StorageCheck>, String> {
    set_folder_at(&setting_path()?, folder, library_folders)
}

pub fn set_folder_at(
    setting: &Path,
    folder: Option<&Path>,
    library_folders: &[PathBuf],
) -> Result<Option<StorageCheck>, String> {
    let Some(folder) = folder else {
        match std::fs::remove_file(setting) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("{}: {error}", setting.display())),
        }
        return Ok(None);
    };
    let check = check_folder(folder, library_folders, true);
    if !check.usable() {
        return Err(check.problems.join(" "));
    }
    let text = folder
        .to_str()
        .ok_or("The folder name is not valid text.")?
        .to_string();
    if let Some(parent) = setting.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    crate::atomic_write_text(setting, &format!("{text}\n")).map_err(|error| error.to_string())?;
    Ok(Some(check))
}

/// The store this profile uses now, and the default one, for display.
pub fn describe_current() -> Result<(PreparedMediaStore, bool), String> {
    let chosen = configured_folder()?.is_some()
        || std::env::var_os(super::prepared_media::PREPARED_MEDIA_DIR_ENV).is_some();
    Ok((PreparedMediaStore::for_profile()?, chosen))
}
