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

/// Which filesystem object a storage folder is: recorded when a folder is
/// chosen and when it receives its first copy, and checked at every Play.
/// An unmounted drive leaves an empty mount-point folder on another
/// filesystem (different inode and filesystem); a replaced folder has a new
/// inode. Either way the identity no longer matches and Play is refused.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FolderIdentity {
    pub path: PathBuf,
    pub dev: u64,
    pub ino: u64,
    /// statvfs filesystem id: stable across re-plugging for most Linux
    /// filesystems, where the device number may change.
    pub fsid: u64,
}

/// Opens `path` as a directory without following a symlink at its last
/// component, and reads its identity from the open handle.
pub fn identify_folder(path: &Path) -> std::io::Result<FolderIdentity> {
    use std::os::unix::fs::MetadataExt;
    let dir = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)?;
    let meta = dir.metadata()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `dir` is an open descriptor and `stats` is a writable struct.
    if unsafe { libc::fstatvfs(std::os::fd::AsRawFd::as_raw_fd(&dir), &mut stats) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    #[allow(clippy::useless_conversion)]
    let fsid = u64::from(stats.f_fsid);
    Ok(FolderIdentity {
        path: path.to_path_buf(),
        dev: meta.dev(),
        ino: meta.ino(),
        fsid,
    })
}

/// Same inode on the same filesystem. The filesystem id is preferred (it
/// survives re-plugging on most Linux filesystems, where the device number
/// can change); the device number is used only when an id is unavailable.
fn same_folder(recorded: &FolderIdentity, now: &FolderIdentity) -> bool {
    let same_filesystem = if recorded.fsid != 0 && now.fsid != 0 {
        recorded.fsid == now.fsid
    } else {
        recorded.dev == now.dev
    };
    recorded.ino == now.ino && same_filesystem
}

/// Whether a folder's identity cannot tell its drive apart from another one
/// plugged into the same port. FAT/exFAT (and some others) report a
/// filesystem id that is only the device number, or none at all; with a
/// fixed root inode, a different stick would then look identical.
fn ambiguous_identity(identity: &FolderIdentity) -> bool {
    let dev = identity.dev;
    let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff);
    let minor = (dev & 0xff) | ((dev >> 12) & !0xff);
    let kernel_encoded = (minor & 0xff) | (major << 8) | ((minor & !0xff) << 12);
    identity.fsid == 0 || identity.fsid == kernel_encoded
}

const AMBIGUOUS_DRIVE: &str = "EmuWiz cannot reliably recognise the drive this folder is on: drives in this format (for example FAT or exFAT) look the same as any other drive plugged in at the same place, so saves could end up on the wrong drive. Choose a folder on a Linux-formatted drive (ext4 or btrfs) in Settings > Amiga hard-disk game copies.";

/// Refuses a storage folder whose drive cannot be told apart from another.
fn refuse_ambiguous(identity: &FolderIdentity) -> Result<(), String> {
    if ambiguous_identity(identity) {
        return Err(format!("{}: {AMBIGUOUS_DRIVE}", identity.path.display()));
    }
    Ok(())
}

/// Every Play: each storage folder other than EmuWiz's own data folder
/// (chosen, override, earlier or used ones) must be on a drive EmuWiz can
/// recognise, or a different drive could be mistaken for it and saves
/// split. When the copies folder does not exist yet, the folder it would be
/// created in is checked instead.
fn refuse_ambiguous_storage(storage: &EffectiveStorage, default: &Path) -> Result<(), String> {
    let mut folders: Vec<&Path> = vec![storage.root.as_path()];
    folders.extend(storage.earlier_roots.iter().map(PathBuf::as_path));
    folders.extend(storage.used_roots.iter().map(PathBuf::as_path));
    for folder in folders {
        if folder == default {
            continue;
        }
        let checked = match identify_folder(folder) {
            Ok(identity) => identity,
            Err(_) if folder == storage.root => match folder.parent().map(identify_folder) {
                Some(Ok(identity)) => identity,
                _ => continue,
            },
            Err(_) => continue,
        };
        refuse_ambiguous(&checked)?;
    }
    Ok(())
}

const RECONNECT_HELP: &str = "Reconnect or mount the drive and try again. If the folder really moved, choose it again in Settings > Amiga hard-disk game copies (or use \"Accept as it is now\" there); if it is gone for good, choose Forget there, and its saves will no longer be used.";

/// Refuses unless `folder` (and its parent, when that was the folder the
/// user picked) is the same filesystem object that was recorded.
fn verify_recorded(folder: &Path, identities: &[FolderIdentity]) -> Result<(), String> {
    for candidate in [Some(folder), folder.parent()].into_iter().flatten() {
        let Some(recorded) = identities.iter().find(|i| i.path == candidate) else {
            continue;
        };
        match identify_folder(candidate) {
            Ok(now) if same_folder(recorded, &now) => {}
            Ok(_) => {
                return Err(format!(
                    "{} is not the folder that holds your Amiga hard-disk game copies: it is on a different drive or was replaced (is the drive mounted?). Nothing was created. {RECONNECT_HELP}",
                    candidate.display()
                ));
            }
            Err(e) => {
                return Err(format!(
                    "{} is not available ({e}). Nothing was created. {RECONNECT_HELP}",
                    candidate.display()
                ));
            }
        }
    }
    Ok(())
}

/// Creates the storage folder itself if needed, but never the folder above
/// it: a picked folder (or drive) that has gone missing must not be silently
/// recreated on another disk.
fn ensure_storage_root(root: &Path) -> Result<(), String> {
    if let Some(parent) = root.parent()
        && !std::fs::symlink_metadata(parent).is_ok_and(|m| m.file_type().is_dir())
    {
        return Err(format!(
            "{} is not available; reconnect it or choose another folder in Settings > Amiga hard-disk game copies",
            parent.display()
        ));
    }
    ensure_private_dir(root)
}

