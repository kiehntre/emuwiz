//! Private working copies for Amiga hard-disk (`.hdf`) games.
//!
//! WHDLoad HDF packs boot directly in RetroArch's PUAE core, but the core
//! mounts the image read-write: a game that saves high scores or progress
//! writes into the image itself. Source archives are read-only (ARCHITECTURE
//! rule), so the emulator is never handed the library file. Instead it gets a
//! private copy in EmuWiz's own data folder:
//!
//! `<data>/amiga-hdf-working/<sha256 prefix of the original>/<file name>`
//!
//! The copy is made once, at launch time (never during a preview), and is
//! then reused so the game's own saves survive between sessions. A changed
//! original has a different digest and therefore gets a fresh copy; the old
//! copy, holding the old saves, is left untouched.
//!
//! These copies are persistent game data (they hold saves), not a cache:
//! nothing in EmuWiz prunes them. Blue's Settings page can store them in a
//! folder the user chooses (saved in `amiga-hdf-storage.json`);
//! `EMUWIZ_AMIGA_HDF_WORKING_DIR` remains an advanced override. A storage
//! folder must not be inside a cache or Cargo build folder, which cleanup
//! tools may delete. Changing the folder never moves or deletes copies:
//! earlier folders are remembered and a game's existing copy is still used
//! from wherever it is.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Folder name under the EmuWiz data directory.
pub const WORKING_COPY_DIR: &str = "amiga-hdf-working";

/// Larger images are refused rather than duplicated. WHDLoad HDF packs are
/// typically 1-80 MiB.
pub const MAX_HDF_BYTES: u64 = 1024 * 1024 * 1024;

/// Whether a RetroArch launch for this platform/content needs a working copy.
pub fn applies(platform_id: &str, content: &Path) -> bool {
    platform_id == "Amiga"
        && content
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("hdf"))
}

const PARTIAL_PREFIX: &str = ".partial-";

/// Absolute path that replaces `<data>/amiga-hdf-working` as the home of the
/// persistent working copies.
pub const ROOT_OVERRIDE_ENV: &str = "EMUWIZ_AMIGA_HDF_WORKING_DIR";

/// Minimum space left free after a copy. The reserve is the larger of this
/// and 5% of the drive, so a nearly full system drive is never filled.
pub const MIN_RESERVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

const KEEP_NOTE_NAME: &str = "README-KEEP.txt";
const KEEP_NOTE: &str = "EmuWiz Amiga HDF working copies.\n\nEach folder holds a private, writable copy of an Amiga hard-disk game and the\nsaves the game wrote into it. This is not a cache: deleting it deletes those saves.\nThe original game files are never changed.\n";

fn reserve_bytes(total: u64) -> u64 {
    MIN_RESERVE_BYTES.max(total / 20)
}

fn enough_space(available: u64, total: u64, size: u64) -> bool {
    available >= size.saturating_add(reserve_bytes(total))
}

/// (available, total) bytes on the filesystem holding `directory`.
fn space(directory: &Path) -> Result<(u64, u64), String> {
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(directory.as_os_str().as_bytes())
        .map_err(|_| "working-copy path contains NUL".to_string())?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `name` is a valid NUL-terminated path and `stats` is a
    // writable, correctly sized struct.
    if unsafe { libc::statvfs(name.as_ptr(), &mut stats) } != 0 {
        return Err(format!(
            "cannot check free space: {}",
            std::io::Error::last_os_error()
        ));
    }
    fn widen<T: Into<u64>>(value: T) -> u64 {
        value.into()
    }
    let unit = widen(stats.f_frsize);
    Ok((
        widen(stats.f_bavail).saturating_mul(unit),
        widen(stats.f_blocks).saturating_mul(unit),
    ))
}

