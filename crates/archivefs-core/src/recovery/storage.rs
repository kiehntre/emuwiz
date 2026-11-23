//! Where prepared copies are kept.
//!
//! By default `<data>/prepared-media`. The person may choose another folder
//! (for example on a larger disk); the choice is one line in
//! `prepared_media_folder.txt` in EmuWiz's configuration folder and is
//! accepted only after [`check_folder`] passes, and is re-checked each time
//! it is used ([`still_safe`]). The person says which disk the folder is on
//! by that disk's filesystem UUID; the folder is accepted only when it is on
//! that filesystem (see [`super::disk_identity`]), the UUID is saved with
//! the choice, and the folder is used only while it is still on that
//! filesystem. Copies already in another folder are not
//! moved; folders used earlier are remembered, so their copies are still
//! found and still covered by the launch gate.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::prepared_media::PreparedMediaStore;

/// File name inside EmuWiz's configuration folder.
pub const STORAGE_SETTING_FILE: &str = "prepared_media_folder.txt";
const MAX_SETTING_BYTES: u64 = 8 * 1024;

/// Where the choice is saved, for callers that pass it on explicitly.
pub fn setting_file() -> Result<PathBuf, String> {
    setting_path()
}

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

/// Which filesystem each chosen folder must be on, beside the setting: the
/// UUID the person named when choosing it, saved only after the folder was
/// verified to be on that filesystem. A folder merely existing later proves
/// nothing: an unmounted disk leaves its empty mount point behind, and
/// another disk can be mounted in its place. The store compares the
/// filesystem's identity each time the folder is used.
///
/// Format 1 (never in an installed build) recorded whatever filesystem the
/// folder was on when it was chosen, which is not proof of the intended
/// disk; such a record is not trusted and the folder must be chosen again.
const FILESYSTEMS_FILE: &str = "prepared_media_folder_filesystems.json";
const FILESYSTEMS_FORMAT: u32 = 2;
const MAX_FILESYSTEMS_BYTES: u64 = 256 * 1024;
const MAX_RECORDED_FOLDERS: usize = 64;

#[derive(serde::Deserialize, Serialize)]
struct RecordedFilesystems {
    format_version: u32,
    /// Folder path as chosen -> the UUID of the filesystem it must be on.
    folders: std::collections::BTreeMap<String, String>,
}

#[derive(serde::Deserialize)]
struct RecordVersion {
    format_version: u32,
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
    let version: RecordVersion = serde_json::from_str(&text).map_err(|_| unusable())?;
    if version.format_version == 1 {
        return Err(format!(
            "{} was written by a build that did not verify which disk a folder is on; choose the folder for prepared copies again, naming its disk",
            path.display()
        ));
    }
    if version.format_version != FILESYSTEMS_FORMAT {
        return Err(unusable());
    }
    let recorded: RecordedFilesystems = serde_json::from_str(&text).map_err(|_| unusable())?;
    // A UUID that cannot be read back is a damaged record, not "no record".
    if recorded
        .folders
        .values()
        .any(|uuid| super::disk_identity::ExpectedFilesystem::parse(uuid).is_err())
    {
        return Err(unusable());
    }
    Ok(recorded)
}

/// The id of the filesystem `folder` must be on (the identity of the UUID
/// saved when it was chosen), if one was recorded. A record that cannot be
/// read is an error, never "no record".
pub(crate) fn recorded_filesystem(folder: &Path) -> Result<Option<u64>, String> {
    recorded_filesystem_at(&setting_path()?, folder)
}