/// Returns the working copy for `original` under `root`, creating it from the
/// original's bytes when it does not exist yet. The original is only ever
/// opened read-only.
pub fn working_copy_for(original: &Path, root: &Path) -> Result<PathBuf, String> {
    working_copy_in(original, root, &[], &[], &[], &mut |_| Ok(()))
}

/// What one storage folder says about a game's copy.
enum Probe {
    Found(PathBuf),
    Absent,
    Unavailable(String),
}

fn probe(root: &Path, prefix: &str, name: &std::ffi::OsStr, must_exist: bool) -> Probe {
    let candidate = root.join(prefix).join(name);
    match std::fs::symlink_metadata(&candidate) {
        Ok(meta) if meta.file_type().is_file() => Probe::Found(candidate),
        Ok(_) => Probe::Unavailable(format!(
            "{} exists and is not a regular file",
            candidate.display()
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            match std::fs::symlink_metadata(root) {
                Ok(meta) if meta.file_type().is_dir() => Probe::Absent,
                Ok(_) => Probe::Unavailable(format!("{} is not a folder", root.display())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && !must_exist => Probe::Absent,
                Err(e) => Probe::Unavailable(format!("{}: {e}", root.display())),
            }
        }
        Err(e) => Probe::Unavailable(format!("{}: {e}", candidate.display())),
    }
}

/// Like [`working_copy_for`], but the game's copy is looked for in `root`
/// and every folder in `earlier_roots` (folders used before the storage
/// folder was changed), and reused where it is so its saves are never left
/// behind. Folders in `must_exist` are known to have held copies: if one is
/// missing or unreadable (an unplugged drive, say) the launch is refused
/// rather than starting a second copy that would split the saves. A copy
/// found in two places is refused too. Every folder with a recorded
/// identity in `identities` (and the picked folder above `root`) must still
/// be that same filesystem object, checked before searching and again just
/// before anything is created. `before_first_copy` runs before the first
/// copy is written into `root`, with its identity, so it can be remembered.
pub fn working_copy_in(
    original: &Path,
    root: &Path,
    earlier_roots: &[PathBuf],
    must_exist: &[PathBuf],
    identities: &[FolderIdentity],
    before_first_copy: &mut dyn FnMut(&FolderIdentity) -> Result<(), String>,
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

    let mut searched: Vec<&Path> = vec![root];
    for earlier in earlier_roots {
        if !searched.contains(&earlier.as_path()) {
            searched.push(earlier);
        }
    }
    for folder in &searched {
        verify_recorded(folder, identities)?;
    }
    let mut found = Vec::new();
    for folder in &searched {
        let required = must_exist.iter().any(|r| r.as_path() == *folder);
        match probe(folder, &prefix, &name, required) {
            Probe::Found(path) => found.push(path),
            Probe::Absent => {}
            Probe::Unavailable(why) => {
                return Err(format!(
                    "a folder that holds Amiga hard-disk game copies and saves is not available ({why}). Reconnect it and try again. If it is gone for good, choose Forget for it in Settings > Amiga hard-disk game copies; its saves will then no longer be used."
                ));
            }
        }
    }
    match found.as_slice() {
        [one] => return Ok(one.clone()),
        [] => {}
        many => {
            let places: Vec<String> = many.iter().map(|p| p.display().to_string()).collect();
            return Err(format!(
                "this game has private copies in more than one place ({}); each may hold different saves. Keep one: move or rename the other yourself, then try again.",
                places.join(", ")
            ));
        }
    }

    // Re-check right before creating anything: the drive may have gone
    // between the search and now.
    verify_recorded(root, identities)?;
    ensure_storage_root(root)?;
    let root_identity =
        identify_folder(root).map_err(|e| format!("cannot open {}: {e}", root.display()))?;
    if let Some(parent) = root.parent()
        && identities.iter().any(|i| i.path == parent)
        && identify_folder(parent).map(|now| now.dev).ok() != Some(root_identity.dev)
    {
        return Err(format!(
            "{} is not on the same drive as the folder you chose; nothing was copied",
            root.display()
        ));
    }
    verify_recorded(root, identities)?;
    if !must_exist.iter().any(|r| r.as_path() == root) {
        before_first_copy(&root_identity)?;
    }
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
    let write = || -> std::io::Result<(u64, u64)> {
        use std::os::unix::fs::MetadataExt;
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)?;
        out.write_all(&bytes)?;
        out.sync_all()?;
        let meta = out.metadata()?;
        Ok((meta.dev(), meta.ino()))
    };
    let written = match write() {
        Ok(id) => id,
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            return Err(format!("cannot create the working copy: {e}"));
        }
    };
    // `link` never replaces an existing file: a copy another launch finished
    // first wins and keeps its saves.
    let linked = std::fs::hard_link(&temp, &target);
    let _ = std::fs::remove_file(&temp);
    let created = match linked {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(e) => return Err(format!("cannot create the working copy: {e}")),
    };
    check_landed_copy(root, &root_identity, &target, created, written)?;
    if let Ok(dir_handle) = File::open(&dir) {
        let _ = dir_handle.sync_all();
    }
    Ok(target)
}