fn writer_alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks for existence/permission.
    pid > 0
        && (unsafe { libc::kill(pid, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

/// Removes `.partial-<pid>-...` files in this private copy folder whose
/// writing process no longer exists. Only regular files with that exact
/// prefix are touched; anything else (the copy, other names) is left alone.
fn remove_abandoned_partials(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(rest) = name.to_str().and_then(|n| n.strip_prefix(PARTIAL_PREFIX)) else {
            continue;
        };
        let Some(pid) = rest.split('-').next().and_then(|p| p.parse::<i32>().ok()) else {
            continue;
        };
        let is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);
        if is_file && !writer_alive(pid) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn open_original(original: &Path) -> Result<(File, u64), String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(original)
        .map_err(|e| format!("cannot open the hard-disk image: {e}"))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("cannot read the hard-disk image: {e}"))?;
    if !metadata.file_type().is_file() {
        return Err("the hard-disk image is not a regular file".into());
    }
    if metadata.len() == 0 || metadata.len() > MAX_HDF_BYTES {
        return Err(format!(
            "the hard-disk image size ({} bytes) is outside the supported range",
            metadata.len()
        ));
    }
    Ok((file, metadata.len()))
}

fn ensure_private_dir(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_dir() => Ok(()),
        Ok(_) => Err(format!(
            "{} exists and is not a folder; refusing to use it",
            path.display()
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(path)
                .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
            Ok(())
        }
        Err(e) => Err(format!("cannot inspect {}: {e}", path.display())),
    }
}

/// Returns the working copy for `original` under `root`, creating it from the
/// original's bytes when it does not exist yet. The original is only ever
/// opened read-only.
pub fn working_copy_for(original: &Path, root: &Path) -> Result<PathBuf, String> {
    working_copy_in(original, root, &[])
}

/// Like [`working_copy_for`], but a copy that already exists under one of
/// `earlier_roots` (folders used before the storage folder was changed) is
/// reused where it is, so its saves are never left behind.
pub fn working_copy_in(
    original: &Path,
    root: &Path,
    earlier_roots: &[PathBuf],
) -> Result<PathBuf, String> {
    let name = original
        .file_name()
        .ok_or_else(|| "the hard-disk image has no file name".to_string())?
        .to_owned();
    let (mut file, size) = open_original(original)?;
    let mut bytes = Vec::with_capacity(size as usize);
    (&mut file)
        .take(size + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read the hard-disk image: {e}"))?;
    if bytes.len() as u64 != size {
        return Err("the hard-disk image changed while it was being read".into());
    }
    let digest = Sha256::digest(&bytes);
    let prefix: String = digest.iter().take(12).map(|b| format!("{b:02x}")).collect();

    for earlier in earlier_roots.iter().filter(|r| r.as_path() != root) {
        let candidate = earlier.join(&prefix).join(&name);
        if std::fs::symlink_metadata(&candidate).is_ok_and(|m| m.file_type().is_file()) {
            return Ok(candidate);
        }
    }

    ensure_private_dir(root)?;
    let note = root.join(KEEP_NOTE_NAME);
    if std::fs::symlink_metadata(&note).is_err() {
        let _ = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&note)
            .and_then(|mut f| f.write_all(KEEP_NOTE.as_bytes()));
    }
    let dir = root.join(prefix);
    ensure_private_dir(&dir)?;
    let target = dir.join(&name);
    match std::fs::symlink_metadata(&target) {
        // Reused as-is: it may hold the game's own saves.
        Ok(meta) if meta.file_type().is_file() => return Ok(target),
        Ok(_) => {
            return Err(format!(
                "{} exists and is not a regular file; refusing to use it",
                target.display()
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("cannot inspect {}: {e}", target.display())),
    }

    // A launch that crashed or lost power mid-copy leaves only a `.partial-`
    // file (the copy itself appears atomically, via `link`, once complete).
    // Clear those whose writer is gone, then check there is room.
    remove_abandoned_partials(&dir);
    let (available, total) = space(&dir)?;
    if !enough_space(available, total, size) {
        return Err(format!(
            "not enough free space for this game's private working copy: it needs {} MiB and {} MiB must stay free, but only {} MiB is free in {}. Free some space or set {ROOT_OVERRIDE_ENV} to a folder on a bigger drive.",
            size.div_ceil(1024 * 1024),
            reserve_bytes(total) / (1024 * 1024),
            available / (1024 * 1024),
            dir.display()
        ));
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let mut temp_name = OsString::from(PARTIAL_PREFIX);
    temp_name.push(format!("{}-{nanos}-", std::process::id()));
    temp_name.push(&name);
    let temp = dir.join(temp_name);
    let write = || -> std::io::Result<()> {
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)?;
        out.write_all(&bytes)?;
        out.sync_all()?;
        Ok(())
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("cannot create the working copy: {e}"));
    }
    // `link` never replaces an existing file: a copy another launch finished
    // first wins and keeps its saves.
    let linked = std::fs::hard_link(&temp, &target);
    let _ = std::fs::remove_file(&temp);
    match linked {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("cannot create the working copy: {e}")),
    }
    if let Ok(dir_handle) = File::open(&dir) {
        let _ = dir_handle.sync_all();
    }
    Ok(target)
}

/// Points the emulator argv at the working copy instead of the library file.
/// Fails closed when the content argument cannot be found exactly once.
pub fn redirect_arguments(
    arguments: &mut [OsString],
    content: &Path,
    working_copy: &Path,
) -> Result<(), String> {
    let wanted = content.as_os_str();
    let positions: Vec<usize> = arguments
        .iter()
        .enumerate()
        .filter(|(_, arg)| arg.as_os_str() == wanted)
        .map(|(index, _)| index)
        .collect();
    match positions.as_slice() {
        [index] => {
            arguments[*index] = working_copy.as_os_str().to_owned();
            Ok(())
        }
        _ => Err("the emulator command does not name the hard-disk image exactly once".into()),
    }
}

/// Launch-time entry point: when the command starts an Amiga HDF, make sure
/// the working copy exists and hand that to the emulator.
pub fn redirect_for_launch(
    platform_id: &str,
    content: &Path,
    arguments: &mut [OsString],
) -> Result<Option<PathBuf>, String> {
    if !applies(platform_id, content) {
        return Ok(None);
    }
    let storage = effective_storage()?;
    let copy = working_copy_in(content, &storage.root, &storage.earlier_roots)?;
    redirect_arguments(arguments, content, &copy)?;
    Ok(Some(copy))
}

/// Saved storage choice, in the EmuWiz config folder.
pub const SETTINGS_FILE: &str = "amiga-hdf-storage.json";

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StorageSettings {
    /// Folder chosen in Blue; `None` means the default data folder.
    #[serde(default)]
    pub root: Option<PathBuf>,
    /// Folders used before, still searched for existing copies.
    #[serde(default)]
    pub earlier_roots: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageSource {
    /// `EMUWIZ_AMIGA_HDF_WORKING_DIR` is set.
    Override,
    /// Chosen in Blue's Settings.
    Chosen,
    Default,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectiveStorage {
    pub root: PathBuf,
    pub source: StorageSource,
    /// Other folders that may hold existing copies (default and earlier).
    pub earlier_roots: Vec<PathBuf>,
}

fn default_root() -> Result<PathBuf, String> {
    Ok(crate::app_dirs::data_dir()
        .map_err(|e| format!("EmuWiz data folder unavailable: {e}"))?
        .join(WORKING_COPY_DIR))
}

fn settings_path() -> Result<PathBuf, String> {
    crate::app_dirs::config_path(SETTINGS_FILE)
        .map_err(|e| format!("EmuWiz settings folder unavailable: {e}"))
}

/// Reads the saved choice. A missing file is the default; an unreadable or
/// damaged one is an error, so a launch never silently falls back to a
/// different folder and misses the user's saves.
pub fn load_settings_from(path: &Path) -> Result<StorageSettings, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| format!("{} is damaged: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StorageSettings::default()),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

fn save_settings_to(path: &Path, settings: &StorageSettings) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, bytes).map_err(|e| format!("cannot save {}: {e}", path.display()))?;
    std::fs::rename(&temp, path).map_err(|e| format!("cannot save {}: {e}", path.display()))
}

fn resolve(
    env_value: Option<&Path>,
    settings: &StorageSettings,
    default: PathBuf,
    home: Option<&Path>,
    target_root: Option<&Path>,
) -> Result<EffectiveStorage, String> {
    let (root, source) = match (env_value, &settings.root) {
        (Some(value), _) => (
            check_override(value, home, target_root)?,
            StorageSource::Override,
        ),
        (None, Some(chosen)) => (
            check_override(chosen, home, target_root)?,
            StorageSource::Chosen,
        ),
        (None, None) => (default.clone(), StorageSource::Default),
    };
    let mut earlier_roots = Vec::new();
    for candidate in std::iter::once(&default)
        .chain(settings.root.iter())
        .chain(settings.earlier_roots.iter())
    {
        if candidate != &root && !earlier_roots.contains(candidate) {
            earlier_roots.push(candidate.clone());
        }
    }
    Ok(EffectiveStorage {
        root,
        source,
        earlier_roots,
    })
}

fn env_paths() -> (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>) {
    let value = std::env::var_os(ROOT_OVERRIDE_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let target_root = std::env::var_os("EMUWIZ_TARGET_ROOT").map(PathBuf::from);
    (value, home, target_root)
}

/// Where new working copies go, and where older ones may still be.
pub fn effective_storage() -> Result<EffectiveStorage, String> {
    let settings = load_settings_from(&settings_path()?)?;
    let (value, home, target_root) = env_paths();
    resolve(
        value.as_deref(),
        &settings,
        default_root()?,
        home.as_deref(),
        target_root.as_deref(),
    )
}

/// Where new working copies go.
pub fn working_root() -> Result<PathBuf, String> {
    effective_storage().map(|storage| storage.root)
}

/// Facts shown in Settings for one storage folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageFacts {
    pub available: u64,
    pub total: u64,
    pub reserve: u64,
    /// Game copies already in this folder.
    pub copies: usize,
}

/// Free space is read from the nearest existing folder at or above `root`,
/// which is the filesystem the copies will be written to.
pub fn storage_facts(root: &Path) -> Result<StorageFacts, String> {
    let existing = root
        .ancestors()
        .find(|p| p.is_dir())
        .ok_or_else(|| format!("no existing folder above {}", root.display()))?;
    let (available, total) = space(existing)?;
    let copies = std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .count()
        })
        .unwrap_or(0);
    Ok(StorageFacts {
        available,
        total,
        reserve: reserve_bytes(total),
        copies,
    })
}

