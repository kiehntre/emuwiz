//! Where prepared copies are kept.
//!
//! By default `<data>/prepared-media`. The person may choose another folder
//! (for example on a larger disk); the choice is one line in
//! `prepared_media_folder.txt` in EmuWiz's configuration folder and is
//! accepted only after [`check_folder`] passes, and is re-checked each time
//! it is used ([`still_safe`]). The filesystem the folder is on is recorded
//! with the choice, and the folder is used only while it is still on that
//! filesystem. Copies already in another folder are not
//! moved; folders used earlier are remembered, so their copies are still
//! found and still covered by the launch gate.

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

/// The saved folder text even when it would not pass the checks, for the
/// launch gate only (it must still be able to refuse files in that folder).
pub(crate) fn configured_folder_unchecked() -> Option<PathBuf> {
    configured_folder_unchecked_at(&setting_path().ok()?)
}

pub(crate) fn configured_folder_unchecked_at(setting: &Path) -> Option<PathBuf> {
    use std::io::Read;
    // Bounded and non-blocking: this runs before every launch, exactly when
    // the setting is in a state the normal reader refused.
    let file = crate::safe_read::open_regular_file_nonblocking(setting, false).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_SETTING_BYTES).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let folder = PathBuf::from(text.lines().next()?.trim());
    folder.is_absolute().then_some(folder)
}

/// Re-checks a saved folder each time it is used: it must still be a real
/// folder (not a link) of this user that others cannot write to. A missing
/// folder passes this check (an earlier folder whose disk is away is simply
/// not there); the folder in current use must also exist, which
/// `PreparedMediaStore::configured` requires.
pub fn still_safe(folder: &Path) -> Result<(), String> {
    if !has_plain_components(folder) {
        return Err(format!(
            "{} is not a plain absolute folder path",
            folder.display()
        ));
    }
    let metadata = match std::fs::symlink_metadata(folder) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("{}: {error}", folder.display())),
    };
    // SAFETY: geteuid has no preconditions.
    let mine = metadata.uid() == unsafe { libc::geteuid() };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "{} is no longer a real folder; choose the folder for prepared copies again",
            folder.display()
        ));
    }
    if !mine || metadata.mode() & 0o022 != 0 {
        return Err(format!(
            "{} is no longer a folder only you can change; choose the folder for prepared copies again",
            folder.display()
        ));
    }
    Ok(())
}

/// Which filesystem each chosen folder was on when it was chosen, beside the
/// setting. A folder merely existing later proves nothing: an unmounted disk
/// leaves its empty mount point behind, and another disk can be mounted in
/// its place. The store compares this id each time the folder is used.
const FILESYSTEMS_FILE: &str = "prepared_media_folder_filesystems.json";
const FILESYSTEMS_FORMAT: u32 = 1;
const MAX_FILESYSTEMS_BYTES: u64 = 256 * 1024;
const MAX_RECORDED_FOLDERS: usize = 64;

#[derive(serde::Deserialize, Serialize)]
struct RecordedFilesystems {
    format_version: u32,
    /// Folder path as chosen -> the filesystem's own id.
    folders: std::collections::BTreeMap<String, u64>,
}

fn filesystems_path_beside(setting: &Path) -> PathBuf {
    setting.with_file_name(FILESYSTEMS_FILE)
}