/// After linking: the copy must have landed in the folder checked before.
/// If the folder was swapped meanwhile, the copy this call just created (it
/// holds no saves yet) is removed and the launch refused. Any other file at
/// `target` (an existing copy, or one substituted by another launch or
/// drive) may hold saves and is never removed: only a file whose device and
/// inode are exactly those this call wrote.
fn check_landed_copy(
    root: &Path,
    root_identity: &FolderIdentity,
    target: &Path,
    created: bool,
    written: (u64, u64),
) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let landed = std::fs::symlink_metadata(target).map(|m| (m.dev(), m.ino()));
    let still_same = identify_folder(root).is_ok_and(|now| same_folder(root_identity, &now));
    if still_same && landed.as_ref().ok().map(|l| l.0) == Some(root_identity.dev) {
        return Ok(());
    }
    if created && landed.as_ref().ok() == Some(&written) {
        let _ = std::fs::remove_file(target);
    }
    Err(format!(
        "{} changed while the copy was being made; nothing was kept. {RECONNECT_HELP}",
        root.display()
    ))
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
    refuse_ambiguous_storage(&storage, &default_root()?)?;
    let copy = working_copy_in(
        content,
        &storage.root,
        &storage.earlier_roots,
        &storage.used_roots,
        &storage.identities,
        &mut |root| remember_used_root(root),
    )?;
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
    /// Folders that have held a copy (recorded before the first copy is
    /// written there, including an `EMUWIZ_AMIGA_HDF_WORKING_DIR` folder).
    /// They are always searched, and must be reachable for a launch.
    #[serde(default)]
    pub used_roots: Vec<PathBuf>,
    /// Filesystem identity of the picked folder and of every used folder.
    #[serde(default)]
    pub identities: Vec<FolderIdentity>,
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
    /// Folders known to have held copies; they must be reachable.
    pub used_roots: Vec<PathBuf>,
    pub identities: Vec<FolderIdentity>,
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

/// Runs a read-modify-write of the settings file under an exclusive lock, so
/// Settings and a launch recording a used folder never lose each other's
/// update (a lost folder record would weaken the must-exist check).
fn with_settings_lock<T>(
    path: &Path,
    update: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    use std::os::fd::AsRawFd;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let lock_path = path.with_extension("json.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&lock_path)
        .map_err(|e| format!("cannot lock {}: {e}", lock_path.display()))?;
    // O_NOFOLLOW refuses a symlink; anything else that is not the user's own
    // regular file (a FIFO, device or someone else's file) is refused too.
    {
        use std::os::unix::fs::MetadataExt;
        let meta = lock
            .metadata()
            .map_err(|e| format!("cannot lock {}: {e}", lock_path.display()))?;
        // SAFETY: geteuid has no preconditions.
        let me = unsafe { libc::geteuid() };
        if !meta.file_type().is_file() || meta.uid() != me {
            return Err(format!(
                "cannot lock {}: it is not a regular file owned by you; settings were not changed",
                lock_path.display()
            ));
        }
    }
    // SAFETY: flock on a valid, owned descriptor; released when `lock` closes.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(format!(
            "cannot lock {}: {}",
            lock_path.display(),
            std::io::Error::last_os_error()
        ));
    }
    let result = update();
    drop(lock);
    result
}

/// Crash-safe replace: a uniquely named temp file is written and flushed,
/// renamed over the settings file, and the folder entry is flushed. A crash
/// leaves either the old or the new settings, never a mix.
fn save_settings_to(path: &Path, settings: &StorageSettings) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("cannot save {}: {e}", path.display());
    let parent = path
        .parent()
        .ok_or_else(|| format!("cannot save {}: no folder", path.display()))?;
    std::fs::create_dir_all(parent).map_err(fail)?;
    let bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temp = parent.join(format!(
        ".{SETTINGS_FILE}.{}-{nanos}.tmp",
        std::process::id()
    ));
    let written = (|| -> std::io::Result<()> {
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)?;
        out.write_all(&bytes)?;
        out.sync_all()?;
        std::fs::rename(&temp, path)?;
        File::open(parent)?.sync_all()
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written.map_err(fail)
}

fn resolve(
    env_value: Option<&Path>,
    settings: &StorageSettings,
    default: PathBuf,
    home: Option<&Path>,
    target_root: Option<&Path>,
) -> Result<EffectiveStorage, String> {
    if let Some(chosen) = &settings.root
        && let Some(picked) = chosen.parent()
        && !settings.identities.iter().any(|i| i.path == picked)
    {
        return Err(format!(
            "EmuWiz has no record of which drive {} is on. Choose that folder again in Settings > Amiga hard-disk game copies so EmuWiz can recognise it (nothing is moved or deleted).",
            picked.display()
        ));
    }
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
        .chain(settings.used_roots.iter())
    {
        if candidate != &root && !earlier_roots.contains(candidate) {
            earlier_roots.push(candidate.clone());
        }
    }
    Ok(EffectiveStorage {
        root,
        source,
        earlier_roots,
        used_roots: {
            let mut used = settings.used_roots.clone();
            for earlier in &settings.earlier_roots {
                let known = settings
                    .identities
                    .iter()
                    .any(|i| &i.path == earlier || Some(i.path.as_path()) == earlier.parent());
                if earlier != &default && !known && !used.contains(earlier) {
                    used.push(earlier.clone());
                }
            }
            used
        },
        identities: settings.identities.clone(),
    })
}

fn with_identity(settings: &StorageSettings, identity: FolderIdentity) -> StorageSettings {
    let mut next = settings.clone();
    next.identities.retain(|i| i.path != identity.path);
    next.identities.push(identity);
    next
}

fn add_used_root(settings: &StorageSettings, root: &FolderIdentity) -> Option<StorageSettings> {
    if settings.used_roots.iter().any(|r| r == &root.path) {
        return None;
    }
    let mut next = with_identity(settings, root.clone());
    next.used_roots.push(root.path.clone());
    Some(next)
}

