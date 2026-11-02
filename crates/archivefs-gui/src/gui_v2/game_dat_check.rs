//! Game Details "Verify this game" / "Re-check this game": the existing
//! library verification engine bound to exactly one library row. Only that
//! game's file (and the companions its own descriptor names) is read, and only
//! its evidence is saved, under the same strict proof rules as a platform
//! check. If the game cannot be resolved to one eligible file the check stops;
//! it never widens to the platform. The platform-wide check stays a separate,
//! separately labelled action.
use super::library::Game;
use crate::dat_catalogue_picker::CatalogueInventorySnapshot;
use archivefs_core::dat::catalogue_selection::{
    CatalogueAvailability, CatalogueRef, InstalledCatalogueSummary, list_installed_catalogues,
    resolve_catalogue,
};
use archivefs_core::dat::library_verification::{
    VerificationPreview, VerificationTotals, preview_game_verification,
};
use archivefs_core::dat::sources::audit_run::DatAuditProgress;
use archivefs_core::safe_read::TrustedRoots;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[cfg(test)]
mod tests;

/// Everything a check reads from outside the page, loaded once per check.
#[derive(Clone)]
pub(super) struct Inputs {
    pub database: PathBuf,
    pub inventory: CatalogueInventorySnapshot,
    pub trusted: TrustedRoots,
}

/// The game a check is bound to: its library row and the path shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Target {
    pub archive_id: i64,
    pub title: String,
    pub platform: String,
    pub path: PathBuf,
}

impl Target {
    pub(super) fn of(game: &Game) -> Self {
        Self {
            archive_id: game.archive.id,
            title: game.title.clone(),
            // The engine takes a canonical system; an unknown one fails at
            // preview with nothing checked.
            platform: archivefs_core::canonical_platform_for_alias(&game.platform)
                .map(str::to_string)
                .unwrap_or_else(|| game.platform.clone()),
            path: game.archive.absolute_path.clone(),
        }
    }
}

/// What one finished check found for this game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Finding {
    Verified,
    NoMatch,
    Ambiguous,
    /// Hashed but with no definitive result; the reason comes from the engine.
    NotChecked(String),
}

pub(super) struct Finished {
    pub finding: Finding,
    pub totals: VerificationTotals,
}

#[derive(Default)]
pub(super) enum Stage {
    #[default]
    Loading,
    Choose,
    Previewing,
    Ready(Box<VerificationPreview>),
    Running(String),
    Finished(Box<Finished>),
    Failed(String),
}

enum Message {
    Catalogues(Result<(Inputs, Vec<InstalledCatalogueSummary>), String>),
    Preview(Result<Box<VerificationPreview>, String>),
    Progress(String),
    Done(Result<VerificationTotals, String>),
}

#[derive(Default)]
pub(super) struct State {
    pub target: Option<Target>,
    pub stage: Stage,
    inputs: Option<Inputs>,
    pub catalogues: Vec<InstalledCatalogueSummary>,
    pub choice: Option<CatalogueRef>,
    receiver: Option<Receiver<Message>>,
    cancel: Option<Arc<AtomicBool>>,
    /// Set once evidence was saved, so the library reloads current evidence.
    saved: bool,
}

/// Enabled, ready, verify-capable catalogues confirmed for this system: the
/// same rule Game Details guidance uses to decide a check is possible.
pub(super) fn ready_catalogues(
    rows: Vec<InstalledCatalogueSummary>,
    platform: &str,
) -> Vec<InstalledCatalogueSummary> {
    let Some(platform) = archivefs_core::canonical_platform_for_alias(platform) else {
        return Vec::new();
    };
    rows.into_iter()
        .filter(|row| {
            row.enabled
                && row.availability == CatalogueAvailability::Ready
                && row.capabilities.verify
                && row
                    .platform
                    .confirmed()
                    .and_then(|p| archivefs_core::canonical_platform_for_alias(p))
                    == Some(platform)
        })
        .collect()
}

