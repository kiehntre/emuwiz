use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use super::{MAX_SETUP_BYTES, SetupManifest};

/// Bounded regular-file reads. No directory walking and no leaf symlinks.
pub(super) fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Could not inspect the settings file.".into()),
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            return Err("Settings must be a regular file, not a link or directory.".into());
        }
        _ => {}
    }
    let file = options
        .open(path)
        .map_err(|_| "Could not read the settings file.")?;
    if !file
        .metadata()
        .map_err(|_| "Could not inspect the opened settings file.")?
        .is_file()
    {
        return Err("Settings must be a regular file.".into());
    }
    bounded_read(file).map(Some)
}

fn bounded_read(file: File) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    file.take(MAX_SETUP_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read the settings file.")?;
    if bytes.len() as u64 > MAX_SETUP_BYTES {
        return Err("The settings file exceeds the 1 MiB limit.".into());
    }
    Ok(bytes)
}

pub fn read_setup_manifest(path: &Path) -> Result<SetupManifest, String> {
    let bytes = read_optional(path)?.ok_or("The setup file was not found.")?;
    // Never echo malformed input; it may contain a pasted credential.
    let manifest: SetupManifest = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "The setup file is invalid or contains unsupported fields (line {}, column {}).",
            error.line(),
            error.column()
        )
    })?;
    manifest.validate()?;
    Ok(manifest)
}

/// Export only to a new file. Existing files and links are never replaced.
/// Settings/ROMs are never opened for writing. A failed write removes only the
/// file this invocation created; a partial file after interruption is rejected
/// by the import parser.
pub fn export_setup_new(path: &Path, manifest: &SetupManifest) -> Result<(), String> {
    manifest.validate()?;
    let bytes =
        serde_json::to_vec_pretty(manifest).map_err(|_| "Could not encode the setup file.")?;
    if bytes.len() as u64 > MAX_SETUP_BYTES {
        return Err("The setup file exceeds the 1 MiB limit.".into());
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            "A file already exists there. Choose a new filename; nothing was overwritten."
                .to_string()
        } else {
            "Could not create the setup file.".into()
        }
    })?;
    if file
        .write_all(&bytes)
        .and_then(|_| file.sync_all())
        .is_err()
    {
        drop(file);
        let _ = fs::remove_file(path);
        return Err("Could not finish writing the setup file.".into());
    }
    Ok(())
}
