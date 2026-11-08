//! Isolated, disposable Hatari session.
//!
//! Hatari merges `--configfile` into whatever it already loaded from the
//! user's own `~/.config/hatari`, and it will insert drive B, attach hard
//! disks and write back to the selected image according to that inherited
//! configuration. This module removes every one of those paths:
//!
//! * a private session directory holds a fresh `HOME` and `XDG_*` tree, so no
//!   user configuration, storage path or save is ever read;
//! * the selected disk is copied (bounded, no symlinks, hash-verified) into
//!   the session as a read-only file, so the original media is never opened
//!   for writing;
//! * the command line enables only drive A, switches drive B off, turns floppy
//!   write protection on, and passes machine and TOS explicitly.
//!
//! Anything the game writes lands in the disposable copy and is discarded
//! with the session (stale sessions are pruned on the next launch).

use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::launch::hatari_command::HatariCommand;
use crate::patch_manager::HatariMachineModel;

/// Floppy images are small; anything larger is refused.
pub const MAX_DISK_BYTES: u64 = 32 * 1024 * 1024;

/// Sessions older than this are removed at the next launch.
pub const STALE_SESSION_AGE: Duration = Duration::from_secs(24 * 60 * 60);

const SESSION_PREFIX: &str = "session-";

/// A prepared, private Hatari invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HatariSession {
    pub root: PathBuf,
    pub disk_copy: PathBuf,
    pub working_directory: PathBuf,
    pub environment: Vec<(OsString, OsString)>,
    pub arguments: Vec<OsString>,
}

/// The `--machine` argument value for a model.
pub fn machine_argument(model: HatariMachineModel) -> Option<&'static str> {
    match model {
        HatariMachineModel::St => Some("st"),
        HatariMachineModel::Ste => Some("ste"),
        HatariMachineModel::Tt => Some("tt"),
        HatariMachineModel::Falcon => Some("falcon"),
        HatariMachineModel::Unknown => None,
    }
}

/// The complete, inherited-config-free argument list. `disk` is the image
/// Hatari will open (the session copy at spawn time, the selected media in a
/// read-only preview).
pub fn isolated_arguments(
    machine: HatariMachineModel,
    tos_path: &Path,
    disk: &Path,
) -> Result<Vec<OsString>, String> {
    let machine = machine_argument(machine).ok_or("Hatari machine model is unknown")?;
    Ok(vec![
        "--machine".into(),
        machine.into(),
        "--tos".into(),
        tos_path.as_os_str().to_os_string(),
        "--drive-a".into(),
        "on".into(),
        "--drive-b".into(),
        "off".into(),
        "--protect-floppy".into(),
        "on".into(),
        "--confirm-quit".into(),
        "false".into(),
        "--disk-a".into(),
        disk.as_os_str().to_os_string(),
    ])
}

fn read_source(path: &Path) -> Result<Vec<u8>, String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| format!("cannot open the selected disk: {e}"))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.file_type().is_file() {
        return Err("the selected disk is not a regular file".into());
    }
    if metadata.len() == 0 || metadata.len() > MAX_DISK_BYTES {
        return Err("the selected disk image is empty or larger than 32 MiB".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_DISK_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 != metadata.len() {
        return Err("the selected disk changed while it was being copied".into());
    }
    Ok(bytes)
}

fn private_dir(path: &Path) -> Result<(), String> {
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(path)
        .map_err(|e| format!("cannot create {}: {e}", path.display()))
}

fn copy_file_name(source: &Path) -> String {
    let name: String = source
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("disk.st")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("disk-{name}")
}

/// Builds the private session for `command` under `sessions_root`. Never
/// writes to, renames or locks the selected media.
pub fn prepare_session(
    command: &HatariCommand,
    sessions_root: &Path,
) -> Result<HatariSession, String> {
    prune_stale_sessions(sessions_root, STALE_SESSION_AGE);
    let source = &command.selection.content_path;
    let bytes = read_source(source)?;
    let digest = Sha256::digest(&bytes);

    private_dir(sessions_root)?;
    let unique = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let root = sessions_root.join(format!("{SESSION_PREFIX}{}-{unique}", std::process::id()));
    private_dir(&root)?;
    let home = root.join("home");
    let config = root.join("xdg-config");
    let data = root.join("xdg-data");
    let cache = root.join("xdg-cache");
    let state = root.join("xdg-state");
    let media = root.join("media");
    for dir in [&home, &config, &data, &cache, &state, &media] {
        private_dir(dir)?;
    }

    let disk_copy = media.join(copy_file_name(source));
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o400)
        .open(&disk_copy)
        .map_err(|e| format!("cannot create the disk copy: {e}"))?;
    out.write_all(&bytes).map_err(|e| e.to_string())?;
    out.sync_all().map_err(|e| e.to_string())?;
    drop(out);
    let written = std::fs::read(&disk_copy).map_err(|e| e.to_string())?;
    if Sha256::digest(&written) != digest {
        let _ = std::fs::remove_dir_all(&root);
        return Err("the disk copy did not match the original; nothing was launched".into());
    }

    let mut environment: Vec<(OsString, OsString)> = vec![
        ("HOME".into(), home.clone().into_os_string()),
        ("XDG_CONFIG_HOME".into(), config.into_os_string()),
        ("XDG_DATA_HOME".into(), data.into_os_string()),
        ("XDG_CACHE_HOME".into(), cache.into_os_string()),
        ("XDG_STATE_HOME".into(), state.into_os_string()),
    ];
    // A changed HOME must not break X authorisation.
    if std::env::var_os("XAUTHORITY").is_none()
        && let Some(original_home) = std::env::var_os("HOME")
    {
        let cookie = PathBuf::from(original_home).join(".Xauthority");
        if cookie.is_file() {
            environment.push(("XAUTHORITY".into(), cookie.into_os_string()));
        }
    }

    let arguments = isolated_arguments(
        command.selection.machine_model,
        &command.selection.tos_path,
        &disk_copy,
    )?;
    Ok(HatariSession {
        root,
        disk_copy,
        working_directory: home,
        environment,
        arguments,
    })
}

/// Removes session directories (only those this module created, never
/// symlinks) whose modification time is older than `max_age`.
pub fn prune_stale_sessions(sessions_root: &Path, max_age: Duration) {
    let Ok(entries) = std::fs::read_dir(sessions_root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(SESSION_PREFIX) {
            continue;
        }
        let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !metadata.file_type().is_dir() {
            continue;
        }
        let old = metadata
            .modified()
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age > max_age);
        if old {
            // The disk copy is read-only; make the tree writable to remove it.
            let _ = make_tree_removable(&entry.path());
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn make_tree_removable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        for entry in std::fs::read_dir(path)?.flatten() {
            let _ = make_tree_removable(&entry.path());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