fn progress_text(progress: DatAuditProgress) -> String {
    match progress {
        DatAuditProgress::Hashing { file_name, .. } => format!("Reading {file_name}…"),
        DatAuditProgress::ReadingCatalogue { .. } => "Reading the confirmed catalogue…".into(),
        DatAuditProgress::CatalogueReady { .. } => "Catalogue ready; preparing this game…".into(),
        DatAuditProgress::Comparing { .. } => "Comparing this game with the catalogue…".into(),
        _ => "Preparing this game…".into(),
    }
}

/// The one finding for the one game, from the engine's totals.
pub(super) fn finding(totals: &VerificationTotals) -> Finding {
    if totals.verified == 1 {
        Finding::Verified
    } else if totals.no_match == 1 {
        Finding::NoMatch
    } else if totals.ambiguous == 1 {
        Finding::Ambiguous
    } else {
        Finding::NotChecked(
            totals
                .unchecked
                .first()
                .map(|item| item.reason.clone())
                .unwrap_or_else(|| "No definitive result was produced.".into()),
        )
    }
}

impl State {
    pub(super) fn busy(&self) -> bool {
        self.receiver.is_some()
    }

    /// Binds the page to `target`. Only a confirmed, running check keeps its
    /// own game; catalogue loading or a preview for another game is dropped
    /// (both are read-only). A pending library reload is never lost.
    pub(super) fn open(
        &mut self,
        target: Target,
        load: impl FnOnce() -> Result<Inputs, String> + Send + 'static,
    ) {
        if self.target.as_ref() == Some(&target) || matches!(self.stage, Stage::Running(_)) {
            return;
        }
        *self = Self {
            target: Some(target),
            saved: self.saved,
            ..Default::default()
        };
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        std::thread::spawn(move || {
            let result = load().map(|inputs| {
                let rows = list_installed_catalogues(inputs.inventory.inputs());
                (inputs, rows)
            });
            let _ = sender.send(Message::Catalogues(result));
        });
    }

    pub(super) fn take_saved(&mut self) -> bool {
        std::mem::take(&mut self.saved)
    }

    pub(super) fn preview(&mut self) {
        let (Some(target), Some(inputs), Some(choice)) = (
            self.target.clone(),
            self.inputs.clone(),
            self.choice.clone(),
        ) else {
            return;
        };
        if self.busy() {
            return;
        }
        self.stage = Stage::Previewing;
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        std::thread::spawn(move || {
            let result = resolve_catalogue(&choice, inputs.inventory.inputs())
                .map_err(|e| e.to_string())
                .and_then(|resolved| {
                    preview_game_verification(
                        resolved,
                        inputs.database,
                        target.platform,
                        target.archive_id,
                        target.path,
                    )
                })
                .map(Box::new);
            let _ = sender.send(Message::Preview(result));
        });
    }