fn read_recorded(setting: &Path) -> Result<RecordedFilesystems, String> {
    use std::io::Read;
    let path = filesystems_path_beside(setting);
    let empty = || RecordedFilesystems {
        format_version: FILESYSTEMS_FORMAT,
        folders: std::collections::BTreeMap::new(),
    };
    let file = match crate::safe_read::open_regular_file_nonblocking(&path, false) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(empty()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let mut text = String::new();
    file.take(MAX_FILESYSTEMS_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let unusable = || format!("{} is not a record EmuWiz wrote", path.display());
    if text.len() as u64 > MAX_FILESYSTEMS_BYTES {
        return Err(unusable());
    }
    let recorded: RecordedFilesystems = serde_json::from_str(&text).map_err(|_| unusable())?;
    if recorded.format_version != FILESYSTEMS_FORMAT {
        return Err(unusable());
    }
    Ok(recorded)
}

/// The id of the filesystem `folder` was on when it was chosen, if that was
/// recorded. A record that cannot be read is an error, never "no record".
pub(crate) fn recorded_filesystem(folder: &Path) -> Result<Option<u64>, String> {
    recorded_filesystem_at(&setting_path()?, folder)
}

pub(crate) fn recorded_filesystem_at(setting: &Path, folder: &Path) -> Result<Option<u64>, String> {
    let Some(folder) = folder.to_str() else {
        return Ok(None);
    };
    Ok(read_recorded(setting)?.folders.get(folder).copied())
}

/// Whether an earlier folder may still be treated as a store of this
/// profile: it must pass [`still_safe`], its filesystem must have been
/// recorded and the record readable, and, while the folder exists, it must
/// be on that filesystem. `Err` carries the reason. The launch gate refuses
/// every file inside a folder this distrusts.
pub(crate) fn earlier_folder_distrust(folder: &Path) -> Result<(), String> {
    earlier_folder_distrust_at(&setting_path()?, folder)
}

pub(crate) fn earlier_folder_distrust_at(setting: &Path, folder: &Path) -> Result<(), String> {
    still_safe(folder)?;
    let Some(recorded) = recorded_filesystem_at(setting, folder)? else {
        return Err(format!(
            "EmuWiz has no record of which disk {} was on; choose it as the folder for prepared copies again to use the copies in it",
            folder.display()
        ));
    };
    if std::fs::symlink_metadata(folder).is_ok()
        && super::prepared_media::filesystem_id(folder) != Some(recorded)
    {
        return Err(format!(
            "{} is not on the disk it was on when it was chosen (is its disk connected?)",
            folder.display()
        ));
    }
    Ok(())
}

fn record_filesystem(setting: &Path, folder: &str, id: u64) -> Result<(), String> {
    // A record that cannot be read is replaced: the folder being chosen now
    // is the one whose disk is known for certain. Earlier folders lose their
    // entries with it and are then not used until chosen again.
    let mut recorded = read_recorded(setting).unwrap_or_else(|_| RecordedFilesystems {
        format_version: FILESYSTEMS_FORMAT,
        folders: std::collections::BTreeMap::new(),
    });
    recorded.folders.insert(folder.to_string(), id);
    // Bounded: only the current folder and the remembered earlier ones are
    // ever looked up.
    if recorded.folders.len() > MAX_RECORDED_FOLDERS {
        let keep: std::collections::BTreeSet<String> =
            previous_folders_at(&previous_path_beside(setting))
                .iter()
                .filter_map(|known| known.to_str().map(str::to_string))
                .chain(std::iter::once(folder.to_string()))
                .collect();
        recorded.folders.retain(|known, _| keep.contains(known));
    }
    let text = serde_json::to_string_pretty(&recorded).map_err(|error| error.to_string())?;
    crate::atomic_write_text(&filesystems_path_beside(setting), &format!("{text}\n"))
        .map_err(|error| error.to_string())
}

const PREVIOUS_FOLDERS_FILE: &str = "prepared_media_previous_folders.txt";
const MAX_PREVIOUS_FOLDERS: usize = 32;

fn previous_path_beside(setting: &Path) -> PathBuf {
    setting.with_file_name(PREVIOUS_FOLDERS_FILE)
}

/// Folders that were chosen for prepared copies earlier. Copies there are
/// still found and still guarded by the launch gate.
pub fn previous_folders() -> Vec<PathBuf> {
    setting_path()
        .map(|setting| previous_folders_at(&previous_path_beside(&setting)))
        .unwrap_or_default()
}

pub fn previous_folders_at(list: &Path) -> Vec<PathBuf> {
    use std::io::Read;
    let Ok(file) = crate::safe_read::open_regular_file_nonblocking(list, false) else {
        return Vec::new();
    };
    let mut text = String::new();
    if file.take(64 * 1024).read_to_string(&mut text).is_err() {
        return Vec::new();
    }
    text.lines()
        .map(PathBuf::from)
        .filter(|folder| has_plain_components(folder))
        .take(MAX_PREVIOUS_FOLDERS)
        .collect()
}

fn remember_previous(setting: &Path, folder: &Path) -> Result<(), String> {
    let list = previous_path_beside(setting);
    let mut folders = previous_folders_at(&list);
    folders.retain(|known| known != folder);
    folders.insert(0, folder.to_path_buf());
    folders.truncate(MAX_PREVIOUS_FOLDERS);
    let text: String = folders
        .iter()
        .filter_map(|folder| folder.to_str())
        .map(|folder| format!("{folder}\n"))
        .collect();
    crate::atomic_write_text(&list, &text).map_err(|error| error.to_string())
}

/// The game folders EmuWiz scans, for the "not inside a game folder" check.
/// No configuration at all means no game folders; a configuration that
/// cannot be read is an error, so the check is never silently skipped.
pub fn library_folders() -> Result<Vec<PathBuf>, String> {
    let config = crate::default_config_path().map_err(|error| error.to_string())?;
    match std::fs::symlink_metadata(&config) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("your game folders could not be read ({error})")),
    }
    crate::load_source_folder_configs_from(&config)
        .map(|sources| sources.into_iter().map(|source| source.path).collect())
        .map_err(|error| format!("your game folders could not be read ({error})"))
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
    // A folder that is itself a mount point is never accepted (see
    // `is_mount_root`); one that does not exist yet cannot be one.
    if existing && let Some(refusal) = super::prepared_media::mount_root_refusal(&resolved) {
        check.problems.push(format!("{refusal}."));
    }
    if !check.usable() {
        return check;
    }
    let mut created = false;
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
        created = true;
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
                        "This disk's filesystem cannot publish a copy safely (no no-replace rename). Use a Linux filesystem such as ext4 or Btrfs."
                            .into(),
                    );
                }
            }
            Err(error) => check
                .problems
                .push(format!("EmuWiz cannot write in this folder: {error}.")),
        }
    }
    // The store works only while the folder is still on the filesystem it
    // was chosen on, so that filesystem must be recognisable later.
    if check.problems.is_empty() {
        match super::prepared_media::filesystem_at(&resolved) {
            Some(filesystem) => match filesystem.id {
                None => check.problems.push(
                    "EmuWiz cannot tell which disk this folder is on (network and FUSE filesystems report no identity), so it could not notice that disk going away. Choose a folder on a local disk."
                        .into(),
                ),
                Some(id) if super::prepared_media::id_is_device_number(id, filesystem.device) => {
                    check.problems.push(
                        "This disk's filesystem has no identity of its own (XFS, FAT and exFAT report only which connector the disk is on), so EmuWiz could not tell it from another disk connected in its place. Use a disk formatted as ext4 or Btrfs."
                            .into(),
                    );
                }
                Some(_) => {}
            },
            None => check
                .problems
                .push("Which disk this folder is on could not be checked.".into()),
        }
    }
    check.available_bytes = super::prepared_media::available_bytes(&resolved).ok();
    note_disk(&mut check, &resolved);
    if created && !check.usable() {
        // Refused after all: do not leave the folder this call made (it is
        // empty; `remove_dir` removes nothing else).
        let _ = std::fs::remove_dir(folder);
    }
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
    // The folder in use until now keeps its copies; they stay findable.
    if let Ok(Some(old)) = configured_folder_at(setting)
        && folder != Some(old.as_path())
    {
        remember_previous(setting, &old)?;
    }
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
    // Which filesystem the folder is on now, recorded before the choice
    // itself: a saved choice without this record is refused when used.
    let filesystem = super::prepared_media::filesystem_id(folder)
        .ok_or("EmuWiz cannot tell which disk this folder is on.")?;
    record_filesystem(setting, &text, filesystem)?;
    crate::atomic_write_text(setting, &format!("{text}\n")).map_err(|error| error.to_string())?;
    Ok(Some(check))
}

/// The store this profile uses now, and the default one, for display.
pub fn describe_current() -> Result<(PreparedMediaStore, bool), String> {
    let chosen = configured_folder()?.is_some()
        || std::env::var_os(super::prepared_media::PREPARED_MEDIA_DIR_ENV).is_some();
    Ok((PreparedMediaStore::for_profile()?, chosen))
}
