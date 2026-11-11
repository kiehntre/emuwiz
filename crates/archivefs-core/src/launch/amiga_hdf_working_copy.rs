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

    ensure_private_dir(root)?;
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

    let mut temp_name = OsString::from(".partial-");
    temp_name.push(std::process::id().to_string());
    temp_name.push("-");
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
    let root = crate::app_dirs::data_dir()
        .map_err(|e| format!("EmuWiz data folder unavailable: {e}"))?
        .join(WORKING_COPY_DIR);
    let copy = working_copy_for(content, &root)?;
    redirect_arguments(arguments, content, &copy)?;
    Ok(Some(copy))
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
}