    pub(super) fn confirm(&mut self) {
        if self.busy() {
            return;
        }
        let Stage::Ready(_) = &self.stage else {
            return;
        };
        let Stage::Ready(preview) =
            std::mem::replace(&mut self.stage, Stage::Running("Starting…".into()))
        else {
            unreachable!()
        };
        let Some(trusted) = self.inputs.as_ref().map(|inputs| inputs.trusted.clone()) else {
            self.stage = Stage::Failed("Check inputs are unavailable. Nothing was checked.".into());
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        std::thread::spawn(move || {
            let progress = sender.clone();
            let result = preview.execute(&trusted, &cancel, &|p| {
                let _ = progress.send(Message::Progress(progress_text(p)));
            });
            let _ = sender.send(Message::Done(result));
        });
    }

    pub(super) fn cancel(&mut self) {
        match &self.stage {
            Stage::Running(_) => {
                if let Some(cancel) = &self.cancel {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
            Stage::Ready(_) => self.stage = Stage::Choose,
            _ => {}
        }
    }

    pub(super) fn poll(&mut self) {
        loop {
            let Some(receiver) = &self.receiver else {
                return;
            };
            let message = match receiver.try_recv() {
                Ok(message) => message,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.receiver = None;
                    self.cancel = None;
                    if matches!(
                        self.stage,
                        Stage::Loading | Stage::Previewing | Stage::Running(_)
                    ) {
                        self.stage = Stage::Failed(
                            "The check stopped unexpectedly. Nothing new was confirmed.".into(),
                        );
                    }
                    return;
                }
            };
            match message {
                Message::Progress(text) => {
                    if let Stage::Running(current) = &mut self.stage {
                        *current = text;
                    }
                }
                Message::Catalogues(result) => {
                    self.receiver = None;
                    match result {
                        Ok((inputs, rows)) => {
                            let platform = self
                                .target
                                .as_ref()
                                .map(|t| t.platform.clone())
                                .unwrap_or_default();
                            self.catalogues = ready_catalogues(rows, &platform);
                            self.choice = (self.catalogues.len() == 1)
                                .then(|| self.catalogues[0].reference.clone());
                            self.inputs = Some(inputs);
                            self.stage = Stage::Choose;
                        }
                        Err(error) => {
                            self.stage = Stage::Failed(format!("{error}. Nothing was checked."))
                        }
                    }
                }
                Message::Preview(result) => {
                    self.receiver = None;
                    self.stage = match result {
                        Ok(preview) => Stage::Ready(preview),
                        Err(error) => Stage::Failed(error),
                    };
                }
                Message::Done(result) => {
                    self.receiver = None;
                    self.cancel = None;
                    self.stage = match result {
                        Ok(totals) => {
                            self.saved = totals.saved > 0;
                            Stage::Finished(Box::new(Finished {
                                finding: finding(&totals),
                                totals,
                            }))
                        }
                        Err(error) => Stage::Failed(error),
                    };
                }
            }
        }
    }
}

/// Loads the real inputs: the library database, the configured catalogues and
/// the configured game folders as trusted roots. Run on the worker.
pub(super) fn load_inputs() -> Result<Inputs, String> {
    let database = archivefs_core::default_database_path().map_err(|e| e.to_string())?;
    let inventory =
        crate::dat_catalogue_picker::load_inventory_snapshot().map_err(|e| e.to_string())?;
    let trusted = archivefs_core::Config::load_default()
        .ok()
        .as_ref()
        .map(TrustedRoots::from_config)
        .unwrap_or_else(TrustedRoots::none);
    Ok(Inputs {
        database,
        inventory,
        trusted,
    })
}

pub(super) enum PageAction {
    BackToGame,
    CheckWholePlatform,
    SetUpCatalogues,
}

/// `opened` is the game whose page this is; while a confirmed check of
/// another game runs, the page says so rather than show that game as this one.
pub(super) fn show(
    ui: &mut eframe::egui::Ui,
    state: &mut State,
    opened: i64,
) -> Option<PageAction> {
    use crate::ui::theme;
    state.poll();
    let Some(target) = state.target.clone() else {
        ui.label("Open a game first.");
        return None;
    };
    let mut action = None;
    if target.archive_id != opened {
        ui.colored_label(
            theme::WARNING,
            format!(
                "Still checking {}. This page switches to the game you opened when that check finishes or is cancelled.",
                target.title
            ),
        );
    }
    ui.heading("Check this game against a DAT");
    ui.label(format!("{} · {}", target.title, target.platform));
    ui.small(target.path.display().to_string());
    ui.label("Only this game's file is read. No other game is checked or changed, and the file itself is never modified.");
    ui.add_space(8.0);
    match &state.stage {
        Stage::Loading => {
            ui.label("Reading the catalogues set up for this system…");
        }
        Stage::Choose => {
            if state.catalogues.is_empty() {
                ui.label(format!(
                    "No ready catalogue covers {}. Set one up first; nothing has been checked.",
                    target.platform
                ));
                if ui.button("Set up verification").clicked() {
                    action = Some(PageAction::SetUpCatalogues);
                }
            } else {
                ui.strong("Catalogue");
                for row in &state.catalogues {
                    let label = match &row.revision {
                        Some(version) => format!("{} · version {version}", row.display_name),
                        None => row.display_name.clone(),
                    };
                    ui.radio_value(&mut state.choice, Some(row.reference.clone()), label);
                }
                let ready = state.choice.is_some();
                if ui
                    .add_enabled(ready, eframe::egui::Button::new("Preview"))
                    .clicked()
                {
                    state.preview();
                }
                ui.small("Preview shows exactly what will be read. Nothing is checked until you confirm.");
            }
        }
        Stage::Previewing => {
            ui.label("Preparing the preview. No game content is read yet.");
        }
        Stage::Ready(preview) => {
            ui.strong("Ready to check this game");
            ui.label(format!(
                "Catalogue: {} · version {}",
                preview.catalogue_name,
                preview.version.as_deref().unwrap_or("not stated")
            ));
            ui.small(format!("Catalogue SHA-256: {}", preview.sha256));
            for path in preview.eligible_paths() {
                ui.label(format!("File: {}", path.display()));
            }
            ui.label(format!("Bytes to read: {}", preview.bytes));
            ui.label("Only this game's result is saved. Its previous saved result for this catalogue is replaced.");
            ui.horizontal(|ui| {
                if ui.button("Confirm check").clicked() {
                    state.confirm();
                }
                if ui.button("Cancel").clicked() {
                    state.cancel();
                }
            });
        }
        Stage::Running(progress) => {
            ui.label(progress);
            if ui.button("Cancel check").clicked() {
                state.cancel();
            }
            ui.small("Cancelling stops before anything is saved. A save already committing finishes as one step.");
        }
        Stage::Finished(finished) => {
            let (headline, detail) = match &finished.finding {
                Finding::Verified => (
                    "Verified".to_string(),
                    format!("This file matches one entry in {}.", finished.totals.catalogue),
                ),
                Finding::NoMatch => (
                    "No match".to_string(),
                    format!(
                        "This file matches no entry in {}. That does not prove the file is damaged.",
                        finished.totals.catalogue
                    ),
                ),
                Finding::Ambiguous => (
                    "More than one possible match".to_string(),
                    "The file matches several catalogue entries, so no single identity was recorded.".to_string(),
                ),
                Finding::NotChecked(reason) => (
                    "Not checked".to_string(),
                    format!("{reason}. Nothing new was confirmed for this game."),
                ),
            };
            ui.strong(theme::card_title(headline));
            ui.label(detail);
            if finished.totals.errors > 0 {
                ui.colored_label(theme::WARNING, "The file could not be read completely.");
            }
            ui.small(format!(
                "Catalogue SHA-256: {}",
                finished.totals.catalogue_sha256
            ));
            if ui.button("Check again").clicked() {
                state.stage = Stage::Choose;
            }
        }
        Stage::Failed(error) => {
            ui.colored_label(theme::WARNING, error);
            if ui.button("Try again").clicked() {
                if state.inputs.is_none() {
                    // Catalogues never loaded: the page reloads them on its next frame.
                    state.target = None;
                } else {
                    state.stage = Stage::Choose;
                }
            }
        }
    }
    if state.busy() {
        ui.ctx().request_repaint();
    }
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        if ui.button("Back to Game Details").clicked() {
            action = Some(PageAction::BackToGame);
        }
        if ui
            .add_enabled(
                !state.busy(),
                eframe::egui::Button::new(format!("Check all {} games instead", target.platform)),
            )
            .on_hover_text("Opens the platform check, which reads every game on this system.")
            .clicked()
        {
            action = Some(PageAction::CheckWholePlatform);
        }
    });
    action
}
