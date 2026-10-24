//! User-selected, read-only roots for the existing ES-DE and LaunchBox artwork providers.
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

const SETTINGS_FILE: &str = "local-artwork-folders.json";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Layout {
    EsDe,
    LaunchBox,
}

impl Layout {
    fn label(self) -> &'static str {
        match self {
            Self::EsDe => "ES-DE",
            Self::LaunchBox => "LaunchBox",
        }
    }
    fn detect(root: &Path) -> Option<Self> {
        if root.join("gamelists").is_dir() && root.join("downloaded_media").is_dir() {
            Some(Self::EsDe)
        } else if root.join("Data/Platforms").is_dir() && root.join("Images").is_dir() {
            Some(Self::LaunchBox)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct Folder {
    pub path: PathBuf,
    pub layout: Layout,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
struct Settings {
    folders: Vec<Folder>,
}

pub(super) fn configured() -> Vec<Folder> {
    path()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<Settings>(&bytes).ok())
        .map(|settings| settings.folders)
        .unwrap_or_default()
}

fn path() -> Option<PathBuf> {
    archivefs_core::app_dirs::config_path(SETTINGS_FILE).ok()
}

fn save(folders: &[Folder]) -> Result<(), String> {
    let path = path().ok_or("EmuWiz settings folder is unavailable")?;
    let parent = path
        .parent()
        .ok_or("EmuWiz settings folder is unavailable")?;
    fs::create_dir_all(parent).map_err(|e| format!("Could not save artwork folders: {e}"))?;
    let bytes = serde_json::to_vec_pretty(&Settings {
        folders: folders.to_vec(),
    })
    .map_err(|e| e.to_string())?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, bytes).map_err(|e| format!("Could not save artwork folders: {e}"))?;
    fs::rename(temp, path).map_err(|e| format!("Could not save artwork folders: {e}"))
}

pub(super) fn safe_existing(folder: &Folder) -> bool {
    let Ok(meta) = fs::symlink_metadata(&folder.path) else {
        return false;
    };
    meta.is_dir()
        && !meta.file_type().is_symlink()
        && fs::canonicalize(&folder.path).is_ok_and(|canonical| canonical == folder.path)
        && Layout::detect(&folder.path) == Some(folder.layout)
}

/// Settings for read-only local artwork providers.
pub(super) fn show(ui: &mut egui::Ui, folders: &mut Vec<Folder>) -> bool {
    let mut changed = false;
    ui.separator();
    ui.heading("Local Artwork Folders");
    ui.label("Use existing ES-DE or LaunchBox artwork from a folder on this device. EmuWiz reads these folders; it never renames or rewrites their files.");
    ui.label("Matches use the providers' exact game and platform evidence. Filename-only LaunchBox title candidates are not auto-associated.");
    if ui.button("Add local artwork folder…").clicked()
        && let Some(path) = rfd::FileDialog::new()
            .set_title("Select an ES-DE or LaunchBox artwork folder")
            .pick_folder()
    {
        let canonical = fs::canonicalize(&path).ok();
        if let Some(canonical) = canonical.filter(|p| p.is_dir()) {
            if let Some(layout) = Layout::detect(&canonical) {
                if !folders.iter().any(|folder| folder.path == canonical) {
                    folders.push(Folder {
                        path: canonical,
                        layout,
                        enabled: true,
                    });
                    changed = true;
                }
            } else {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "This folder does not contain a supported ES-DE or LaunchBox artwork layout.",
                );
            }
        }
    }
    if folders.is_empty() {
        ui.label("No local folders configured. Choose an ES-DE or LaunchBox folder to add one.");
    }
    let mut remove = None;
    for (index, folder) in folders.iter_mut().enumerate() {
        ui.group(|ui| {
            ui.horizontal(|ui| {
                if ui.checkbox(&mut folder.enabled, "Enabled").changed() { changed = true; }
                ui.strong(folder.layout.label());
                if ui.small_button("Remove from EmuWiz").clicked() { remove = Some(index); }
            });
            ui.label(folder.path.display().to_string());
            if safe_existing(folder) {
                ui.label(match folder.layout { Layout::EsDe => "Detected: gamelists/<system>/gamelist.xml and downloaded_media. The current Blue resolver displays verified covers and screenshots.", Layout::LaunchBox => "Detected: Data/Platforms XML and Images/<platform>/<title>. Core indexes covers, screenshots, logos, fanart, banners, video and manuals; Blue currently displays resolved covers and screenshots." });
            } else { ui.colored_label(egui::Color32::YELLOW, "Folder is missing, disconnected, inaccessible or its layout changed. It is not indexed."); }
        });
    }
    if let Some(index) = remove {
        folders.remove(index);
        changed = true;
    }
    let mut saved = changed;
    if changed {
        if let Err(error) = save(folders) {
            ui.colored_label(egui::Color32::RED, error);
            saved = false;
        } else {
            ui.label("Saved. Artwork providers are refreshing through the existing cache index.");
        }
    }
    saved
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_supported_layouts_and_refuses_symlink_root() {
        let dir = tempfile::tempdir().unwrap();
        let esde = dir.path().join("esde");
        fs::create_dir_all(esde.join("gamelists/nes")).unwrap();
        fs::create_dir_all(esde.join("downloaded_media")).unwrap();
        assert_eq!(Layout::detect(&esde), Some(Layout::EsDe));
        let launchbox = dir.path().join("lb");
        fs::create_dir_all(launchbox.join("Data/Platforms")).unwrap();
        fs::create_dir_all(launchbox.join("Images")).unwrap();
        assert_eq!(Layout::detect(&launchbox), Some(Layout::LaunchBox));
        assert!(!safe_existing(&Folder {
            path: dir.path().join("disconnected"),
            layout: Layout::EsDe,
            enabled: true,
        }));
        assert!(!safe_existing(&Folder {
            path: launchbox.clone(),
            layout: Layout::EsDe,
            enabled: true,
        }));
        let link = dir.path().join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&esde, &link).unwrap();
        #[cfg(unix)]
        assert!(!safe_existing(&Folder {
            path: link,
            layout: Layout::EsDe,
            enabled: true
        }));
    }
}
