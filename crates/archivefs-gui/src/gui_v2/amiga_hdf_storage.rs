//! Settings panel: where Blue keeps private, writable copies of Amiga
//! hard-disk (`.hdf`) games and the saves inside them. Choosing a folder never
//! moves or deletes existing copies; earlier folders keep being searched.

use archivefs_core::launch::amiga_hdf_working_copy::{
    self as storage, EffectiveStorage, StorageFacts, StorageSource,
};
use eframe::egui;
use std::path::PathBuf;

#[derive(Default)]
pub(super) struct AmigaHdfStorageState {
    loaded: Option<Result<View, String>>,
    message: Option<String>,
}

struct View {
    storage: EffectiveStorage,
    facts: Result<StorageFacts, String>,
    earlier: Vec<(PathBuf, usize)>,
    /// Remembered folders that cannot be used now: (folder, still exists).
    /// A folder that exists here has changed identity (unmounted drive's
    /// empty mount point, or a replaced folder).
    unavailable: Vec<(PathBuf, bool)>,
}

fn mib(bytes: u64) -> String {
    let gib = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    format!("{gib:.1} GiB")
}

fn load() -> Result<View, String> {
    let storage = storage::effective_storage()?;
    let facts = storage::storage_facts(&storage.root);
    let exists =
        |p: &std::path::Path| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_dir());
    let mut earlier = Vec::new();
    let mut unavailable: Vec<(PathBuf, bool)> = Vec::new();
    let mut checked: Vec<PathBuf> = storage.earlier_roots.clone();
    checked.push(storage.root.clone());
    if let Some(parent) = storage.root.parent() {
        checked.push(parent.to_path_buf());
    }
    for folder in &checked {
        let here = exists(folder);
        let changed = storage::folder_matches_record(folder, &storage.identities) == Some(false);
        if changed || (!here && storage.used_roots.contains(folder)) {
            if !unavailable.iter().any(|(f, _)| f == folder) {
                unavailable.push((folder.clone(), here));
            }
            continue;
        }
        if here && storage.earlier_roots.contains(folder) {
            let copies = storage::storage_facts(folder)
                .map(|f| f.copies)
                .unwrap_or(0);
            if copies > 0 {
                earlier.push((folder.clone(), copies));
            }
        }
    }
    Ok(View {
        storage,
        facts,
        earlier,
        unavailable,
    })
}