/// Remembers `root` (with its filesystem identity) as holding copies before
/// the first copy is written there. Fails closed: no copy is made if this
/// cannot be saved.
fn remember_used_root(root: &FolderIdentity) -> Result<(), String> {
    let path = settings_path()?;
    with_settings_lock(&path, || {
        let settings = load_settings_from(&path)?;
        match add_used_root(&settings, root) {
            Some(next) => save_settings_to(&path, &next),
            None => Ok(()),
        }
    })
}

/// The user confirms that a remembered folder whose identity changed (for
/// example a drive formatted or restored elsewhere) is the right one. Only
/// the setting changes; nothing in the folder is touched.
pub fn accept_storage_folder(folder: &Path) -> Result<(), String> {
    accept_storage_folder_in(&settings_path()?, folder)
}

fn accept_storage_folder_in(path: &Path, folder: &Path) -> Result<(), String> {
    let path = path.to_path_buf();
    with_settings_lock(&path, || {
        let settings = load_settings_from(&path)?;
        if !settings.identities.iter().any(|i| i.path == folder) {
            return Err(format!("{} is not a remembered folder", folder.display()));
        }
        if !holds_copies(folder) {
            return Err(format!(
                "{} does not contain EmuWiz Amiga copies (no {KEEP_NOTE_NAME}). An unplugged drive leaves an empty folder like this: mount the drive instead of accepting it.",
                folder.display()
            ));
        }
        let now = identify_folder(folder)
            .map_err(|e| format!("{} is not available: {e}", folder.display()))?;
        refuse_ambiguous(&now)?;
        save_settings_to(&path, &with_identity(&settings, now))
    })
}

/// Whether `folder` visibly holds EmuWiz copies: the keep-note written into
/// every storage folder, directly or in the chosen-folder subfolder. An
/// empty mount point never does.
fn holds_copies(folder: &Path) -> bool {
    [
        folder.join(KEEP_NOTE_NAME),
        folder.join(CHOSEN_SUBFOLDER).join(KEEP_NOTE_NAME),
    ]
    .iter()
    .any(|note| std::fs::symlink_metadata(note).is_ok_and(|m| m.file_type().is_file()))
}

/// Whether a remembered folder still matches its recorded identity:
/// `None` when nothing was recorded for it.
pub fn folder_matches_record(folder: &Path, identities: &[FolderIdentity]) -> Option<bool> {
    let recorded = identities.iter().find(|i| i.path == folder)?;
    Some(identify_folder(folder).is_ok_and(|now| same_folder(recorded, &now)))
}

fn without_root(settings: &StorageSettings, folder: &Path) -> StorageSettings {
    let mut next = settings.clone();
    next.earlier_roots.retain(|r| r != folder);
    next.used_roots.retain(|r| r != folder);
    next.identities.retain(|i| i.path != folder);
    next
}

