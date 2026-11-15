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
    CatalogueAvailability, CatalogueRef, EvidenceValue, InstalledCatalogueSummary,
    list_installed_catalogues, resolve_catalogue,
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
    Catalogues(Result<(Inputs, Vec<InstalledCatalogueSummary>, Vec<HeaderMatch>), String>),
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
    /// Ready local catalogues whose header names this system among others.
    /// They are offered only for explicit confirmation, never preselected.
    pub suggested: Vec<InstalledCatalogueSummary>,
    /// The last "use for this system" save, as the existing setup reported it.
    pub assignment: Option<Result<String, String>>,
    /// `(local source id, system, catalogue name)` for the check now running:
    /// the core preview proved this catalogue is for this system and no
    /// system is saved for it yet (the existing post-verification rule).
    executing: Option<(String, String, String)>,
    /// Offered after a Verified result only; nothing is saved until Yes.
    pub assignable: Option<(String, String, String)>,
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

/// `(local source id, system)` for each catalogue whose validated header the
/// existing "Apply recognised systems" rule recognises as one games system
/// and that has no system saved yet. Read from saved validation; no file is
/// opened and nothing is written.
pub(super) type HeaderMatch = (String, &'static str);

fn header_matches(inputs: &Inputs) -> Vec<HeaderMatch> {
    archivefs_core::dat::sources::recognition::safe_matches(&inputs.inventory.local_registry)
        .into_iter()
        .map(|found| (found.source_id, found.platform))
        .collect()
}

/// Enabled, ready, verify-capable local catalogues with no saved system
/// whose own header names this game's system. Offered for an explicit,
/// one-game trial only: never preselected, never saved from here. The core
/// preview re-proves the header before anything is read.
pub(super) fn suggested_catalogues(
    rows: &[InstalledCatalogueSummary],
    matches: &[HeaderMatch],
    platform: &str,
) -> Vec<InstalledCatalogueSummary> {
    let Some(platform) = archivefs_core::canonical_platform_for_alias(platform) else {
        return Vec::new();
    };
    rows.iter()
        .filter(|row| {
            row.enabled
                && row.availability == CatalogueAvailability::Ready
                && row.capabilities.verify
                && matches!(row.platform, EvidenceValue::Unknown)
                && local_source_id(row).is_some_and(|id| {
                    matches.iter().any(|(source, system)| {
                        source == id
                            && archivefs_core::canonical_platform_for_alias(system)
                                == Some(platform)
                    })
                })
        })
        .cloned()
        .collect()
}

/// The existing post-verification rule (Check My Games): only a local
/// catalogue the core preview proved is for the selected system, with no
/// system saved for it yet, may be offered for a later explicit assignment.
pub(super) fn assignment_candidate(
    row: &InstalledCatalogueSummary,
    preview_platform: &str,
    target_platform: &str,
) -> Option<(String, String, String)> {
    let source_id = local_source_id(row)?;
    (preview_platform == target_platform
        && row.platform_origin == archivefs_core::dat::sources::PlatformProvenance::NotSet)
        .then(|| {
            (
                source_id.to_string(),
                target_platform.to_string(),
                row.display_name.clone(),
            )
        })
}

/// Stricter than Check My Games: only a Verified result offers "from now
/// on". No match, more than one match and not-checked results offer nothing.
pub(super) fn assignable_after(
    finding: &Finding,
    executing: Option<(String, String, String)>,
) -> Option<(String, String, String)> {
    matches!(finding, Finding::Verified)
        .then_some(executing)
        .flatten()
}

/// The registry source a suggested catalogue's confirmation is saved to.
pub(super) fn local_source_id(row: &InstalledCatalogueSummary) -> Option<&str> {
    match &row.reference {
        CatalogueRef::Local {
            source_id,
            member: None,
        } => Some(source_id),
        _ => None,
    }
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
                let matches = header_matches(&inputs);
                (inputs, rows, matches)
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
        self.assignable = None;
        self.executing = self.target.as_ref().and_then(|target| {
            let row = self
                .catalogues
                .iter()
                .chain(&self.suggested)
                .find(|row| Some(&row.reference) == self.choice.as_ref())?;
            assignment_candidate(row, &preview.platform, &target.platform)
        });
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
                        Ok((inputs, rows, matches)) => {
                            let platform = self
                                .target
                                .as_ref()
                                .map(|t| t.platform.clone())
                                .unwrap_or_default();
                            self.suggested = suggested_catalogues(&rows, &matches, &platform);
                            self.catalogues = ready_catalogues(rows, &platform);
                            // Preselect only one unambiguous eligible choice.
                            self.choice = (self.catalogues.len() == 1 && self.suggested.is_empty())
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
                            let finding = finding(&totals);
                            self.assignable = assignable_after(&finding, self.executing.take());
                            Stage::Finished(Box::new(Finished { finding, totals }))
                        }
                        // Errors and cancellation never offer an assignment.
                        Err(error) => {
                            self.executing = None;
                            self.assignable = None;
                            Stage::Failed(error)
                        }
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

/// Why an installed catalogue may still not be offered here, in plain words.
/// The one-game check's start button (it opens the confirm step first).
pub(super) const CHECK_THIS_GAME: &str = "Check this game";
/// Verification never gates Play; this keeps that visible on every outcome.
pub(super) const NOT_NEEDED_TO_PLAY: &str = "Checking is optional. You do not need it to play: Play is on Game Details and runs its own launch check.";

/// Suggested catalogues: shown, never selected, never saved from here. The
/// engine refuses a several-system header until its system is chosen.
fn show_suggested(
    ui: &mut eframe::egui::Ui,
    suggested: &[InstalledCatalogueSummary],
    choice: &mut Option<CatalogueRef>,
    platform: &str,
) {
    if suggested.is_empty() {
        return;
    }
    ui.strong("Suggested — needs your confirmation");
    ui.small(suggested_reason(platform));
    for row in suggested {
        ui.radio_value(choice, Some(row.reference.clone()), &row.display_name);
    }
}

pub(super) fn suggested_reason(platform: &str) -> String {
    format!(
        "Its header says {platform}, but no system is saved for it. Choosing it tries it for this game only; nothing is saved to your setup unless this game is verified and you say Yes."
    )
}

pub(super) fn remember_question(name: &str, platform: &str) -> String {
    format!("Use {name} for {platform} from now on?")
}

pub(super) fn use_for_system_label(platform: &str) -> String {
    format!("Yes — use for {platform}")
}

pub(super) fn choose_for_system_label(platform: &str) -> String {
    format!("Choose a catalogue for {platform}")
}

/// Why a catalogue is (or is not) already chosen. Only a single eligible
/// catalogue is preselected; EmuWiz never ranks several against each other.
pub(super) fn catalogue_choice_note(count: usize, platform: &str) -> String {
    if count == 1 {
        format!("Selected for you: the only catalogue confirmed for {platform}.")
    } else {
        format!(
            "{count} ready catalogues cover {platform}. Choose one; EmuWiz does not guess between them."
        )
    }
}

pub(super) const MISSING_CATALOGUE_HELP: &str = "A catalogue is offered here only when it is enabled, has passed its catalogue check, is set to this game's system, and can check individual games (a folder of catalogues cannot). Choose a catalogue uses one you already installed. Set up verification opens DAT Management, where you can add a catalogue or choose its system. Having a catalogue installed does not verify any game.";

pub(super) enum PageAction {
    BackToGame,
    CheckWholePlatform,
    SetUpCatalogues,
    /// The person confirmed a suggested catalogue for this game's system.
    UseCatalogueForSystem {
        source_id: String,
        platform: String,
    },
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
            if let Some(result) = &state.assignment {
                match result {
                    Ok(message) => ui.colored_label(theme::SUCCESS, message),
                    Err(message) => ui.colored_label(theme::WARNING, message),
                };
            }
            if state.catalogues.is_empty() && state.suggested.is_empty() {
                ui.label(format!(
                    "No ready catalogue covers {}. Set one up first; nothing has been checked.",
                    target.platform
                ));
                ui.label(NOT_NEEDED_TO_PLAY);
                ui.label(MISSING_CATALOGUE_HELP);
                // Two different next steps, side by side: use a catalogue that
                // is already installed (asks before saving any system for it),
                // or open DAT Management to add one.
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button(choose_for_system_label(&target.platform))
                        .on_hover_text("Opens this system's check, which offers installed catalogues and asks before saving any system for them. Nothing is checked until you confirm.")
                        .clicked()
                    {
                        action = Some(PageAction::CheckWholePlatform);
                    }
                    if ui.button("Set up verification").clicked() {
                        action = Some(PageAction::SetUpCatalogues);
                    }
                });
            } else {
                if !state.catalogues.is_empty() {
                    ui.strong("Confirmed for this system");
                    ui.label(catalogue_choice_note(
                        state.catalogues.len(),
                        &target.platform,
                    ));
                }
                for row in &state.catalogues {
                    let label = match &row.revision {
                        Some(version) => format!("{} · version {version}", row.display_name),
                        None => row.display_name.clone(),
                    };
                    ui.radio_value(&mut state.choice, Some(row.reference.clone()), label);
                }
                show_suggested(ui, &state.suggested, &mut state.choice, &target.platform);
                let ready = state.choice.is_some();
                if ui
                    .add_enabled(ready, eframe::egui::Button::new(CHECK_THIS_GAME))
                    .clicked()
                {
                    state.preview();
                }
                ui.small("You see exactly what will be read first. Nothing is checked until you confirm.");
                ui.label(NOT_NEEDED_TO_PLAY);
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
                    "Needs review".to_string(),
                    "The file matches several catalogue entries, so no single identity was recorded. Review identity to choose the right one.".to_string(),
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
            if let Some(result) = &state.assignment {
                match result {
                    Ok(message) => ui.colored_label(theme::SUCCESS, message),
                    Err(message) => ui.colored_label(theme::WARNING, message),
                };
            }
            if let Some((source_id, platform, name)) = state.assignable.clone() {
                ui.group(|ui| {
                    ui.strong(remember_question(&name, &platform));
                    ui.label(format!(
                        "Its catalogue header identifies {platform} and the check you confirmed verified this game. Saving keeps it selected after a restart and lets launch checks use it. Your games are not changed."
                    ));
                    ui.horizontal(|ui| {
                        if ui.button(use_for_system_label(&platform)).clicked() {
                            action = Some(PageAction::UseCatalogueForSystem {
                                source_id: source_id.clone(),
                                platform: platform.clone(),
                            });
                        }
                        if ui.button("Not now").clicked() {
                            state.assignable = None;
                        }
                    });
                });
            }
            ui.label(NOT_NEEDED_TO_PLAY);
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
                eframe::egui::Button::new(format!(
                    "Check this system's games ({})",
                    target.platform
                )),
            )
            .on_hover_text("Opens the platform check, which reads every game on this system.")
            .clicked()
        {
            action = Some(PageAction::CheckWholePlatform);
        }
    });
    action
}