/// Checks a folder picked in Settings: absolute, an existing real folder
/// (not a symlink), writable, outside cache/Cargo folders, and with at least
/// the safety reserve free. Writes and removes one probe file.
fn validate_chosen(
    folder: &Path,
    home: Option<&Path>,
    target_root: Option<&Path>,
) -> Result<(), String> {
    check_override(folder, home, target_root)?;
    let meta = std::fs::symlink_metadata(folder)
        .map_err(|e| format!("cannot use {}: {e}", folder.display()))?;
    if !meta.file_type().is_dir() {
        return Err(format!("{} is not a folder", folder.display()));
    }
    let probe = folder.join(format!(".emuwiz-write-check-{}", std::process::id()));
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&probe)
        .map_err(|e| format!("EmuWiz cannot write to {}: {e}", folder.display()))?;
    let _ = std::fs::remove_file(&probe);
    let (available, total) = space(folder)?;
    if available < reserve_bytes(total) {
        return Err(format!(
            "{} has only {} MiB free; at least {} MiB must stay free on that drive",
            folder.display(),
            available / (1024 * 1024),
            reserve_bytes(total) / (1024 * 1024)
        ));
    }
    Ok(())
}

fn record_choice(
    settings: &StorageSettings,
    chosen: Option<PathBuf>,
    previous_root: &Path,
) -> StorageSettings {
    let mut next = settings.clone();
    if !next.earlier_roots.iter().any(|r| r == previous_root) {
        next.earlier_roots.push(previous_root.to_path_buf());
    }
    next.earlier_roots.retain(|r| Some(r) != chosen.as_ref());
    next.root = chosen;
    next
}

