//! Bounded, emulator-specific discovery of AppImage and Flatpak installs.
//!
//! Only explicit definitions are searched: a few fixed directories under the
//! home folder for AppImages whose file name starts with a known emulator
//! name, and exact Flatpak application ids looked up in Flatpak's metadata
//! directories. Nothing is executed, nothing is searched recursively, and no
//! arbitrary `*.AppImage` or Flatpak is ever guessed to be an emulator.

use std::fs;
use std::path::{Path, PathBuf};

use super::installation::{LaunchInstallation, valid_flatpak_app_id};

const MAX_DIR_ENTRIES: usize = 512;

/// What EmuWiz knows about how one emulator is packaged.
#[derive(Clone, Copy, Debug)]
pub struct KnownEmulator {
    pub id: &'static str,
    /// Lower-case file name prefixes of its AppImages (`melonds` matches
    /// `melonDS-x86_64.AppImage`).
    pub appimage_stems: &'static [&'static str],
    pub flatpak_ids: &'static [&'static str],
    /// Files beside the executable that put the emulator in portable mode.
    pub portable_markers: &'static [&'static str],
}

pub const PCSX2: KnownEmulator = KnownEmulator {
    id: "PCSX2",
    appimage_stems: &["pcsx2"],
    flatpak_ids: &["net.pcsx2.PCSX2"],
    portable_markers: &["portable.ini", "portable.txt"],
};
pub const DUCKSTATION: KnownEmulator = KnownEmulator {
    id: "DuckStation",
    appimage_stems: &["duckstation"],
    flatpak_ids: &["org.duckstation.DuckStation"],
    portable_markers: &["portable.txt", "settings.ini"],
};
pub const PPSSPP: KnownEmulator = KnownEmulator {
    id: "PPSSPP",
    appimage_stems: &["ppsspp"],
    flatpak_ids: &["org.ppsspp.PPSSPP"],
    portable_markers: &[],
};
pub const MELONDS: KnownEmulator = KnownEmulator {
    id: "melonDS",
    appimage_stems: &["melonds"],
    flatpak_ids: &["net.kuribo64.melonDS"],
    portable_markers: &[],
};

#[must_use]
pub fn known_emulator(id: &str) -> Option<&'static KnownEmulator> {
    [&PCSX2, &DUCKSTATION, &PPSSPP, &MELONDS]
        .into_iter()
        .find(|e| e.id.eq_ignore_ascii_case(id))
}

/// Where to look. Built from the environment in production and from a temp
/// directory in tests.
#[derive(Clone, Debug)]
pub struct KnownInstallRoots {
    pub home: PathBuf,
    /// `$XDG_DATA_HOME` (default `~/.local/share`); user Flatpaks live in
    /// `<this>/flatpak/app/<id>`.
    pub user_data: PathBuf,
    /// `/var/lib` equivalent; system Flatpaks live in `<this>/flatpak/app/<id>`.
    pub system_data: PathBuf,
    pub path_dirs: Vec<PathBuf>,
}

impl KnownInstallRoots {
    #[must_use]
    pub fn from_environment() -> Option<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        let user_data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".local/share"));
        let path_dirs = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).take(64).collect())
            .unwrap_or_default();
        Some(Self {
            home,
            user_data,
            system_data: PathBuf::from("/var/lib"),
            path_dirs,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundAppImage {
    pub path: PathBuf,
    /// A portable-mode marker beside the AppImage, if one exists. Recorded
    /// only; it is never created, moved or deleted.
    pub portable_marker: Option<PathBuf>,
}

fn regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
}

fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    regular_file(path) && fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

fn list(dir: &Path) -> Vec<fs::DirEntry> {
    fs::read_dir(dir)
        .map(|rd| rd.flatten().take(MAX_DIR_ENTRIES).collect())
        .unwrap_or_default()
}

fn matches_appimage(name: &str, def: &KnownEmulator) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".appimage")
        && def
            .appimage_stems
            .iter()
            .any(|stem| lower.starts_with(stem))
}

