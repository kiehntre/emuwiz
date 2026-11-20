//! Blue: rename one game from its Game Details page.
//!
//! Owns only this card's state and rendering. The rename itself is the core
//! one-game rename (`dat::rom_organisation::game_rename`), which reuses the
//! organiser's identity checks, collision refusal, journal, library rebinding
//! and History Undo. Nothing is changed until the person confirms a preview.

use archivefs_core::dat::reaudit::{GameRecheck, ReauditCatalogues, recheck_game};
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
    /// The one-game catalogue re-check started after a rename or its Undo.
    recheck: Option<Recheck>,
    /// Its result, shown under the rename outcome.
    recheck_line: Option<String>,
    /// Re-checks asked for while one was running; started in turn.
    queued: std::collections::VecDeque<RecheckTarget>,
    /// A finished re-check saved a result: reload once no load is running.
    pub(super) reload_pending: bool,
}

/// A running re-check: its Activity job and the worker's answer.
struct Recheck {
    job: u64,
    /// The game when known up front (a rename); an Undo finds it by path.
    archive_id: Option<i64>,
    receiver: std::sync::mpsc::Receiver<Result<GameRecheck, String>>,
}

/// Which game to re-check.
pub(super) enum RecheckTarget {
    Id(i64),
    /// The library game at this path (after an Undo put the old name back).
    Path(std::path::PathBuf),
}

/// Re-checks `archive_id`'s saved catalogue results against the configured
/// catalogues, on the worker. Read again from the person's settings.
fn recheck_now(database: &std::path::Path, game: RecheckTarget) -> Result<GameRecheck, String> {
    let archive_id = match game {
        RecheckTarget::Id(id) => id,
        RecheckTarget::Path(path) => archivefs_core::Database::open_read_only(database)
            .and_then(|db| db.find_archive_id_by_absolute_path(&path))
            .map_err(|error| error.to_string())?
            .ok_or("the game is not in the library at its restored name")?,
    };
    let catalogues = ReauditCatalogues::load_default()?;
    let trusted = archivefs_core::Config::load_default()
        .ok()
        .as_ref()
        .map(archivefs_core::safe_read::TrustedRoots::from_config)
        .unwrap_or_else(archivefs_core::safe_read::TrustedRoots::none);
    recheck_game(
        database,
        archive_id,
        &catalogues.configured(),
        &|source| catalogues.resolve(source),
        &trusted,
        &std::sync::atomic::AtomicBool::new(false),
    )
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
        self.recheck_line = None;
    }

    fn close(&mut self) {
        *self = Self {
            generation: self.generation,
            paths: self.paths.take(),
            saves: self.saves.take(),
            recheck: self.recheck.take(),
            queued: std::mem::take(&mut self.queued),
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
        self.recheck_line = None;
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
        let saves = match preview.saves.len() {
            0 => String::new(),
            1 => " with its RetroArch save".into(),
            n => format!(" with its {n} RetroArch saves"),
        };
        self.outcome = Some(result.map(|_| {
            format!(
                "Renamed to {}{saves}. Undo is in History. RomM shows the new name after its next scan of this system.",
                preview.new_file_name
            )
        }));
        renamed
    }
}

impl App {
    /// Starts the one-game re-check for `archive_id` (after a rename or its
    /// Undo). A catalogue result is tied to the file's name, so it is read
    /// again now rather than carried over. One runs at a time; a request
    /// made meanwhile waits its turn and is never dropped.
    pub(super) fn start_rename_recheck(&mut self, game: RecheckTarget) {
        if self.game_rename.recheck.is_some() {
            self.game_rename.queued.push_back(game);
            return;
        }
        // Tests never reach the person's own library or catalogues.
        if cfg!(test) && self.game_rename.paths.is_none() {
            return;
        }
        let archive_id = match &game {
            RecheckTarget::Id(id) => Some(*id),
            RecheckTarget::Path(_) => None,
        };
        let Ok(database) = self.game_rename.database_path() else {
            return;
        };
        let job = self.activity.queue(
            "Re-checking the renamed game",
            match archive_id {
                Some(id) => super::Route::Game(id),
                None => super::Route::Section(super::Section::Games),
            },
            false,
        );
        self.activity.start(job);
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(recheck_now(&database, game));
        });
        self.game_rename.recheck = Some(Recheck {
            job,
            archive_id,
            receiver,
        });
    }

    /// Collects a finished re-check; a saved result sets `reload_pending` so
    /// Game Details shows it.
    pub(super) fn poll_rename_recheck(&mut self) {
        let Some(running) = &self.game_rename.recheck else {
            return;
        };
        let result = match running.receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("the re-check stopped unexpectedly".into())
            }
        };
        let Recheck {
            job, archive_id, ..
        } = self.game_rename.recheck.take().expect("checked above");
        let (line, error) = match &result {
            Ok(recheck) => match recheck.summary() {
                Some(line) => (line, None),
                None => ("This game had no catalogue result to re-check.".into(), None),
            },
            Err(error) => (
                "The game could not be re-checked. It shows as not verified until you re-check it (Re-check this game).".into(),
                Some(error.clone()),
            ),
        };
        self.activity.finish(job, line.clone(), error);
        if archive_id.is_some() && self.game_rename.open_for == archive_id {
            self.game_rename.recheck_line = Some(line);
        }
        if let Some(next) = self.game_rename.queued.pop_front() {
            self.start_rename_recheck(next);
        }
        self.game_rename.reload_pending |=
            result.is_ok_and(|recheck| recheck.report.is_some_and(|report| report.saved > 0));
    }

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
                    match &self.game_rename.recheck_line {
                        Some(line) => {
                            ui.label(line);
                        }
                        None if self.game_rename.recheck.is_some() => {
                            ui.label(
                                RichText::new("Re-checking its catalogue result…")
                                    .color(theme::muted(ui)),
                            );
                        }
                        None => {}
                    }
                }
                Some(Err(reason)) => {
                    ui.colored_label(theme::warning(ui), reason);
                }
                None => {}
            }
            if renamed {
                // The library row keeps its id; only real settings run it.
                if self.game_rename.paths.is_none() {
                    self.start_rename_recheck(RecheckTarget::Id(game.archive.id));
                }
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
