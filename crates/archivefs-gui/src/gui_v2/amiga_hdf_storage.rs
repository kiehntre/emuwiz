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
}

fn mib(bytes: u64) -> String {
    let gib = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    format!("{gib:.1} GiB")
}

fn load() -> Result<View, String> {
    let storage = storage::effective_storage()?;
    let facts = storage::storage_facts(&storage.root);
    let earlier = storage
        .earlier_roots
        .iter()
        .map(|root| {
            let copies = storage::storage_facts(root).map(|f| f.copies).unwrap_or(0);
            (root.clone(), copies)
        })
        .filter(|(root, copies)| *copies > 0 && root.is_dir())
        .collect();
    Ok(View {
        storage,
        facts,
        earlier,
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
        match self.loaded.as_ref() {
            Some(Err(error)) => {
                ui.colored_label(ui.visuals().warn_fg_color, format!("Storage folder unavailable: {error}"));
            }
            Some(Ok(view)) => {
                let source = match view.storage.source {
                    StorageSource::Default => "default",
                    StorageSource::Chosen => "chosen in Settings",
                    StorageSource::Override => "set by EMUWIZ_AMIGA_HDF_WORKING_DIR",
                };
                ui.label(format!("New copies go to: {} ({source})", view.storage.root.display()));
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
                        ui.colored_label(ui.visuals().warn_fg_color, format!("Free space unknown: {error}"));
                    }
                }
                for (root, copies) in &view.earlier {
                    ui.label(format!(
                        "Still used: {copies} game copies in {}. Those games keep playing from there with their saves. To move them, quit EmuWiz and move the numbered folders yourself into the folder above.",
                        root.display()
                    ));
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
        if let Some(choice) = choose {
            self.message = Some(match storage::choose_storage(choice.as_deref()) {
                Ok(next) => format!("New Amiga hard-disk copies will go to {}.", next.root.display()),
                Err(error) => format!("Folder not changed: {error}"),
            });
            self.loaded = None;
        }
        if let Some(message) = &self.message {
            ui.label(message);
        }
    }
}