pub(crate) fn recorded_filesystem_at(setting: &Path, folder: &Path) -> Result<Option<u64>, String> {
    let Some(folder) = folder.to_str() else {
        return Ok(None);
    };
    Ok(read_recorded(setting)?
        .folders
        .get(folder)
        .and_then(|uuid| super::disk_identity::ExpectedFilesystem::parse(uuid).ok())
        .map(|expected| expected.id()))
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

fn record_filesystem(setting: &Path, folder: &str, uuid: &str) -> Result<(), String> {
    // A record that cannot be read is replaced: the folder being chosen now
    // is the one whose disk is known for certain. Earlier folders lose their
    // entries with it and are then not used until chosen again.
    let mut recorded = read_recorded(setting).unwrap_or_else(|_| RecordedFilesystems {
        format_version: FILESYSTEMS_FORMAT,
        folders: std::collections::BTreeMap::new(),
    });
    recorded
        .folders
        .insert(folder.to_string(), uuid.to_string());
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
    /// The UUID of the filesystem the folder (or, when it does not exist
    /// yet, the folder above it) was verified to be on. `None` until that
    /// verification passed; nothing is written anywhere before it has.
    pub filesystem_uuid: Option<String>,
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

/// Examines `folder`. `filesystem_uuid` names the disk the person means (the
/// UUID of its ext4 filesystem); `library_folders` are the game folders
/// EmuWiz scans: prepared copies inside one would be listed as games.
///
/// Nothing is created. The first thing established is which disk the folder
/// is on, through a handle on the folder itself or, when it does not exist
/// yet, on the folder above it: unless that is the named disk, the check
/// stops there and nothing has been written. Only on the verified disk, and
/// only for a folder that exists, a probe folder is created inside it and
/// removed again (the store needs a no-replace rename).
pub fn check_folder(
    folder: &Path,
    library_folders: &[PathBuf],
    filesystem_uuid: Option<&str>,
) -> StorageCheck {
    let mut check = StorageCheck {
        folder: folder.to_path_buf(),
        problems: Vec::new(),
        notes: Vec::new(),
        available_bytes: None,
        same_disk_as_data: None,
        filesystem_uuid: None,
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
    let anchor = if existing {
        std::fs::canonicalize(folder).ok()
    } else {
        folder
            .parent()
            .filter(|parent| std::fs::metadata(parent).is_ok_and(|m| m.is_dir()))
            .and_then(|parent| std::fs::canonicalize(parent).ok())
    };
    let Some(anchor) = anchor else {
        check.problems.push(if existing {
            "The folder could not be opened.".into()
        } else {
            "The folder above this one does not exist. EmuWiz creates only the last folder of the path, and only on the disk you name.".into()
        });
        return check;
    };
    let resolved = if existing {
        anchor.clone()
    } else {
        match folder.file_name() {
            Some(name) => anchor.join(name),
            None => {
                check
                    .problems
                    .push("Give the full path of a folder.".into());
                return check;
            }
        }
    };
    // Which disk: named by the person, verified through a handle. Until
    // this passes nothing else is examined and nothing is written.
    let Some(filesystem_uuid) = filesystem_uuid.filter(|uuid| !uuid.trim().is_empty()) else {
        check.problems.push(format!(
            "Say which disk this folder is on: choose the disk, or give the UUID of its ext4 filesystem. A folder being there does not prove the right disk is connected ({}).",
            super::disk_identity::HOW_TO_FIND_UUID
        ));
        return check;
    };
    let expected = match super::disk_identity::ExpectedFilesystem::parse(filesystem_uuid) {
        Ok(expected) => expected,
        Err(detail) => {
            check.problems.push(format!("{detail}."));
            return check;
        }
    };
    // The handle of the verified folder is kept: the only thing this check
    // ever writes (the probe below) goes through it, not through the path.
    let verified = crate::safe_read::open_directory_no_symlinks(&anchor)
        .map_err(|refusal| format!("{} is not a plain folder ({refusal:?})", anchor.display()))
        .and_then(|directory| {
            super::disk_identity::verify_on(&directory, &anchor, &expected).map(|()| directory)
        });
    let directory = match verified {
        Ok(directory) => directory,
        Err(detail) => {
            check.problems.push(format!("{detail}."));
            return check;
        }
    };
    check.filesystem_uuid = Some(expected.uuid().to_string());
    check = location_checks(check, &resolved, library_folders);
    // A folder that is itself a mount point is never accepted (see
    // `is_mount_root`); one that does not exist yet cannot be one.
    if existing && let Some(refusal) = super::prepared_media::mount_root_refusal(&resolved) {
        check.problems.push(format!("{refusal}."));
    }
    if !check.usable() {
        return check;
    }
    if !existing {
        check
            .notes
            .push("The folder does not exist yet; it will be created on that disk.".into());
        check.available_bytes = super::prepared_media::available_bytes(&anchor).ok();
        note_disk(&mut check, &anchor);
        return check;
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
        // filesystem without it cannot hold the store. Tried on an empty
        // folder made, renamed and removed through the verified handle.
        match probe_noreplace_rename(&directory) {
            Ok(true) => {}
            Ok(false) => check.problems.push(
                "This disk's filesystem cannot publish a copy safely (no no-replace rename)."
                    .into(),
            ),
            Err(error) => check
                .problems
                .push(format!("EmuWiz cannot write in this folder: {error}.")),
        }
    }
    check.available_bytes = super::prepared_media::available_bytes(&resolved).ok();
    note_disk(&mut check, &resolved);
    check
}

/// Makes an empty folder inside `directory`, renames it without replacing,
/// and removes it again, all relative to the open handle: whatever the path
/// leads to by now, nothing is written anywhere but in that folder. `false`
/// when the filesystem has no no-replace rename.
fn probe_noreplace_rename(directory: &std::fs::File) -> std::io::Result<bool> {
    use std::os::fd::AsRawFd;
    let fd = directory.as_raw_fd();
    // Unpredictable enough that no other folder has this name.
    let stem = {
        use std::hash::{BuildHasher, Hasher};
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u32(std::process::id());
        format!("{:016x}", hasher.finish())
    };
    let name = |suffix: &str| {
        std::ffi::CString::new(format!(".emuwiz-storage-check-{stem}{suffix}"))
            .map_err(|_| std::io::Error::other("name"))
    };
    let (from, to) = (name("-a")?, name("-b")?);
    // SAFETY: both names are NUL-terminated; `fd` is an open folder.
    if unsafe { libc::mkdirat(fd, from.as_ptr(), 0o700) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: as above.
    let renamed =
        unsafe { libc::renameat2(fd, from.as_ptr(), fd, to.as_ptr(), libc::RENAME_NOREPLACE) } == 0;
    // SAFETY: as above. Only the empty folder this call made is removed.
    unsafe {
        libc::unlinkat(fd, to.as_ptr(), libc::AT_REMOVEDIR);
        libc::unlinkat(fd, from.as_ptr(), libc::AT_REMOVEDIR);
    }
    Ok(renamed)
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

/// Saves `folder` as the place for prepared copies, or clears the choice
/// with `None`. Copies already prepared elsewhere stay where they are.
///
/// `filesystem_uuid` names the disk the folder is meant to be on and is
/// required with a folder. The folder must pass [`check_folder`] for that
/// disk. One that does not exist yet is created (0700) only after the folder
/// above it was verified to be on that disk, through that folder's handle,
/// and is then checked again; if it fails after all, the empty folder this
/// call made is removed. The UUID is saved with the choice.
pub fn set_folder(
    folder: Option<&Path>,
    library_folders: &[PathBuf],
    filesystem_uuid: Option<&str>,
) -> Result<Option<StorageCheck>, String> {
    set_folder_at(&setting_path()?, folder, library_folders, filesystem_uuid)
}

pub fn set_folder_at(
    setting: &Path,
    folder: Option<&Path>,
    library_folders: &[PathBuf],
    filesystem_uuid: Option<&str>,
) -> Result<Option<StorageCheck>, String> {
    let Some(folder) = folder else {
        // The folder in use until now keeps its copies; they stay findable.
        if let Ok(Some(old)) = configured_folder_at(setting) {
            remember_previous(setting, &old)?;
        }
        match std::fs::remove_file(setting) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("{}: {error}", setting.display())),
        }
        return Ok(None);
    };
    let text = folder
        .to_str()
        .ok_or("The folder name is not valid text.")?
        .to_string();
    let mut check = check_folder(folder, library_folders, filesystem_uuid);
    if !check.usable() {
        return Err(check.problems.join(" "));
    }
    if std::fs::symlink_metadata(folder).is_err() {
        check = create_on_verified_disk(folder, library_folders, filesystem_uuid)?;
    }
    // Only a check that verified the disk carries its UUID.
    let Some(uuid) = check.filesystem_uuid.clone() else {
        return Err("Which disk this folder is on was not verified.".into());
    };
    if let Some(parent) = setting.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    if let Ok(Some(old)) = configured_folder_at(setting)
        && old != folder
    {
        remember_previous(setting, &old)?;
    }
    // The disk the folder must be on, recorded before the choice itself: a
    // saved choice without this record is refused when used.
    record_filesystem(setting, &text, &uuid)?;
    crate::atomic_write_text(setting, &format!("{text}\n")).map_err(|error| error.to_string())?;
    Ok(Some(check))
}

/// Creates the last folder of `folder` (0700) through a handle on the folder
/// above it, after proving through that same handle that it is on the named
/// disk, then checks the new folder like an existing one. A mount change
/// between the proof and the creation cannot redirect it: the handle is of
/// the verified folder, not of the path.
pub(super) fn create_on_verified_disk(
    folder: &Path,
    library_folders: &[PathBuf],
    filesystem_uuid: Option<&str>,
) -> Result<StorageCheck, String> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    let expected = super::disk_identity::ExpectedFilesystem::parse(
        filesystem_uuid.ok_or("Which disk this folder is on was not said.")?,
    )?;
    let (Some(parent), Some(name)) = (folder.parent(), folder.file_name()) else {
        return Err("Give the full path of a folder.".into());
    };
    let parent = std::fs::canonicalize(parent).map_err(|error| error.to_string())?;
    let above = crate::safe_read::open_directory_no_symlinks(&parent)
        .map_err(|refusal| format!("{} is not a plain folder ({refusal:?})", parent.display()))?;
    super::disk_identity::verify_on(&above, &parent, &expected)?;
    let c_name = std::ffi::CString::new(name.as_bytes())
        .map_err(|_| "The folder name is not valid.".to_string())?;
    // SAFETY: `c_name` is NUL-terminated and `above` is an open folder.
    if unsafe { libc::mkdirat(above.as_raw_fd(), c_name.as_ptr(), 0o700) } != 0 {
        return Err(format!(
            "The folder could not be created: {}.",
            std::io::Error::last_os_error()
        ));
    }
    let remove = || {
        // Refused after all: do not leave the folder this call made (it is
        // empty; this removes nothing else).
        // SAFETY: as above.
        unsafe { libc::unlinkat(above.as_raw_fd(), c_name.as_ptr(), libc::AT_REMOVEDIR) };
    };
    // SAFETY: as above; the new folder is opened through the same handle,
    // never through a link.
    let made = unsafe {
        libc::openat(
            above.as_raw_fd(),
            c_name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if made < 0 {
        remove();
        return Err("The new folder could not be opened.".into());
    }
    // SAFETY: a fresh descriptor this function owns.
    let made = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(made) };
    if let Err(detail) = super::disk_identity::verify_on(&made, folder, &expected) {
        remove();
        return Err(detail);
    }
    let check = check_folder(folder, library_folders, filesystem_uuid);
    if !check.usable() || check.filesystem_uuid.is_none() {
        remove();
        return Err(check.problems.join(" "));
    }
    Ok(check)
}

/// The store this profile uses now, and the default one, for display.
pub fn describe_current() -> Result<(PreparedMediaStore, bool), String> {
    let chosen = configured_folder()?.is_some()
        || std::env::var_os(super::prepared_media::PREPARED_MEDIA_DIR_ENV).is_some();
    Ok((PreparedMediaStore::for_profile()?, chosen))
}