/// Stops searching a remembered folder that is gone for good. The folder and
/// anything still in it are not touched; only the setting changes. The
/// folder in use for new copies cannot be forgotten.
pub fn forget_storage_folder(folder: &Path) -> Result<(), String> {
    let path = settings_path()?;
    with_settings_lock(&path, || {
        let settings = load_settings_from(&path)?;
        if settings.root.as_deref() == Some(folder) {
            return Err("this folder is in use for new copies; choose another folder first".into());
        }
        save_settings_to(&path, &without_root(&settings, folder))
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
    // Never nest storage inside an existing copies folder (or one of its
    // per-game folders): a folder picker's autocompletion makes that easy.
    if let Some(inside) = folder
        .ancestors()
        .find(|a| std::fs::symlink_metadata(a.join(KEEP_NOTE_NAME)).is_ok())
    {
        return Err(format!(
            "{} is inside an existing EmuWiz copies folder ({}); choose the folder that contains \"{CHOSEN_SUBFOLDER}\" instead",
            folder.display(),
            inside.display()
        ));
    }
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
    with_settings_lock(&path, || {
        let settings = load_settings_from(&path)?;
        let (_, home, target_root) = env_paths();
        let current_root = match &settings.root {
            Some(root) => root.clone(),
            None => default_root()?,
        };
        let mut next = settings.clone();
        let chosen = match folder {
            Some(folder) => {
                validate_chosen(folder, home.as_deref(), target_root.as_deref())?;
                // Remember which filesystem object was picked, so Play can tell
                // it from an empty mount point when the drive is absent.
                let picked = identify_folder(folder)
                    .map_err(|e| format!("cannot use {}: {e}", folder.display()))?;
                refuse_ambiguous(&picked)?;
                next = with_identity(&next, picked);
                Some(folder.join(CHOSEN_SUBFOLDER))
            }
            None => None,
        };
        save_settings_to(&path, &record_choice(&next, chosen, &current_root))?;
        Ok(())
    })?;
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
    let refuse = |inside: &Path| {
        Err(format!(
            "{} is inside {}, which cleanup tools may delete; choose a folder for saved games instead",
            path.display(),
            inside.display()
        ))
    };
    for root in disposable {
        let root_resolved = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        if path.starts_with(&root) || resolved.starts_with(&root_resolved) {
            return refuse(&root);
        }
    }
    // Recognised without any environment variable (a desktop-launched Blue
    // does not see the Cargo settings): a folder named like the EmuWiz Cargo
    // target root, or any folder carrying the standard cache marker that
    // Cargo writes into every build folder.
    for candidate in [path, resolved.as_path()] {
        for ancestor in candidate.ancestors() {
            let named_cargo = ancestor
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("emuwiz-cargo-target"));
            if named_cargo || ancestor.join("CACHEDIR.TAG").exists() {
                return refuse(ancestor);
            }
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
            used_roots: vec![PathBuf::from("/mnt/used")],
            identities: vec![FolderIdentity {
                path: PathBuf::from("/mnt/bulk"),
                dev: 1,
                ino: 2,
                fsid: 3,
            }],
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
            vec![
                default.clone(),
                PathBuf::from("/mnt/old"),
                PathBuf::from("/mnt/used")
            ]
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

        let found = working_copy_in(
            &path,
            new.path(),
            &[old.path().to_path_buf()],
            &[],
            &[],
            &mut |_| Ok(()),
        )
        .unwrap();
        assert_eq!(found, old_copy);
        assert_eq!(std::fs::read(&found).unwrap(), b"RDSK-image+save");
        assert!(std::fs::read_dir(new.path()).unwrap().next().is_none());

        // A game with no earlier copy goes to the new folder.
        let other = src.path().join("Other.hdf");
        std::fs::write(&other, b"RDSK-other").unwrap();
        let fresh = working_copy_in(
            &other,
            new.path(),
            &[old.path().to_path_buf()],
            &[],
            &[],
            &mut |_| Ok(()),
        )
        .unwrap();
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
    fn settings_saves_are_atomic_and_concurrent_updates_are_never_lost() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(SETTINGS_FILE);
        // Many launches recording used folders at once, as Settings and a
        // launch can: every record must survive.
        let threads: Vec<_> = (0..16)
            .map(|n| {
                let file = file.clone();
                std::thread::spawn(move || {
                    let root = FolderIdentity {
                        path: PathBuf::from(format!("/mnt/drive{n}/C")),
                        dev: n,
                        ino: n,
                        fsid: n,
                    };
                    with_settings_lock(&file, || {
                        let settings = load_settings_from(&file)?;
                        let next = add_used_root(&settings, &root).unwrap();
                        std::thread::sleep(std::time::Duration::from_millis(2));
                        save_settings_to(&file, &next)
                    })
                    .unwrap();
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let saved = load_settings_from(&file).unwrap();
        assert_eq!(saved.used_roots.len(), 16);
        assert_eq!(saved.identities.len(), 16);
        // Only the settings file and its lock remain: no temp files.
        let mut names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "amiga-hdf-storage.json".to_string(),
                "amiga-hdf-storage.json.lock".to_string()
            ]
        );
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

    #[test]
    fn an_unavailable_folder_with_copies_blocks_play_instead_of_splitting_saves() {
        let src = tempfile::tempdir().unwrap();
        let drive = tempfile::tempdir().unwrap();
        let new = tempfile::tempdir().unwrap();
        let path = original(src.path(), b"RDSK-x");
        // Folder A held copies, then its drive went away.
        let gone = drive.path().join("unplugged/EmuWiz Amiga HDF copies");
        let result = working_copy_in(
            &path,
            new.path(),
            &[gone.clone()],
            &[gone.clone()],
            &[],
            &mut |_| Ok(()),
        );
        let error = result.unwrap_err();
        assert!(error.contains("not available"), "{error}");
        assert!(
            std::fs::read_dir(new.path()).unwrap().next().is_none(),
            "no second copy"
        );
        // Forgetting it (explicit user choice) lets play continue.
        let settings = StorageSettings {
            earlier_roots: vec![gone.clone()],
            used_roots: vec![gone.clone()],
            ..Default::default()
        };
        let forgotten = without_root(&settings, &gone);
        assert!(forgotten.earlier_roots.is_empty() && forgotten.used_roots.is_empty());
        // The same folder never used for copies is simply skipped.
        assert!(working_copy_in(&path, new.path(), &[gone], &[], &[], &mut |_| Ok(())).is_ok());
    }

    #[test]
    fn copies_in_two_places_are_refused() {
        let src = tempfile::tempdir().unwrap();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let path = original(src.path(), b"RDSK-two");
        working_copy_for(&path, a.path()).unwrap();
        working_copy_for(&path, b.path()).unwrap();
        let error = working_copy_in(
            &path,
            b.path(),
            &[a.path().to_path_buf()],
            &[],
            &[],
            &mut |_| Ok(()),
        )
        .unwrap_err();
        assert!(error.contains("more than one place"), "{error}");
    }

    #[test]
    fn a_folder_is_remembered_before_its_first_copy_and_failure_writes_nothing() {
        let src = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("copies");
        let path = original(src.path(), b"RDSK-r");
        let refused = working_copy_in(&path, &root, &[], &[], &[], &mut |_| {
            Err("cannot save".into())
        });
        assert!(refused.is_err());
        assert!(
            !root.join(KEEP_NOTE_NAME).exists() || std::fs::read_dir(&root).unwrap().count() == 1
        );
        let mut seen = Vec::new();
        let copy = working_copy_in(&path, &root, &[], &[], &[], &mut |r| {
            seen.push(r.path.clone());
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, vec![root.clone()]);
        // Already remembered: not recorded again; the copy is reused.
        let mut again = 0;
        let reused = working_copy_in(&path, &root, &[], &[root.clone()], &[], &mut |_| {
            again += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!((reused, again), (copy, 0));
        // An override folder recorded this way is still searched after the
        // variable is unset.
        let x = FolderIdentity {
            path: PathBuf::from("/x"),
            dev: 1,
            ino: 2,
            fsid: 3,
        };
        let settings = add_used_root(&StorageSettings::default(), &x).unwrap();
        assert!(add_used_root(&settings, &x).is_none());
        assert_eq!(settings.identities, vec![x]);
        let after = resolve(None, &settings, PathBuf::from("/d"), None, None).unwrap();
        assert!(after.earlier_roots.contains(&PathBuf::from("/x")));
        assert_eq!(after.used_roots, vec![PathBuf::from("/x")]);
    }

    #[test]
    fn a_missing_picked_folder_is_never_recreated() {
        let src = tempfile::tempdir().unwrap();
        let drive = tempfile::tempdir().unwrap();
        let path = original(src.path(), b"RDSK-m");
        let root = drive.path().join("picked-but-gone").join(CHOSEN_SUBFOLDER);
        assert!(working_copy_for(&path, &root).is_err());
        assert!(!drive.path().join("picked-but-gone").exists());
    }

    #[test]
    fn cargo_and_cache_folders_are_recognised_without_environment_variables() {
        let base = tempfile::tempdir().unwrap();
        let cargo_root = base.path().join("emuwiz-cargo-targets");
        std::fs::create_dir_all(cargo_root.join("x")).unwrap();
        assert!(check_override(&cargo_root.join("x"), None, None).is_err());
        let build = base.path().join("some-target");
        std::fs::create_dir_all(build.join("debug")).unwrap();
        std::fs::write(
            build.join("CACHEDIR.TAG"),
            b"Signature: 8a477f597d28d172789f06886806bc55",
        )
        .unwrap();
        assert!(check_override(&build.join("debug"), None, None).is_err());
        assert!(check_override(&base.path().join("saves"), None, None).is_ok());
    }

    fn picked(dir: &Path) -> (PathBuf, FolderIdentity) {
        let folder = dir.join("drive-folder");
        std::fs::create_dir(&folder).unwrap();
        let identity = identify_folder(&folder).unwrap();
        (folder, identity)
    }

    fn launch(path: &Path, root: &Path, ids: &[FolderIdentity]) -> Result<PathBuf, String> {
        working_copy_in(path, root, &[], &[], ids, &mut |_| Ok(()))
    }

    #[test]
    fn mount_identity_normal_mounted_folder_gets_the_copy() {
        let src = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        let (folder, id) = picked(base.path());
        let path = original(src.path(), b"RDSK-ok");
        let root = folder.join(CHOSEN_SUBFOLDER);
        let copy = launch(&path, &root, std::slice::from_ref(&id)).unwrap();
        assert!(copy.starts_with(&root));
        // And again, reused, with the same identity.
        assert_eq!(launch(&path, &root, &[id]).unwrap(), copy);
    }

    #[test]
    fn mount_identity_unmounted_drive_leaves_an_empty_mount_point_and_play_is_refused() {
        let src = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        let (folder, id) = picked(base.path());
        let path = original(src.path(), b"RDSK-um");
        // "Unmount": the real folder goes away and an empty directory (the
        // bare mount point on the system drive) is left at the same path.
        std::fs::rename(&folder, base.path().join("elsewhere")).unwrap();
        std::fs::create_dir(&folder).unwrap();
        let error = launch(&path, &folder.join(CHOSEN_SUBFOLDER), &[id]).unwrap_err();
        assert!(error.contains("different drive or was replaced"), "{error}");
        assert!(
            std::fs::read_dir(&folder).unwrap().next().is_none(),
            "nothing created"
        );
    }

    #[test]
    fn mount_identity_missing_or_symlinked_folder_is_refused_without_creating_it() {
        let src = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        let (folder, id) = picked(base.path());
        let path = original(src.path(), b"RDSK-rp");
        std::fs::remove_dir(&folder).unwrap();
        let root = folder.join(CHOSEN_SUBFOLDER);
        assert!(launch(&path, &root, std::slice::from_ref(&id)).is_err());
        assert!(!folder.exists());
        // Replaced by a symlink to another folder: refused, nothing written there.
        let other = base.path().join("other");
        std::fs::create_dir(&other).unwrap();
        std::os::unix::fs::symlink(&other, &folder).unwrap();
        assert!(launch(&path, &root, &[id]).is_err());
        assert!(std::fs::read_dir(&other).unwrap().next().is_none());
    }

    #[test]
    fn mount_identity_changed_filesystem_is_refused_and_a_used_folder_is_checked_too() {
        let src = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        let (folder, id) = picked(base.path());
        let path = original(src.path(), b"RDSK-fs");
        let root = folder.join(CHOSEN_SUBFOLDER);
        let moved = FolderIdentity {
            dev: id.dev.wrapping_add(1),
            fsid: id.fsid.wrapping_add(1),
            ..id.clone()
        };
        assert!(launch(&path, &root, &[moved]).is_err());
        assert!(!root.exists());
        // The storage folder itself, once used, is checked as well.
        let copy = launch(&path, &root, std::slice::from_ref(&id)).unwrap();
        let used = identify_folder(&root).unwrap();
        let replaced = FolderIdentity {
            ino: used.ino.wrapping_add(1),
            ..used
        };
        let error = launch(&path, &root, &[id, replaced]).unwrap_err();
        assert!(error.contains("replaced"), "{error}");
        assert!(
            copy.exists(),
            "the existing copy and its saves are untouched"
        );
    }

    #[test]
    fn mount_identity_replugged_drive_with_a_new_device_number_still_plays() {
        let src = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        let (folder, id) = picked(base.path());
        let path = original(src.path(), b"RDSK-rp2");
        let root = folder.join(CHOSEN_SUBFOLDER);
        // Same inode and filesystem id, different device number.
        let replugged = FolderIdentity {
            dev: id.dev.wrapping_add(7),
            ..id.clone()
        };
        if id.fsid != 0 {
            assert!(launch(&path, &root, &[replugged]).is_ok());
        }
    }

    #[test]
    fn settings_without_a_drive_record_fail_closed_until_the_folder_is_chosen_again() {
        let legacy = StorageSettings {
            root: Some(PathBuf::from("/mnt/usb/EmuWiz Amiga HDF copies")),
            earlier_roots: vec![PathBuf::from("/mnt/old/EmuWiz Amiga HDF copies")],
            ..Default::default()
        };
        let error = resolve(None, &legacy, PathBuf::from("/d"), None, None).unwrap_err();
        assert!(error.contains("Choose that folder again"), "{error}");
        // An earlier folder with no drive record is treated as required.
        let chosen_again = with_identity(
            &legacy,
            FolderIdentity {
                path: PathBuf::from("/mnt/usb"),
                dev: 1,
                ino: 2,
                fsid: 3,
            },
        );
        let storage = resolve(None, &chosen_again, PathBuf::from("/d"), None, None).unwrap();
        assert!(
            storage
                .used_roots
                .contains(&PathBuf::from("/mnt/old/EmuWiz Amiga HDF copies"))
        );
    }

    #[test]
    fn accept_is_only_possible_for_a_folder_that_holds_copies() {
        let base = tempfile::tempdir().unwrap();
        let empty_mount_point = base.path().join("usb");
        std::fs::create_dir(&empty_mount_point).unwrap();
        assert!(!holds_copies(&empty_mount_point));
        let src = tempfile::tempdir().unwrap();
        let path = original(src.path(), b"RDSK-acc");
        working_copy_for(&path, &empty_mount_point.join(CHOSEN_SUBFOLDER)).unwrap();
        assert!(holds_copies(&empty_mount_point));
        assert!(holds_copies(&empty_mount_point.join(CHOSEN_SUBFOLDER)));
    }

    fn dev_ino(path: &Path) -> (u64, u64) {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::symlink_metadata(path).unwrap();
        (m.dev(), m.ino())
    }

    #[test]
    fn failed_copy_cleanup_never_removes_an_existing_or_substituted_save_file() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("copies");
        std::fs::create_dir(&root).unwrap();
        let recorded = identify_folder(&root).unwrap();
        let game = root.join("game.hdf");
        // A folder that was swapped after the check (its recorded identity no
        // longer matches).
        let swapped = FolderIdentity {
            ino: recorded.ino + 1,
            ..recorded.clone()
        };

        // 1. A save-bearing copy substituted at the target (by another
        //    launch, or a different drive) is kept, though this call
        //    "created" a file whose identity it remembers.
        std::fs::write(&game, b"SAVES-substituted").unwrap();
        let ours = (recorded.dev, u64::MAX);
        assert!(check_landed_copy(&root, &swapped, &game, true, ours).is_err());
        assert_eq!(std::fs::read(&game).unwrap(), b"SAVES-substituted");

        // 2. An existing copy (link refused, so not created by this call) is
        //    kept even when its device and inode are the ones given.
        let existing = dev_ino(&game);
        assert!(check_landed_copy(&root, &swapped, &game, false, existing).is_err());
        assert_eq!(std::fs::read(&game).unwrap(), b"SAVES-substituted");

        // 3. Same inode number on another device (another filesystem) is not
        //    ours: kept.
        let other_device = (existing.0 ^ 1, existing.1);
        assert!(check_landed_copy(&root, &swapped, &game, true, other_device).is_err());
        assert!(game.exists());

        // 4. The folder did not change: nothing is removed, the copy stands.
        assert!(check_landed_copy(&root, &recorded, &game, true, existing).is_ok());
        assert!(game.exists());

        // 5. Control: the exact file this call wrote, in a swapped folder, is
        //    removed (it holds no saves yet) and the launch is refused.
        assert!(check_landed_copy(&root, &swapped, &game, true, existing).is_err());
        assert!(!game.exists());

        // An existing save-bearing copy is reused, never overwritten, and a
        // failed attempt to make another leaves it alone.
        let src = tempfile::tempdir().unwrap();
        let path = original(src.path(), b"RDSK-orig");
        let copy = working_copy_for(&path, &root).unwrap();
        std::fs::write(&copy, b"RDSK-with-saves").unwrap();
        assert_eq!(working_copy_for(&path, &root).unwrap(), copy);
        assert_eq!(std::fs::read(&copy).unwrap(), b"RDSK-with-saves");
    }

    #[test]
    fn accept_refuses_an_empty_or_incorrect_folder_and_changes_nothing() {
        let base = tempfile::tempdir().unwrap();
        let settings_file = base.path().join("cfg").join(SETTINGS_FILE);
        let empty = base.path().join("usb");
        std::fs::create_dir(&empty).unwrap();
        let unrelated = base.path().join("photos");
        std::fs::create_dir(&unrelated).unwrap();
        std::fs::write(unrelated.join("holiday.jpg"), b"jpg").unwrap();
        // A keep-note that is a symlink or a folder does not count.
        let fake_note = base.path().join("fake");
        std::fs::create_dir(&fake_note).unwrap();
        std::os::unix::fs::symlink(base.path().join("x"), fake_note.join(KEEP_NOTE_NAME)).unwrap();
        let real = base.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let src = tempfile::tempdir().unwrap();
        working_copy_for(&original(src.path(), b"RDSK"), &real.join(CHOSEN_SUBFOLDER)).unwrap();
        let link_to_real = base.path().join("link");
        std::os::unix::fs::symlink(&real, &link_to_real).unwrap();
        let not_remembered = base.path().join("elsewhere");
        std::fs::create_dir(&not_remembered).unwrap();
        working_copy_for(&original(src.path(), b"RDSK2"), &not_remembered).unwrap();

        let stale = |path: &Path| FolderIdentity {
            path: path.to_path_buf(),
            dev: 1,
            ino: 2,
            fsid: 3,
        };
        let settings = StorageSettings {
            identities: [&empty, &unrelated, &fake_note, &link_to_real, &real]
                .iter()
                .map(|p| stale(p))
                .collect(),
            ..StorageSettings::default()
        };
        save_settings_to(&settings_file, &settings).unwrap();
        let before = std::fs::read(&settings_file).unwrap();

        for refused in [
            &empty,
            &unrelated,
            &fake_note,
            &link_to_real,
            &not_remembered,
        ] {
            assert!(
                accept_storage_folder_in(&settings_file, refused).is_err(),
                "{} must be refused",
                refused.display()
            );
            assert_eq!(std::fs::read(&settings_file).unwrap(), before);
        }
        assert!(std::fs::read_dir(&empty).unwrap().next().is_none());

        // The right folder, holding copies, is accepted with its live identity
        // (unless this test's own drive is one EmuWiz cannot tell apart, e.g.
        // a tmpfs /tmp: then it is refused too and nothing changes).
        if ambiguous_identity(&identify_folder(&real).unwrap()) {
            assert!(accept_storage_folder_in(&settings_file, &real).is_err());
            assert_eq!(std::fs::read(&settings_file).unwrap(), before);
            return;
        }
        accept_storage_folder_in(&settings_file, &real).unwrap();
        let after = load_settings_from(&settings_file).unwrap();
        let now = after.identities.iter().find(|i| i.path == real).unwrap();
        assert_eq!(now, &identify_folder(&real).unwrap());
    }

    #[test]
    fn the_settings_lock_refuses_symlinks_and_non_regular_files() {
        let base = tempfile::tempdir().unwrap();
        let settings_file = base.path().join(SETTINGS_FILE);
        let lock_path = settings_file.with_extension("json.lock");
        let victim = base.path().join("victim.txt");
        std::fs::write(&victim, b"keep me").unwrap();
        let ran = std::cell::Cell::new(false);
        let attempt = || {
            with_settings_lock(&settings_file, || {
                ran.set(true);
                Ok(())
            })
        };

        std::os::unix::fs::symlink(&victim, &lock_path).unwrap();
        assert!(attempt().is_err());
        std::fs::remove_file(&lock_path).unwrap();
        // A dangling symlink is not followed to create a file either.
        let created_through = base.path().join("created-through-link");
        std::os::unix::fs::symlink(&created_through, &lock_path).unwrap();
        assert!(attempt().is_err());
        assert!(!created_through.exists());
        std::fs::remove_file(&lock_path).unwrap();

        let fifo = std::ffi::CString::new(lock_path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(attempt().is_err());
        std::fs::remove_file(&lock_path).unwrap();

        std::fs::create_dir(&lock_path).unwrap();
        assert!(attempt().is_err());
        std::fs::remove_dir(&lock_path).unwrap();

        assert!(!ran.get(), "nothing runs without the lock");
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep me");
        assert!(!settings_file.exists());

        // Normal case: a regular lock file is created and used.
        assert!(attempt().is_ok());
        assert!(ran.get());
        assert!(
            std::fs::symlink_metadata(&lock_path)
                .unwrap()
                .file_type()
                .is_file()
        );
    }

    #[test]
    fn play_refuses_storage_on_a_drive_that_cannot_be_told_apart() {
        let stick = FolderIdentity {
            path: PathBuf::from("/media/stick"),
            dev: 0x0821,
            ino: 1,
            fsid: 0x0821,
        };
        assert!(refuse_ambiguous(&stick).is_err());

        let base = tempfile::tempdir().unwrap();
        let default = base.path().join("data").join("amiga-hdf-working");
        let picked = base.path().join("picked");
        std::fs::create_dir(&picked).unwrap();
        let storage = |root: PathBuf, earlier: Vec<PathBuf>| EffectiveStorage {
            root,
            source: StorageSource::Chosen,
            earlier_roots: earlier,
            used_roots: Vec::new(),
            identities: Vec::new(),
        };
        // The copies folder does not exist yet: the picked folder is judged.
        let pending = storage(picked.join(CHOSEN_SUBFOLDER), vec![default.clone()]);
        let here_ambiguous = ambiguous_identity(&identify_folder(&picked).unwrap());
        assert_eq!(
            refuse_ambiguous_storage(&pending, &default).is_err(),
            here_ambiguous
        );
        // EmuWiz's own data folder is never judged, and missing folders are
        // left to the availability checks.
        std::fs::create_dir_all(&default).unwrap();
        let own = storage(default.clone(), vec![base.path().join("gone")]);
        assert!(refuse_ambiguous_storage(&own, &default).is_ok());
    }

    #[test]
    fn a_drive_whose_identity_is_only_its_device_number_is_ambiguous() {
        let id = |dev: u64, fsid: u64| FolderIdentity {
            path: PathBuf::from("/media/stick"),
            dev,
            ino: 1,
            fsid,
        };
        // FAT/exFAT report the kernel's encoding of the device number.
        assert!(ambiguous_identity(&id(0x0821, 0x0821))); // sdc1 (8:33)
        let big = (259u64 << 8) | 5; // nvme partition 259:5
        assert!(ambiguous_identity(&id(big, big)));
        let wide_minor = (8u64 << 8) | 0x10 | ((0x300u64 & !0xff) << 12); // 8:0x310
        assert!(ambiguous_identity(&id(
            wide_minor,
            0x10 | (8 << 8) | ((0x310u64 & !0xff) << 12)
        )));
        assert!(ambiguous_identity(&id(0x0821, 0)));
        // ext4/btrfs ids come from the filesystem UUID: distinct.
        assert!(!ambiguous_identity(&id(0x0821, 0xd76c_9b1f_2a40_33e1)));
    }

    #[test]
    fn a_folder_inside_an_existing_copies_folder_cannot_be_chosen() {
        let base = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let path = original(src.path(), b"RDSK-nest");
        let root = base.path().join(CHOSEN_SUBFOLDER);
        let copy = working_copy_for(&path, &root).unwrap();
        let home = Some(Path::new("/nonexistent-home"));
        assert!(validate_chosen(&root, home, None).is_err());
        assert!(validate_chosen(copy.parent().unwrap(), home, None).is_err());
        assert!(validate_chosen(base.path(), home, None).is_ok());
    }
}