/// Copies go in this subfolder of a folder picked in Settings, so they never
/// mix with the user's other files.
pub const CHOSEN_SUBFOLDER: &str = "EmuWiz Amiga HDF copies";

/// Saves `folder` (or the default, for `None`) as the storage folder for new
/// copies. Nothing is moved or deleted: the folder in use until now is
/// remembered so its copies keep being found.
pub fn choose_storage(folder: Option<&Path>) -> Result<EffectiveStorage, String> {
    let path = settings_path()?;
    let settings = load_settings_from(&path)?;
    let (_, home, target_root) = env_paths();
    let current = resolve(
        None,
        &settings,
        default_root()?,
        home.as_deref(),
        target_root.as_deref(),
    )?;
    let chosen = match folder {
        Some(folder) => {
            validate_chosen(folder, home.as_deref(), target_root.as_deref())?;
            Some(folder.join(CHOSEN_SUBFOLDER))
        }
        None => None,
    };
    save_settings_to(&path, &record_choice(&settings, chosen, &current.root))?;
    effective_storage()
}

/// An override must be absolute and outside folders that cleanup tools treat
/// as disposable (`~/.cache`, the Cargo build-target root).
fn check_override(
    path: &Path,
    home: Option<&Path>,
    target_root: Option<&Path>,
) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("{ROOT_OVERRIDE_ENV} must be an absolute path"));
    }
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut disposable: Vec<PathBuf> = Vec::new();
    if let Some(home) = home {
        disposable.push(home.join(".cache"));
    }
    if let Some(target) = target_root {
        disposable.push(target.to_path_buf());
    }
    for root in disposable {
        let root_resolved = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        if path.starts_with(&root) || resolved.starts_with(&root_resolved) {
            return Err(format!(
                "{ROOT_OVERRIDE_ENV} ({}) is inside {}, which cleanup tools may delete; choose a folder for saved games instead",
                path.display(),
                root.display()
            ));
        }
    }
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn original(dir: &Path, bytes: &[u8]) -> PathBuf {
        let path = dir.join("Game_v1.0.hdf");
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn applies_only_to_amiga_hdf() {
        assert!(applies("Amiga", Path::new("/g/a.HDF")));
        assert!(!applies("Amiga", Path::new("/g/a.lha")));
        assert!(!applies("Amiga", Path::new("/g/a.adf")));
        assert!(!applies("AtariST", Path::new("/g/a.hdf")));
    }

    #[test]
    fn copy_is_made_once_reused_and_the_original_never_changes() {
        let src = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let path = original(src.path(), b"RDSK-game-bytes");
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();

        let copy = working_copy_for(&path, data.path()).unwrap();
        assert_ne!(copy, path);
        assert!(copy.starts_with(data.path()));
        assert_eq!(copy.file_name().unwrap(), "Game_v1.0.hdf");
        assert_eq!(std::fs::read(&copy).unwrap(), b"RDSK-game-bytes");

        // The emulator writes a save into the copy; the next launch keeps it.
        std::fs::write(&copy, b"RDSK-game-bytes+save").unwrap();
        let again = working_copy_for(&path, data.path()).unwrap();
        assert_eq!(again, copy);
        assert_eq!(std::fs::read(&again).unwrap(), b"RDSK-game-bytes+save");

        assert_eq!(std::fs::read(&path).unwrap(), b"RDSK-game-bytes");
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before
        );
        // No partial files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(copy.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(leftovers.len(), 1);
    }

    #[test]
    fn a_changed_original_gets_its_own_copy_and_old_saves_stay() {
        let src = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let path = original(src.path(), b"version-one");
        let first = working_copy_for(&path, data.path()).unwrap();
        std::fs::write(&path, b"version-two").unwrap();
        let second = working_copy_for(&path, data.path()).unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read(&first).unwrap(), b"version-one");
        assert_eq!(std::fs::read(&second).unwrap(), b"version-two");
    }

    #[test]
    fn symlinked_or_empty_originals_are_refused() {
        let src = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let real = original(src.path(), b"bytes");
        let link = src.path().join("link.hdf");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(working_copy_for(&link, data.path()).is_err());
        let empty = src.path().join("empty.hdf");
        std::fs::write(&empty, b"").unwrap();
        assert!(working_copy_for(&empty, data.path()).is_err());
    }

    #[test]
    fn a_symlink_planted_at_the_copy_location_is_refused() {
        let src = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let path = original(src.path(), b"bytes");
        let copy = working_copy_for(&path, data.path()).unwrap();
        std::fs::remove_file(&copy).unwrap();
        let elsewhere = src.path().join("elsewhere");
        std::os::unix::fs::symlink(&elsewhere, &copy).unwrap();
        assert!(working_copy_for(&path, data.path()).is_err());
        assert!(!elsewhere.exists());
    }

    #[test]
    fn argv_is_redirected_exactly_once_or_refused() {
        let content = Path::new("/lib/Game.hdf");
        let copy = Path::new("/data/x/Game.hdf");
        let mut args: Vec<OsString> = vec!["-L".into(), "core.so".into(), content.into()];
        redirect_arguments(&mut args, content, copy).unwrap();
        assert_eq!(args[2], OsString::from("/data/x/Game.hdf"));

        let mut none: Vec<OsString> = vec!["-L".into(), "core.so".into()];
        assert!(redirect_arguments(&mut none, content, copy).is_err());
        let mut twice: Vec<OsString> = vec![content.into(), content.into()];
        assert!(redirect_arguments(&mut twice, content, copy).is_err());
    }
    #[test]
    fn an_abandoned_partial_copy_is_cleared_and_the_copy_completes() {
        let src = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let path = original(src.path(), b"full-image");
        // First run creates the folder; simulate a crash mid-copy by a dead
        // writer (pid far beyond pid_max) and a live one (us).
        let copy = working_copy_for(&path, data.path()).unwrap();
        let dir = copy.parent().unwrap().to_path_buf();
        std::fs::remove_file(&copy).unwrap();
        let dead = dir.join(".partial-2147483000-1-Game_v1.0.hdf");
        std::fs::write(&dead, b"full-im").unwrap();
        let live = dir.join(format!(".partial-{}-1-Game_v1.0.hdf", std::process::id()));
        std::fs::write(&live, b"full").unwrap();

        let again = working_copy_for(&path, data.path()).unwrap();
        assert_eq!(std::fs::read(&again).unwrap(), b"full-image");
        assert!(!dead.exists(), "abandoned partial removed");
        assert!(live.exists(), "a live writer's partial is never touched");
    }

    #[test]
    fn a_failed_write_leaves_no_copy_and_no_partial() {
        use std::os::unix::fs::PermissionsExt;
        let src = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let path = original(src.path(), b"image");
        let copy = working_copy_for(&path, data.path()).unwrap();
        let dir = copy.parent().unwrap().to_path_buf();
        std::fs::remove_file(&copy).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let result = working_copy_for(&path, data.path());
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        if result.is_ok() {
            // Running as root ignores directory permissions; nothing to check.
            return;
        }
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
    }

    #[test]
    fn space_check_keeps_a_reserve_of_2_gib_or_5_percent() {
        const GIB: u64 = 1024 * 1024 * 1024;
        // Small drive: the 2 GiB floor applies.
        assert!(enough_space(10 + 2 * GIB, 20 * GIB, 10));
        assert!(!enough_space(9 + 2 * GIB, 20 * GIB, 10));
        // 1.5 TiB drive (like the owner's system drive): 5% (~77 GiB) applies.
        let total = 1536 * GIB;
        assert!(!enough_space(70 * GIB, total, GIB));
        assert!(enough_space(total / 20 + GIB, total, GIB));
        assert!(!enough_space(u64::MAX - 1, u64::MAX, u64::MAX));
    }

    #[test]
    fn root_override_must_be_absolute_and_outside_disposable_folders() {
        let home = Path::new("/home/u");
        let target = Path::new("/mnt/nvme2/emuwiz-cargo-targets");
        assert!(check_override(Path::new("rel/dir"), Some(home), None).is_err());
        assert!(check_override(Path::new("/home/u/.cache/hdf"), Some(home), None).is_err());
        assert!(
            check_override(
                Path::new("/mnt/nvme2/emuwiz-cargo-targets/hdf"),
                Some(home),
                Some(target)
            )
            .is_err()
        );
        assert_eq!(
            check_override(Path::new("/mnt/bulk/amiga-saves"), Some(home), Some(target)).unwrap(),
            Path::new("/mnt/bulk/amiga-saves")
        );
    }

    #[test]
    fn the_working_root_carries_a_keep_note_and_copies_are_only_made_on_request() {
        let src = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join(WORKING_COPY_DIR);
        // Nothing exists until a launch asks for this one game.
        assert!(!root.exists());
        let path = original(src.path(), b"RDSK");
        working_copy_for(&path, &root).unwrap();
        let note = std::fs::read_to_string(root.join(KEEP_NOTE_NAME)).unwrap();
        assert!(note.contains("not a cache"), "{note}");
        let folders = std::fs::read_dir(&root)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_type().unwrap().is_dir())
            .count();
        assert_eq!(folders, 1);
    }

    #[test]
    fn storage_resolution_prefers_override_then_choice_then_default() {
        let default = PathBuf::from("/data/amiga-hdf-working");
        let chosen = StorageSettings {
            root: Some(PathBuf::from("/mnt/bulk/EmuWiz Amiga HDF copies")),
            earlier_roots: vec![PathBuf::from("/mnt/old"), default.clone()],
        };
        let none = StorageSettings::default();
        let home = Some(Path::new("/home/u"));

        let d = resolve(None, &none, default.clone(), home, None).unwrap();
        assert_eq!(
            (d.root.clone(), d.source),
            (default.clone(), StorageSource::Default)
        );
        assert!(d.earlier_roots.is_empty());

        let c = resolve(None, &chosen, default.clone(), home, None).unwrap();
        assert_eq!(c.source, StorageSource::Chosen);
        assert_eq!(c.root, Path::new("/mnt/bulk/EmuWiz Amiga HDF copies"));
        // The default and every earlier folder are still searched, once each.
        assert_eq!(
            c.earlier_roots,
            vec![default.clone(), PathBuf::from("/mnt/old")]
        );

        let o = resolve(
            Some(Path::new("/mnt/x")),
            &chosen,
            default.clone(),
            home,
            None,
        )
        .unwrap();
        assert_eq!(
            (o.root.clone(), o.source),
            (PathBuf::from("/mnt/x"), StorageSource::Override)
        );
        assert!(
            o.earlier_roots
                .contains(&PathBuf::from("/mnt/bulk/EmuWiz Amiga HDF copies"))
        );

        let cache = StorageSettings {
            root: Some(PathBuf::from("/home/u/.cache/x")),
            ..none
        };
        assert!(resolve(None, &cache, default, home, None).is_err());
    }

    #[test]
    fn a_copy_in_an_earlier_folder_is_reused_not_duplicated() {
        let src = tempfile::tempdir().unwrap();
        let old = tempfile::tempdir().unwrap();
        let new = tempfile::tempdir().unwrap();
        let path = original(src.path(), b"RDSK-image");
        let old_copy = working_copy_for(&path, old.path()).unwrap();
        std::fs::write(&old_copy, b"RDSK-image+save").unwrap();

        let found = working_copy_in(&path, new.path(), &[old.path().to_path_buf()]).unwrap();
        assert_eq!(found, old_copy);
        assert_eq!(std::fs::read(&found).unwrap(), b"RDSK-image+save");
        assert!(std::fs::read_dir(new.path()).unwrap().next().is_none());

        // A game with no earlier copy goes to the new folder.
        let other = src.path().join("Other.hdf");
        std::fs::write(&other, b"RDSK-other").unwrap();
        let fresh = working_copy_in(&other, new.path(), &[old.path().to_path_buf()]).unwrap();
        assert!(fresh.starts_with(new.path()));
    }

    #[test]
    fn changing_the_folder_remembers_the_old_one_and_damaged_settings_fail_closed() {
        let start = StorageSettings::default();
        let default = PathBuf::from("/data/amiga-hdf-working");
        let first = record_choice(&start, Some(PathBuf::from("/mnt/a/C")), &default);
        assert_eq!(first.root, Some(PathBuf::from("/mnt/a/C")));
        assert_eq!(first.earlier_roots, vec![default.clone()]);
        let back = record_choice(&first, None, Path::new("/mnt/a/C"));
        assert_eq!(back.root, None);
        assert_eq!(back.earlier_roots, vec![default, PathBuf::from("/mnt/a/C")]);

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(SETTINGS_FILE);
        assert_eq!(
            load_settings_from(&file).unwrap(),
            StorageSettings::default()
        );
        save_settings_to(&file, &back).unwrap();
        assert_eq!(load_settings_from(&file).unwrap(), back);
        std::fs::write(&file, b"{not json").unwrap();
        assert!(load_settings_from(&file).is_err());
    }

    #[test]
    fn a_chosen_folder_must_be_a_real_writable_folder() {
        let dir = tempfile::tempdir().unwrap();
        let home = Some(Path::new("/nonexistent-home"));
        assert!(validate_chosen(dir.path(), home, None).is_ok());
        // The write check leaves nothing behind.
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
        assert!(validate_chosen(&dir.path().join("missing"), home, None).is_err());
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(dir.path(), &link).unwrap();
        assert!(validate_chosen(&link, home, None).is_err());
        let facts = storage_facts(&dir.path().join("not-yet")).unwrap();
        assert_eq!(facts.copies, 0);
        assert!(facts.reserve >= MIN_RESERVE_BYTES);
    }
}