impl AmigaHdfStorageState {
    pub(super) fn render(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.label(crate::ui::theme::card_title("Amiga hard-disk game copies"));
        ui.label("Amiga hard-disk (.hdf) games save into the disk image itself, so Blue plays a private copy made the first time you press Play. Your original game files are never changed. These copies hold your in-game saves: they are not a cache and EmuWiz never deletes them.");
        if self.loaded.is_none() {
            self.loaded = Some(load());
        }
        let mut choose: Option<Option<PathBuf>> = None;
        let mut forget: Option<PathBuf> = None;
        let mut accept: Option<PathBuf> = None;
        let mut refresh = false;
        match self.loaded.as_ref() {
            Some(Err(error)) => {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("Storage folder unavailable: {error}"),
                );
                ui.horizontal_wrapped(|ui| {
                    refresh = ui.button("Check again").clicked();
                    if ui.button("Choose folder…").clicked()
                        && let Some(folder) = rfd::FileDialog::new()
                            .set_title("Folder for Amiga hard-disk game copies and saves")
                            .pick_folder()
                    {
                        choose = Some(Some(folder));
                    }
                });
            }
            Some(Ok(view)) => {
                let source = match view.storage.source {
                    StorageSource::Default => "default",
                    StorageSource::Chosen => "chosen in Settings",
                    StorageSource::Override => "set by EMUWIZ_AMIGA_HDF_WORKING_DIR",
                };
                ui.label(format!(
                    "New copies go to: {} ({source})",
                    view.storage.root.display()
                ));
                match &view.facts {
                    Ok(facts) => {
                        ui.label(format!(
                            "Free on that drive: {} of {}. A copy is only made if {} stays free afterwards. Games copied here: {}.",
                            mib(facts.available),
                            mib(facts.total),
                            mib(facts.reserve),
                            facts.copies
                        ));
                        if facts.available < facts.reserve {
                            ui.colored_label(ui.visuals().warn_fg_color, "This drive is below the safety reserve: new Amiga hard-disk games will not start until you free space or choose a folder on another drive.");
                        }
                    }
                    Err(error) => {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            format!("Free space unknown: {error}"),
                        );
                    }
                }
                for (root, copies) in &view.earlier {
                    ui.label(format!(
                        "Still used: {copies} game copies in {}. Those games keep playing from there with their saves. To move them, quit EmuWiz and move the numbered folders yourself into the folder above.",
                        root.display()
                    ));
                }
                let in_use = |f: &PathBuf| {
                    f == &view.storage.root || Some(f.as_path()) == view.storage.root.parent()
                };
                for (root, here) in &view.unavailable {
                    let text = if *here {
                        format!(
                            "Changed: {} is not the same folder as before. It is on a different drive or was replaced; this is what an unmounted drive looks like. Amiga hard-disk games will not start until the drive is mounted again. Nothing is created there.",
                            root.display()
                        )
                    } else {
                        format!(
                            "Not available: {}. It holds game copies and saves, so Amiga hard-disk games will not start until it is back (reconnect the drive).",
                            root.display()
                        )
                    };
                    ui.colored_label(ui.visuals().warn_fg_color, text);
                    ui.horizontal_wrapped(|ui| {
                        if *here
                            && ui
                                .button("Accept as it is now")
                                .on_hover_text("Only if you moved or restored this folder yourself and it holds your copies. Never for a drive that is just not plugged in: mount it instead.")
                                .clicked()
                        {
                            accept = Some(root.clone());
                        }
                        if !in_use(root)
                            && ui
                                .button("Forget")
                                .on_hover_text("EmuWiz stops looking here; the saves in it are no longer used. Nothing is deleted.")
                                .clicked()
                        {
                            forget = Some(root.clone());
                        }
                    });
                }
                if !view.unavailable.is_empty() {
                    refresh |= ui.button("Check again").clicked();
                }
                if view.storage.source == StorageSource::Override {
                    ui.label("The environment variable takes priority; unset it to use the folder chosen here.");
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Choose folder…").clicked()
                        && let Some(folder) = rfd::FileDialog::new()
                            .set_title("Folder for Amiga hard-disk game copies and saves")
                            .pick_folder()
                    {
                        choose = Some(Some(folder));
                    }
                    if view.storage.source == StorageSource::Chosen
                        && ui.button("Use default folder").clicked()
                    {
                        choose = Some(None);
                    }
                });
                ui.label(format!(
                    "Choosing a folder creates \"{}\" inside it. Nothing is moved or deleted.",
                    storage::CHOSEN_SUBFOLDER
                ));
            }
            None => {}
        }
        if let Some(folder) = forget {
            self.message = Some(match storage::forget_storage_folder(&folder) {
                Ok(()) => format!(
                    "EmuWiz no longer looks in {}. Nothing there was changed.",
                    folder.display()
                ),
                Err(error) => format!("Not changed: {error}"),
            });
            self.loaded = None;
        }
        if let Some(folder) = accept {
            self.message = Some(match storage::accept_storage_folder(&folder) {
                Ok(()) => format!("{} accepted as it is now.", folder.display()),
                Err(error) => format!("Not changed: {error}"),
            });
            self.loaded = None;
        }
        if refresh {
            self.loaded = None;
        }
        if let Some(choice) = choose {
            self.message = Some(match storage::choose_storage(choice.as_deref()) {
                Ok(next) => format!(
                    "New Amiga hard-disk copies will go to {}.",
                    next.root.display()
                ),
                Err(error) => format!("Folder not changed: {error}"),
            });
            self.loaded = None;
        }
        if let Some(message) = &self.message {
            ui.label(message);
        }
    }
}
