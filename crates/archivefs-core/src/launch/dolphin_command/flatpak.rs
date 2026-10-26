//! Validate the resources behind Dolphin's transient read-only Flatpak grants.
//! No persistent overrides are read or written: explicit per-file options take
//! precedence over installation permissions, including read-only installations.

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::launch::installation::VisibilityPlan;
use crate::launch::process_spawn::CapturedFileIdentity;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Resources {
    pub visibility: VisibilityPlan,
    files: Vec<(PathBuf, CapturedFileIdentity)>,
}

pub(crate) fn inspect(content: &Path) -> Result<Resources, String> {
    let paths = [content.to_path_buf()];
    // Dolphin opens up to nine WBFS continuation files and includes their
    // sizes in its header check. Core currently verifies standalone WBFS
    // only: even a stray companion would change what Dolphin actually reads.
    if let Some(extension) = content.extension().and_then(|ext| ext.to_str())
        && extension.eq_ignore_ascii_case("wbfs")
    {
        for index in 1..=9 {
            let path = content.with_extension(format!("{}{index}", &extension[..3]));
            match fs::symlink_metadata(&path) {
                Ok(_) => {
                    return Err(format!(
                        "WBFS continuation files are not supported by this launch path: {}",
                        path.display()
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!(
                        "cannot inspect WBFS companion {}: {error}",
                        path.display()
                    ));
                }
            }
        }
    }
    let mut resources = Resources {
        visibility: VisibilityPlan::new(),
        files: Vec::new(),
    };
    for path in paths {
        check_path(&path)?;
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(format!(
                "{} is not a direct regular game file",
                path.display()
            ));
        }
        // Canonical equality also rejects symlinks in ancestor directories:
        // a grant for the textual path alone is not proof of target visibility.
        if fs::canonicalize(&path).ok().as_deref() != Some(path.as_path()) {
            return Err(format!("{} is not a canonical game path", path.display()));
        }
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = options
            .open(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let identity = CapturedFileIdentity::capture(&metadata);
        if file
            .metadata()
            .map(|m| CapturedFileIdentity::capture(&m))
            .ok()
            != Some(identity)
        {
            return Err(format!("{} changed while checking access", path.display()));
        }
        resources.files.push((path.clone(), identity));
        resources
            .visibility
            .read_only(&path, "Dolphin game file or WBFS companion");
    }
    resources
        .visibility
        .flatpak_options()
        .map_err(|error| error.to_string())?;
    Ok(resources)
}

fn check_path(path: &Path) -> Result<(), String> {
    // Flatpak ignores filesystem options for these runtime-owned locations.
    // See https://docs.flatpak.org/en/latest/sandbox-permissions.html#reserved-paths
    const RESERVED: &[&str] = &[
        "/app",
        "/bin",
        "/dev",
        "/etc",
        "/lib",
        "/lib32",
        "/lib64",
        "/proc",
        "/run/flatpak",
        "/run/host",
        "/sbin",
        "/usr",
    ];
    if !cfg!(target_os = "linux")
        || !path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
        || RESERVED.iter().any(|root| path.starts_with(root))
    {
        return Err(format!(
            "Flatpak cannot expose this game path: {}",
            path.display()
        ));
    }
    Ok(())
}
