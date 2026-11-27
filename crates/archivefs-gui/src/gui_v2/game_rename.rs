//! Blue: rename one game from its Game Details page.
//!
//! Owns only this card's state and rendering. The rename itself is the core
//! one-game rename (`dat::rom_organisation::game_rename`), which reuses the
//! organiser's identity checks, collision refusal, journal, library rebinding
//! and History Undo. Nothing is changed until the person confirms a preview.

use archivefs_core::dat::reaudit::{GameRecheck, ReauditCatalogues, recheck_game};
use archivefs_core::dat::rom_organisation::game_rename::{
    GameRenamePreview, RetroArchSaveLocations, apply_game_rename, preview_game_rename,
    refuse_if_unfinished_rename,
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
    /// The optional RomM title update (or its restore after Undo) running.
    pub(super) romm: Option<super::romm_title::RommTitleJob>,
    /// Its result, shown under the rename outcome.
    pub(super) romm_line: Option<String>,
    /// RomM requests asked for while one was running.
    pub(super) romm_queued: std::collections::VecDeque<super::romm_title::RommTitleAction>,
    /// The rename just applied, for the RomM title update.
    renamed: Option<super::romm_title::RommTitleAction>,
    /// The file's current name when the card opened: the verified name
    /// replaces it only while the person has not typed anything else.
    opened_with: String,
    /// The verified catalogue name being looked up for the open game.
    suggestion: Option<std::sync::mpsc::Receiver<Option<Suggestion>>>,
    /// The catalogue the pre-filled name came from, shown under the field.
    pub(super) suggested_from: Option<String>,
    /// Test seam: the configured catalogues (default: the person's). With
    /// the database test seam set and this unset, nothing is looked up.
    catalogues: Option<
        std::collections::HashMap<
            String,
            archivefs_core::dat::configured_catalogues::ConfiguredCatalogue,
        >,
    >,
}

/// The verified name proposed for a game and the catalogue it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Suggestion {
    pub stem: String,
    pub catalogue: String,
}

/// The name (without extension; the file's own extension always stays) that
/// the verified catalogue gives this file, or `None` when there is no
/// trustworthy verified name.
///
/// This is bulk Rename's own rule ([`crate::rom_organisation_page::choose_name`])
/// with RomM titles left out, so single-game and bulk Rename always agree:
/// today a loose file whose catalogue file name has the same extension. An
/// archive matched through a file inside it keeps its name here exactly as
/// it does in bulk Rename; the owner's archive policy (2026-11-26) changes
/// that shared rule for both at once (Stage B).
pub(super) fn suggested_stem(
    dat: archivefs_core::VerifiedDatName,
    source: &std::path::Path,
) -> Option<Suggestion> {
    use crate::rom_organisation_page::{NameSource, choose_name};
    let choice = choose_name(dat, None, source);
    let NameSource::VerifiedDat { catalogue } = choice.source else {
        return None;
    };
    let name = choice.name?;
    // A catalogue name naming a folder is never a file name here.
    if name.contains(['/', '\\']) {
        return None;
    }
    let stem = std::path::Path::new(&name).file_stem()?.to_string_lossy();
    let stem = stem.trim();
    let safe = !stem.is_empty() && !stem.starts_with('.') && !stem.chars().any(char::is_control);
    safe.then(|| Suggestion {
        stem: stem.to_owned(),
        catalogue,
    })
}

/// Looks up the verified catalogue name on the worker: read-only, no
/// matching or hashing of the game.
fn suggest_now(
    database: &std::path::Path,
    archive_id: i64,
    source: &std::path::Path,
    catalogues: Option<
        std::collections::HashMap<
            String,
            archivefs_core::dat::configured_catalogues::ConfiguredCatalogue,
        >,
    >,
) -> Option<Suggestion> {
    let catalogues = match catalogues {
        Some(catalogues) => catalogues,
        None => ReauditCatalogues::load_default().ok()?.configured(),
    };
    let dat = archivefs_core::Database::open_read_only(database)
        .ok()?
        .verified_dat_name_for_archive(archive_id, &catalogues)
        .ok()?;
    suggested_stem(dat, source)
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
        self.opened_with = self.typed.clone();
        self.suggested_from = None;
        self.preview = None;
        self.outcome = None;
        self.recheck_line = None;
        self.romm_line = None;
        self.suggestion = None;
        // Tests that set the library seam but no catalogues never read the
        // person's catalogue settings.
        if self.paths.is_some() && self.catalogues.is_none() {
            return;
        }
        let Ok(database) = self.database_path() else {
            return;
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let (id, source, catalogues) = (
            game.archive.id,
            game.archive.absolute_path.clone(),
            self.catalogues.clone(),
        );
        std::thread::spawn(move || {
            let _ = sender.send(suggest_now(&database, id, &source, catalogues));
        });
        self.suggestion = Some(receiver);
    }

    /// Takes the verified name once it has been looked up. It fills the
    /// field only when the person has not changed the name yet.
    pub(super) fn poll_suggestion(&mut self) {
        let Some(receiver) = &self.suggestion else {
            return;
        };
        let found = match receiver.try_recv() {
            Ok(found) => found,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => None,
        };
        self.suggestion = None;
        if let Some(found) = found
            && self.typed == self.opened_with
            && self.preview.is_none()
        {
            self.typed = found.stem;
            self.suggested_from = Some(found.catalogue);
        }
    }

    fn close(&mut self) {
        *self = Self {
            generation: self.generation,
            paths: self.paths.take(),
            saves: self.saves.take(),
            catalogues: self.catalogues.take(),
            recheck: self.recheck.take(),
            queued: std::mem::take(&mut self.queued),
            romm: self.romm.take(),
            romm_queued: std::mem::take(&mut self.romm_queued),
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
            })
            .and_then(|preview| {
                // An unfinished earlier rename of this game shows now, with
                // the History entry to Undo, not only at confirm.
                refuse_if_unfinished_rename(&preview, &self.journal_dir()?)?;
                Ok(preview)
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
        self.romm_line = None;
        self.renamed =
            result
                .as_ref()
                .ok()
                .map(|outcome| super::romm_title::RommTitleAction::AfterRename {
                    transaction_id: outcome.transaction.transaction_id.clone(),
                    old: preview.source.clone(),
                    new: preview.destination.clone(),
                });
        let saves = match preview.saves.len() {
            0 => String::new(),
            1 => " with its RetroArch save".into(),
            n => format!(" with its {n} RetroArch saves"),
        };
        let romm = if super::romm_title::title_updates_enabled() {
            "RomM picks up the new file at its next scan of this system; its title is updated below."
        } else {
            "RomM picks up the new file at its next scan of this system; it keeps its current title (RomM is only read)."
        };
        self.outcome = Some(result.map(|_| {
            format!(
                "Renamed to {}{saves}. Undo is in History. {romm}",
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
        self.game_rename.poll_suggestion();
        if self.game_rename.suggestion.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
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
                self.game_rename.suggested_from = None;
            }
            if let Some(catalogue) = &self.game_rename.suggested_from {
                ui.label(
                    RichText::new(format!(
                        "Filled in with the verified name from the catalogue \"{catalogue}\". Preview before renaming."
                    ))
                    .color(theme::muted(ui)),
                );
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
                    match &self.game_rename.romm_line {
                        Some(line) => {
                            ui.label(line);
                        }
                        None if self.game_rename.romm.is_some() => {
                            ui.label(
                                RichText::new("Updating its title in RomM…")
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
                    if let Some(action) = self.game_rename.renamed.take() {
                        self.start_romm_title(action);
                    }
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
