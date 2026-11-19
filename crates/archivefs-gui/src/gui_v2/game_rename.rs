//! Blue: rename one game from its Game Details page.
//!
//! Owns only this card's state and rendering. The rename itself is the core
//! one-game rename (`dat::rom_organisation::game_rename`), which reuses the
//! organiser's identity checks, collision refusal, journal, library rebinding
//! and History Undo. Nothing is changed until the person confirms a preview.

use archivefs_core::dat::rom_organisation::game_rename::{
    GameRenamePreview, RetroArchSaveLocations, apply_game_rename, preview_game_rename,
};
use eframe::egui::{self, RichText};

use super::{App, library::Game};
use crate::ui::theme;

#[derive(Default)]
pub(super) struct State {
    /// The game whose rename card is open.
    open_for: Option<i64>,
    typed: String,
    preview: Option<Result<GameRenamePreview, String>>,
    outcome: Option<Result<String, String>>,
    generation: u64,
    /// Test seam: library database and History folder (default: the user's).
    paths: Option<(std::path::PathBuf, std::path::PathBuf)>,
    /// Test seam: where RetroArch saves are looked for (default: discovered
    /// from the installed RetroArch configuration).
    saves: Option<RetroArchSaveLocations>,
}

impl State {
    pub(super) fn is_open_for(&self, id: i64) -> bool {
        self.open_for == Some(id)
    }

    pub(super) fn open(&mut self, game: &Game) {
        self.open_for = Some(game.archive.id);
        self.typed = game
            .archive
            .absolute_path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.preview = None;
        self.outcome = None;
    }

    fn close(&mut self) {
        *self = Self {
            generation: self.generation,
            paths: self.paths.take(),
            saves: self.saves.take(),
            ..Self::default()
        };
    }

    fn database_path(&self) -> Result<std::path::PathBuf, String> {
        match &self.paths {
            Some((database, _)) => Ok(database.clone()),
            None => archivefs_core::default_database_path()
                .map_err(|error| format!("Could not find the library: {error}")),
        }
    }

    fn journal_dir(&self) -> Result<std::path::PathBuf, String> {
        match &self.paths {
            Some((_, journal)) => Ok(journal.clone()),
            None => archivefs_core::dat::rename_apply::journal::default_rename_transaction_dir()
                .map_err(|error| {
                    format!("History is unavailable, so nothing was changed: {error}")
                }),
        }
    }

    /// Builds the read-only preview against the live library.
    pub(super) fn preview_now(&mut self, source: &std::path::Path) {
        self.generation += 1;
        self.outcome = None;
        let result = self
            .database_path()
            .and_then(|path| {
                archivefs_core::Database::open_read_only(path)
                    .map_err(|error| format!("Could not open the library: {error}"))
            })
            .map_err(|error| format!("Could not open the library: {error}"))
            .and_then(|database| {
                let saves = self
                    .saves
                    .clone()
                    .unwrap_or_else(RetroArchSaveLocations::discover);
                preview_game_rename(&database, source, &self.typed, self.generation, &saves)
            });
        self.preview = Some(result);
    }

    /// Applies the reviewed preview. Returns true when the file was renamed,
    /// so the caller reloads the game list.
    pub(super) fn confirm(&mut self) -> bool {
        let Some(Ok(preview)) = self.preview.take() else {
            return false;
        };
        let result = self.journal_dir().and_then(|journal| {
            let database = self.database_path().and_then(|path| {
                archivefs_core::Database::open_existing(path)
                    .map_err(|error| format!("Could not open the library: {error}"))
            })?;
            // Read RetroArch's save folders again: it may have run since.
            let saves = self
                .saves
                .clone()
                .unwrap_or_else(RetroArchSaveLocations::discover);
            apply_game_rename(
                &preview,
                &database,
                &journal,
                &std::sync::atomic::AtomicBool::new(false),
                &saves,
            )
        });
        let renamed = result.is_ok();
        self.outcome = Some(result.map(|_| {
            format!(
                "Renamed to {}. Undo is in History. RomM refresh pending: RomM shows the new name after its next scan of this system.",
                preview.new_file_name
            )
        }));
        renamed
    }
}

impl App {
    /// The inline rename card under "More for this game".
    pub(super) fn game_rename_card(&mut self, ui: &mut egui::Ui, game: &Game) {
        if !self.game_rename.is_open_for(game.archive.id) {
            return;
        }
        ui.add_space(theme::SPACE_MD);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(theme::section_title("Rename this game's file"));
            ui.label(
                RichText::new(game.archive.absolute_path.display().to_string())
                    .color(theme::muted(ui)),
            );
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.game_rename.typed)
                    .hint_text("New name (the extension stays)")
                    .desired_width(f32::INFINITY),
            );
            if edit.changed() {
                self.game_rename.preview = None;
            }
            let mut renamed = false;
            ui.horizontal(|ui| {
                if ui.button("Preview").clicked() {
                    self.game_rename.preview_now(&game.archive.absolute_path);
                }
                let ready = matches!(self.game_rename.preview, Some(Ok(_)));
                if ui
                    .add_enabled(ready, egui::Button::new("Rename"))
                    .on_disabled_hover_text("Preview first.")
                    .clicked()
                {
                    renamed = self.game_rename.confirm();
                }
                if ui.button("Close").clicked() {
                    self.game_rename.close();
                }
            });
            match &self.game_rename.preview {
                Some(Ok(preview)) => {
                    for note in &preview.notes {
                        ui.label(format!("• {note}"));
                    }
                }
                Some(Err(reason)) => {
                    ui.colored_label(theme::warning(ui), reason);
                }
                None => {}
            }
            match &self.game_rename.outcome {
                Some(Ok(message)) => {
                    ui.label(RichText::new(message).strong());
                }
                Some(Err(reason)) => {
                    ui.colored_label(theme::warning(ui), reason);
                }
                None => {}
            }
            if renamed {
                self.load(false);
                // History lists the new rename (and its Undo) without a restart.
                self.send(0, super::Command::LoadRepairHistory);
            }
        });
    }
}

#[cfg(test)]
#[path = "game_rename_tests.rs"]
mod tests;