/// AppImages in `~/Applications`, `~/Applications/emulators` and in an
/// `~/Applications/<name>` or `~/Applications/emulators/<name>` folder whose
/// name starts with a known stem. Non-recursive beyond that; symlinks and
/// non-regular files are ignored.
#[must_use]
pub fn discover_appimages(def: &KnownEmulator, roots: &KnownInstallRoots) -> Vec<FoundAppImage> {
    let applications = roots.home.join("Applications");
    let mut directories = vec![applications.clone(), applications.join("emulators")];
    for parent in [applications.clone(), applications.join("emulators")] {
        for entry in list(&parent) {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            if is_dir && def.appimage_stems.iter().any(|s| name.starts_with(s)) {
                directories.push(entry.path());
            }
        }
    }
    let mut found: Vec<FoundAppImage> = Vec::new();
    for directory in directories {
        for entry in list(&directory) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if matches_appimage(&name, def) && is_executable_file(&path) {
                let portable_marker = def
                    .portable_markers
                    .iter()
                    .map(|marker| directory.join(marker))
                    .find(|marker| regular_file(marker));
                found.push(FoundAppImage {
                    path,
                    portable_marker,
                });
            }
        }
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found.dedup_by(|a, b| a.path == b.path);
    found
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlatpakScope {
    User,
    System,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundFlatpak {
    pub app_id: String,
    pub scope: FlatpakScope,
}

/// Exact, known application ids that have an installed deployment in
/// Flatpak's metadata directories (`.../flatpak/app/<id>/current/active`).
#[must_use]
pub fn discover_flatpaks(def: &KnownEmulator, roots: &KnownInstallRoots) -> Vec<FoundFlatpak> {
    let mut out = Vec::new();
    for app_id in def.flatpak_ids.iter().filter(|id| valid_flatpak_app_id(id)) {
        for (scope, base) in [
            (FlatpakScope::User, &roots.user_data),
            (FlatpakScope::System, &roots.system_data),
        ] {
            let deployed = base
                .join("flatpak/app")
                .join(app_id)
                .join("current/active/metadata");
            if regular_file(&deployed) {
                out.push(FoundFlatpak {
                    app_id: (*app_id).to_string(),
                    scope,
                });
            }
        }
    }
    out
}

/// The `flatpak` program, as an absolute regular executable on `PATH`.
#[must_use]
pub fn flatpak_binary(roots: &KnownInstallRoots) -> Option<PathBuf> {
    roots
        .path_dirs
        .iter()
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("flatpak"))
        .find(|candidate| is_executable_file(candidate))
}

/// A portable-mode marker file beside `executable`, if the emulator has one.
#[must_use]
pub fn portable_marker_beside(def: &KnownEmulator, executable: &Path) -> Option<PathBuf> {
    let directory = executable.parent()?;
    def.portable_markers
        .iter()
        .map(|marker| directory.join(marker))
        .find(|marker| regular_file(marker))
}

/// If `path` is a small script whose only job is `flatpak run <known app id>`
/// (for example `~/.local/bin/PPSSPPSDL`), the Flatpak id it wraps. Such a
/// wrapper is the Flatpak installation, not a native emulator.
#[must_use]
pub fn flatpak_wrapper_app_id(path: &Path) -> Option<&'static str> {
    use std::io::Read;
    if !regular_file(path) {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(4096)
        .read_to_end(&mut bytes)
        .ok()?;
    let text = std::str::from_utf8(&bytes).ok()?;
    if !text.starts_with("#!") || !text.contains("flatpak run") {
        return None;
    }
    KNOWN
        .iter()
        .flat_map(|def| def.flatpak_ids.iter())
        .find(|id| text.contains(*id))
        .copied()
}

/// Validates an AppImage path for launching directly.
pub fn validate_appimage(path: &Path) -> Result<(), &'static str> {
    if !path.is_absolute() {
        return Err("AppImage path is not absolute");
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| "AppImage does not exist")?;
    if metadata.file_type().is_symlink() {
        return Err("AppImage is a symlink");
    }
    if !metadata.is_file() {
        return Err("AppImage is not a regular file");
    }
    if !is_executable_file(path) {
        return Err("AppImage is not executable");
    }
    Ok(())
}

/// Resolved launch target for a Flatpak: the `flatpak` binary plus the
/// application id.
pub fn resolve_flatpak_launch(
    app_id: &str,
    roots: &KnownInstallRoots,
) -> Result<(PathBuf, LaunchInstallation), &'static str> {
    let installation = LaunchInstallation::flatpak(app_id).map_err(|_| "invalid Flatpak id")?;
    let installed = KNOWN
        .iter()
        .any(|def| def.flatpak_ids.contains(&app_id) && !discover_flatpaks(def, roots).is_empty());
    if !installed {
        return Err("Flatpak application is not installed");
    }
    let binary = flatpak_binary(roots).ok_or("the flatpak program was not found on PATH")?;
    Ok((binary, installation))
}

const KNOWN: [&KnownEmulator; 4] = [&PCSX2, &DUCKSTATION, &PPSSPP, &MELONDS];

#[cfg(test)]
mod tests;
