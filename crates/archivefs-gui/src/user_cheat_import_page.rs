//! Read-only review of user-supplied RetroArch and PCSX2 cheat files.
//!
//! This page intentionally does not share state with CheatBase or the
//! emulator-specific installation workflows. The core importer is an index:
//! it reads bounded local files, reports provenance and matching evidence, and
//! never offers an install operation.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use archivefs_core::emulator_environment::HostReadOnlyFilesystem;
use archivefs_core::patch_manager::{
    CheatCandidateOptions, CheatDestinationRequest, CheatJourneyApplyApproval,
    CheatJourneyApplyOptions, CheatJourneyGameIdentity, CheatJourneyPreview,
    CheatJourneyPreviewAction, CheatJourneyUndoConfirmation, CheatJourneyUndoOptions,
    CheatJourneyUndoPreview, SharedApplyStatus, UserCheatCandidate, UserCheatDiagnostic,
    UserCheatFormat, UserCheatImportError, UserCheatImportReport, UserCheatLibraryGame,
    UserCheatMatchState, apply_cheat_journey, default_shared_backup_root,
    default_shared_history_root, discover_local_retroarch_cheat_file, generate_shared_operation_id,
    preview_cheat_journey, preview_cheat_journey_undo, scan_user_cheat_directory,
    scan_user_cheat_file, select_cheat_journey_candidate, undo_cheat_journey,
};
use eframe::egui;

use crate::ui::components as widgets;

/// The currently selected game's identity and resolved RetroArch cheat
/// destination, bound by the caller exactly as `build_cheat_candidate_request`
/// binds it for the trusted-catalogue journey - this page never derives
/// identity or a destination path on its own.
pub(crate) struct LocalCheatInstallContext {
    pub game: CheatJourneyGameIdentity,
    /// `None` when no eligible RetroArch profile with a resolved cheat
    /// directory is selected yet; the install action stays disabled with
    /// that exact reason rather than guessing a destination.
    pub destination: Option<CheatDestinationRequest>,
}

/// One local-file install attempt's current stage. Only one is ever active;
/// starting a new one (a different file, or "Try again") replaces it.
enum LocalInstallStage {
    Idle,
    Blocked {
        source_path: PathBuf,
        message: String,
    },
    Preview {
        source_path: PathBuf,
        catalogue_root: PathBuf,
        preview: Box<CheatJourneyPreview>,
    },
    Applied {
        destination_root: PathBuf,
        journal_path: Option<PathBuf>,
        transaction_id: String,
    },
    UndoPreview {
        destination_root: PathBuf,
        journal_path: PathBuf,
        transaction_id: String,
        preview: Box<CheatJourneyUndoPreview>,
    },
    Done {
        message: String,
    },
    Error {
        message: String,
    },
}

impl Default for LocalInstallStage {
    fn default() -> Self {
        Self::Idle
    }
}

#[derive(Debug)]
enum TaskResult {
    Scanned(Result<UserCheatImportReport, UserCheatImportError>),
}

#[derive(Debug, Default)]
enum ImportState {
    #[default]
    Idle,
    Scanning {
        source: PathBuf,
    },
    Ready {
        report: UserCheatImportReport,
    },
    Failed {
        source: PathBuf,
        message: String,
    },
}

/// GUI state for the user-supplied cheat import review card.
#[derive(Default)]
pub(crate) struct UserCheatImportPageState {
    state: ImportState,
    task: Option<(u64, Receiver<TaskResult>)>,
    generation: u64,
    context_key: Option<String>,
    report_context_key: Option<String>,
    last_source: Option<(PathBuf, bool)>,
    selected_candidate: Option<usize>,
    technical_details: bool,
    local_install: LocalInstallStage,
}

impl UserCheatImportPageState {
    fn poll(&mut self, context: &egui::Context) {
        let Some((generation, receiver)) = self.task.as_ref() else {
            return;
        };
        let generation = *generation;
        let result = match receiver.try_recv() {
            Ok(TaskResult::Scanned(result)) => result,
            Err(TryRecvError::Empty) => {
                context.request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
            Err(TryRecvError::Disconnected) => Err(UserCheatImportError::Io {
                path: PathBuf::new(),
                message: "The cheat import worker stopped unexpectedly.".to_string(),
            }),
        };
        self.task = None;
        if generation != self.generation {
            return;
        }
        let source = match &self.state {
            ImportState::Scanning { source } => source.clone(),
            _ => PathBuf::new(),
        };
        match result {
            Ok(report) => {
                self.report_context_key = self.context_key.clone();
                self.selected_candidate = None;
                self.state = ImportState::Ready { report };
            }
            Err(error) => {
                self.state = ImportState::Failed {
                    source,
                    message: error.to_string(),
                };
            }
        }
        context.request_repaint();
    }

    fn start_scan(
        &mut self,
        context: &egui::Context,
        source: PathBuf,
        is_directory: bool,
        library: Vec<UserCheatLibraryGame>,
    ) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let (sender, receiver) = mpsc::channel();
        self.task = Some((generation, receiver));
        self.last_source = Some((source.clone(), is_directory));
        self.state = ImportState::Scanning {
            source: source.clone(),
        };
        let context = context.clone();
        thread::spawn(move || {
            let result = if is_directory {
                scan_user_cheat_directory(&source, &library)
            } else {
                scan_user_cheat_file(&source, &library)
            };
            let _ = sender.send(TaskResult::Scanned(result));
            context.request_repaint();
        });
    }

    fn invalidate_if_context_changed(&mut self, context_key: Option<String>) {
        if self.context_key == context_key {
            return;
        }
        self.context_key = context_key;
        if self.report_context_key != self.context_key {
            self.generation = self.generation.wrapping_add(1);
            self.task = None;
        }
    }

    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        context: &egui::Context,
        library: &[UserCheatLibraryGame],
        selected_game: Option<(&str, &str)>,
        local_install_context: Option<&LocalCheatInstallContext>,
    ) {
        let context_key = selected_game.map(|(id, _)| id.to_string());
        self.invalidate_if_context_changed(context_key);
        self.poll(context);

        widgets::section_header(
            ui,
            "Your cheat files",
            Some("Review local .cht and .pnach files without changing emulator files."),
        );
        widgets::card(ui, |ui| {
            ui.label("Imported for review only. EmuWiz has not changed your emulator files.");
            if let Some((_, title)) = selected_game {
                widgets::status_badge(
                    ui,
                    format!("Selected game: {title}"),
                    widgets::StatusTone::Info,
                );
            }
            ui.horizontal_wrapped(|ui| {
                let can_start = self.task.is_none();
                if widgets::action_button(
                    ui,
                    "Add cheat file",
                    widgets::ActionStyle::Primary,
                    can_start,
                )
                .clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("Cheat files", &["cht", "pnach"])
                        .pick_file()
                {
                    self.start_scan(context, path, false, library.to_vec());
                }
                if widgets::action_button(
                    ui,
                    "Add cheat folder",
                    widgets::ActionStyle::Secondary,
                    can_start,
                )
                .clicked()
                    && let Some(path) = rfd::FileDialog::new().pick_folder()
                {
                    self.start_scan(context, path, true, library.to_vec());
                }
                if let Some((source, is_directory)) = self.last_source.clone()
                    && widgets::action_button(
                        ui,
                        "Scan again",
                        widgets::ActionStyle::Secondary,
                        can_start,
                    )
                    .clicked()
                {
                    self.start_scan(context, source, is_directory, library.to_vec());
                }
                if matches!(
                    &self.state,
                    ImportState::Ready { .. } | ImportState::Failed { .. }
                ) && widgets::action_button(
                    ui,
                    "Clear results",
                    widgets::ActionStyle::Quiet,
                    true,
                )
                .clicked()
                {
                    self.generation = self.generation.wrapping_add(1);
                    self.task = None;
                    self.state = ImportState::Idle;
                    self.report_context_key = None;
                    self.selected_candidate = None;
                }
            });

            let report = match &self.state {
                ImportState::Ready { report } => Some(report.clone()),
                _ => None,
            };
            match &self.state {
                ImportState::Idle => {
                    ui.label("Choose a cheat file or folder to scan it safely.");
                }
                ImportState::Scanning { source } => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Scanning {}…", source.display()));
                    });
                }
                ImportState::Failed { source, message } => {
                    widgets::banner(
                        ui,
                        "Cheat scan failed",
                        &format!("{}: {message}", source.display()),
                        widgets::StatusTone::Blocked,
                    );
                }
                ImportState::Ready { .. } => {
                    if let Some(report) = report.as_ref() {
                        self.show_report(ui, report, local_install_context);
                    }
                }
            }
        });
        self.show_local_install_panel(ui);
    }

    fn show_report(
        &mut self,
        ui: &mut egui::Ui,
        report: &UserCheatImportReport,
        local_install_context: Option<&LocalCheatInstallContext>,
    ) {
        if self.report_context_key != self.context_key {
            widgets::banner(
                ui,
                "Results need a new scan",
                "The selected game or library context changed. Scan again to refresh matching evidence.",
                widgets::StatusTone::Warning,
            );
            return;
        }
        let exact = report
            .candidates
            .iter()
            .filter(|c| c.match_state == UserCheatMatchState::Exact)
            .count();
        let strong = report
            .candidates
            .iter()
            .filter(|c| c.match_state == UserCheatMatchState::Strong)
            .count();
        let possible = report
            .candidates
            .iter()
            .filter(|c| c.match_state == UserCheatMatchState::Possible)
            .count();
        let ambiguous = report
            .candidates
            .iter()
            .filter(|c| c.match_state == UserCheatMatchState::Ambiguous)
            .count();
        let unmatched = report
            .candidates
            .iter()
            .filter(|c| c.match_state == UserCheatMatchState::NoMatch)
            .count();
        ui.label(format!(
            "Scanned {} file(s): {} supported, {} exact, {} strong, {} possible, {} ambiguous, {} unmatched.",
            report.files_visited, report.supported_files, exact, strong, possible, ambiguous, unmatched
        ));
        if report.truncated {
            widgets::banner(
                ui,
                "Scan limit reached",
                "The scan was safely truncated by its bounded file, byte, depth, or warning limits.",
                widgets::StatusTone::Warning,
            );
        }
        self.show_candidates(ui, report, "Matched", local_install_context, |candidate| {
            matches!(
                candidate.match_state,
                UserCheatMatchState::Exact | UserCheatMatchState::Strong
            )
        });
        self.show_candidates(ui, report, "Possible matches", None, |candidate| {
            candidate.match_state == UserCheatMatchState::Possible
        });
        self.show_candidates(ui, report, "Ambiguous matches", None, |candidate| {
            candidate.match_state == UserCheatMatchState::Ambiguous
        });
        self.show_candidates(ui, report, "Unmatched", None, |candidate| {
            candidate.match_state == UserCheatMatchState::NoMatch
        });
        self.show_candidates(ui, report, "Unsupported or rejected", None, |candidate| {
            candidate.match_state == UserCheatMatchState::Unsupported
        });
        if !report.duplicates.is_empty() {
            egui::CollapsingHeader::new(format!("Duplicate files ({})", report.duplicates.len()))
                .default_open(false)
                .show(ui, |ui| {
                    for duplicate in &report.duplicates {
                        ui.label(format!("Same SHA-256: {}", duplicate.source_sha256));
                        for path in &duplicate.paths {
                            ui.label(format!("  {}", path.display()));
                        }
                        ui.label("Duplicate file — both paths were retained for review.");
                    }
                });
        }
        if !report.diagnostics.is_empty() {
            egui::CollapsingHeader::new(format!("Diagnostics ({})", report.diagnostics.len()))
                .default_open(false)
                .show(ui, |ui| {
                    for diagnostic in &report.diagnostics {
                        ui.label(format_diagnostic(diagnostic));
                    }
                });
        }
        egui::CollapsingHeader::new("Technical details")
            .default_open(self.technical_details)
            .show(ui, |ui| {
                self.technical_details = true;
                ui.label(format!("Scan root: {}", report.scanned_root.display()));
                ui.label(format!("Files visited: {}", report.files_visited));
                ui.label(format!("Bytes read: {}", report.bytes_read));
                ui.label(format!("Read-only: {}", report.read_only));
                ui.label(format!("Writes performed: {}", report.writes_performed));
                ui.label(format!("Apply available: {}", report.apply_available));
            });
    }

    fn show_candidates<F>(
        &mut self,
        ui: &mut egui::Ui,
        report: &UserCheatImportReport,
        heading: &str,
        local_install_context: Option<&LocalCheatInstallContext>,
        filter: F,
    ) where
        F: Fn(&UserCheatCandidate) -> bool,
    {
        let indexes: Vec<usize> = report
            .candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| filter(c))
            .map(|(i, _)| i)
            .collect();
        if indexes.is_empty() {
            return;
        }
        egui::CollapsingHeader::new(format!("{heading} ({})", indexes.len()))
            .default_open(true)
            .show(ui, |ui| {
                for index in indexes {
                    let candidate = &report.candidates[index];
                    widgets::card(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(candidate.provenance.original_filename.as_str());
                            widgets::status_badge(ui, match_label(candidate.match_state), match_tone(candidate.match_state));
                            if ui.button(if self.selected_candidate == Some(index) { "Hide details" } else { "View details" }).clicked() {
                                self.selected_candidate = (self.selected_candidate != Some(index)).then_some(index);
                            }
                        });
                        ui.label(format!("{} cheat(s) found in {}.", candidate.cheat_count, format_label(candidate.format)));
                        ui.label(match_explanation(candidate.match_state));
                        if let Some(game) = candidate.matches.first() {
                            ui.label(format!("Matched game: {}", game.game_title));
                        }
                        if self.selected_candidate == Some(index) {
                            ui.label(format!("Source: {}", candidate.provenance.original_path.display()));
                            ui.label(format!("SHA-256: {}", candidate.provenance.source_sha256));
                            for warning in &candidate.parser_warnings { ui.label(format!("Warning: {warning}")); }
                            for game in &candidate.matches {
                                ui.label(format!("Evidence for {}: {}", game.game_title, evidence_text(&game.evidence)));
                            }
                            ui.label("Individual cheat names are not exposed by the current bounded import API.");
                            ui.label("No files were installed or changed.");
                        }
                        if candidate.format == UserCheatFormat::RetroarchCht {
                            self.show_install_action(ui, candidate, local_install_context);
                        } else {
                            ui.label("Local install is only available for RetroArch .cht files in this build; PCSX2 PNACH stays review-only.");
                        }
                    });
                }
            });
    }

    /// The install action for exactly one matched RetroArch `.cht`
    /// candidate. Everything downstream of the click - discovery, matching,
    /// preview, apply, and undo - is the same, unmodified `cheat_journey`
    /// pipeline the trusted-catalogue flow already uses; this only supplies
    /// the one file the user picked and the already-bound game identity.
    fn show_install_action(
        &mut self,
        ui: &mut egui::Ui,
        candidate: &UserCheatCandidate,
        local_install_context: Option<&LocalCheatInstallContext>,
    ) {
        let Some(install_context) = local_install_context else {
            ui.label("Select a game in Cheats & Mods to install this file.");
            return;
        };
        let Some(destination) = install_context.destination.as_ref() else {
            ui.label(
                "Select an eligible RetroArch profile with a resolved cheat directory (Stage 1) before installing a local file.",
            );
            return;
        };
        let path = candidate.provenance.original_path.clone();
        if ui.button("Install this cheat file").clicked() {
            self.start_local_install(path, &install_context.game, destination);
        }
    }

    fn start_local_install(
        &mut self,
        source_path: PathBuf,
        game: &CheatJourneyGameIdentity,
        destination: &CheatDestinationRequest,
    ) {
        let found = match discover_local_retroarch_cheat_file(
            &HostReadOnlyFilesystem,
            &source_path,
            game,
            &CheatCandidateOptions::default(),
        ) {
            Ok(found) => found,
            Err(error) => {
                self.local_install = LocalInstallStage::Blocked {
                    source_path,
                    message: error.to_string(),
                };
                return;
            }
        };
        let Some(candidate) = found.candidate() else {
            self.local_install = LocalInstallStage::Blocked {
                source_path,
                message: "This file could not be read back as a supported cheat file.".to_string(),
            };
            return;
        };
        if !candidate.manually_selectable {
            let evidence = candidate
                .evidence
                .iter()
                .map(|entry| entry.detail.clone())
                .collect::<Vec<_>>()
                .join("; ");
            self.local_install = LocalInstallStage::Blocked {
                source_path,
                message: format!(
                    "Not installable for the selected game ({:?}). {evidence}",
                    candidate.classification
                ),
            };
            return;
        }
        let mut selection = match select_cheat_journey_candidate(
            &found.discovery,
            &found.location.catalogue_root,
            &found.location.catalogue_relative_path,
        ) {
            Ok(selection) => selection,
            Err(error) => {
                self.local_install = LocalInstallStage::Blocked {
                    source_path,
                    message: error.to_string(),
                };
                return;
            }
        };
        selection.cheat_selection.select_all();
        if selection.cheat_selection.selected_count() == 0 {
            self.local_install = LocalInstallStage::Blocked {
                source_path,
                message: "This file has no cheats that can be safely selected.".to_string(),
            };
            return;
        }
        match preview_cheat_journey(
            &selection,
            &found.location.catalogue_root,
            destination.clone(),
            "retroarch-main",
            "local file",
        ) {
            Ok(preview) => {
                self.local_install = LocalInstallStage::Preview {
                    source_path,
                    catalogue_root: found.location.catalogue_root,
                    preview: Box::new(preview),
                };
            }
            Err(error) => {
                self.local_install = LocalInstallStage::Blocked {
                    source_path,
                    message: error.to_string(),
                };
            }
        }
    }

    fn confirm_local_apply(&mut self) {
        let LocalInstallStage::Preview {
            catalogue_root,
            preview,
            ..
        } = std::mem::take(&mut self.local_install)
        else {
            return;
        };
        let history_root = match default_shared_history_root() {
            Ok(root) => root,
            Err(error) => {
                self.local_install = LocalInstallStage::Error {
                    message: format!("History root unavailable: {}", error.detail),
                };
                return;
            }
        };
        let backup_root = match default_shared_backup_root() {
            Ok(root) => root,
            Err(error) => {
                self.local_install = LocalInstallStage::Error {
                    message: format!("Backup root unavailable: {}", error.detail),
                };
                return;
            }
        };
        let staging_root = match crate::default_generated_cheat_staging_root() {
            Ok(root) => root,
            Err(message) => {
                self.local_install = LocalInstallStage::Error { message };
                return;
            }
        };
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let destination_root = preview.destination_request.profile_cheat_root.clone();
        match apply_cheat_journey(
            &preview,
            &catalogue_root,
            &CheatJourneyApplyApproval {
                preview_id: preview.preview_id.clone(),
                approved: true,
                replacement_approved: matches!(
                    preview.action,
                    CheatJourneyPreviewAction::ReplaceExisting
                ),
            },
            &CheatJourneyApplyOptions {
                staging_root,
                operation_id: generate_shared_operation_id(),
                timestamp_unix_seconds: timestamp,
                history_root,
                backup_root,
            },
        ) {
            Ok(applied) => {
                self.local_install = LocalInstallStage::Applied {
                    destination_root,
                    journal_path: applied.result.journal_path,
                    transaction_id: applied.transaction_id,
                };
            }
            Err(error) => {
                self.local_install = LocalInstallStage::Error {
                    message: error.to_string(),
                };
            }
        }
    }

    fn start_local_undo(&mut self) {
        let LocalInstallStage::Applied {
            destination_root,
            journal_path,
            transaction_id,
        } = std::mem::take(&mut self.local_install)
        else {
            return;
        };
        let Some(journal_path) = journal_path else {
            self.local_install = LocalInstallStage::Error {
                message: "No transaction journal was recorded for this apply.".to_string(),
            };
            return;
        };
        let backup_root = match default_shared_backup_root() {
            Ok(root) => root,
            Err(error) => {
                self.local_install = LocalInstallStage::Error {
                    message: format!("Backup root unavailable: {}", error.detail),
                };
                return;
            }
        };
        let preview = preview_cheat_journey_undo(
            &transaction_id,
            &journal_path,
            &destination_root,
            &backup_root,
        );
        self.local_install = LocalInstallStage::UndoPreview {
            destination_root,
            journal_path,
            transaction_id,
            preview: Box::new(preview),
        };
    }

    fn confirm_local_undo(&mut self) {
        let LocalInstallStage::UndoPreview { preview, .. } =
            std::mem::take(&mut self.local_install)
        else {
            return;
        };
        let history_root = match default_shared_history_root() {
            Ok(root) => root,
            Err(error) => {
                self.local_install = LocalInstallStage::Error {
                    message: format!("History root unavailable: {}", error.detail),
                };
                return;
            }
        };
        let backup_root = match default_shared_backup_root() {
            Ok(root) => root,
            Err(error) => {
                self.local_install = LocalInstallStage::Error {
                    message: format!("Backup root unavailable: {}", error.detail),
                };
                return;
            }
        };
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        match undo_cheat_journey(
            &preview,
            &CheatJourneyUndoOptions {
                confirmation: CheatJourneyUndoConfirmation {
                    preview_id: preview.preview.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: generate_shared_operation_id(),
                timestamp_unix_seconds: timestamp,
                history_root,
                backup_root,
            },
        ) {
            Ok(result) if result.status == SharedApplyStatus::Success => {
                self.local_install = LocalInstallStage::Done {
                    message:
                        "The installed cheat file was removed and the prior state was restored."
                            .to_string(),
                };
            }
            Ok(result) => {
                self.local_install = LocalInstallStage::Error {
                    message: format!("Undo did not fully succeed: {:?}", result.status),
                };
            }
            Err(error) => {
                self.local_install = LocalInstallStage::Error {
                    message: error.to_string(),
                };
            }
        }
    }

    /// Takes ownership of the current stage before rendering it so the
    /// action buttons below (which need `&mut self` to advance the state
    /// machine) never conflict with a live borrow of `self.local_install`.
    /// Every arm puts a stage back before returning.
    fn show_local_install_panel(&mut self, ui: &mut egui::Ui) {
        let stage = std::mem::take(&mut self.local_install);
        match stage {
            LocalInstallStage::Idle => {}
            LocalInstallStage::Blocked {
                source_path,
                message,
            } => {
                ui.add_space(theme_gap());
                widgets::banner(
                    ui,
                    &format!("Cannot install {}", source_path.display()),
                    &message,
                    widgets::StatusTone::Blocked,
                );
                if ui.button("Dismiss").clicked() {
                    self.local_install = LocalInstallStage::Idle;
                } else {
                    self.local_install = LocalInstallStage::Blocked {
                        source_path,
                        message,
                    };
                }
            }
            LocalInstallStage::Preview {
                source_path,
                catalogue_root,
                preview,
            } => {
                ui.add_space(theme_gap());
                let mut confirmed = false;
                let mut cancelled = false;
                widgets::card(ui, |ui| {
                    ui.strong("Review before installing");
                    ui.label(format!("Source file: {}", source_path.display()));
                    ui.label(format!(
                        "Destination: {}",
                        preview.destination.path.display()
                    ));
                    ui.label(match preview.action {
                        CheatJourneyPreviewAction::InstallNew => {
                            "This will create a new cheat file at the destination above.".to_string()
                        }
                        CheatJourneyPreviewAction::AlreadyInstalled => {
                            "The exact same content is already installed at this destination - installing again changes nothing.".to_string()
                        }
                        CheatJourneyPreviewAction::ReplaceExisting => {
                            "A different cheat file already exists at this destination. Installing will back it up and replace it.".to_string()
                        }
                    });
                    egui::CollapsingHeader::new("Parsed contents to be written")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.monospace(preview.rendered_contents.as_str());
                        });
                    ui.horizontal(|ui| {
                        if widgets::action_button(
                            ui,
                            "Confirm install",
                            widgets::ActionStyle::Primary,
                            true,
                        )
                        .clicked()
                        {
                            confirmed = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancelled = true;
                        }
                    });
                });
                if confirmed {
                    self.local_install = LocalInstallStage::Preview {
                        source_path,
                        catalogue_root,
                        preview,
                    };
                    self.confirm_local_apply();
                } else if cancelled {
                    self.local_install = LocalInstallStage::Idle;
                } else {
                    self.local_install = LocalInstallStage::Preview {
                        source_path,
                        catalogue_root,
                        preview,
                    };
                }
            }
            LocalInstallStage::Applied {
                destination_root,
                journal_path,
                transaction_id,
            } => {
                ui.add_space(theme_gap());
                let mut undo = false;
                let mut dismissed = false;
                widgets::card(ui, |ui| {
                    widgets::status_badge(ui, "Installed", widgets::StatusTone::Success);
                    if let Some(journal_path) = journal_path.as_ref() {
                        ui.label(format!("Transaction journal: {}", journal_path.display()));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Undo this install").clicked() {
                            undo = true;
                        }
                        if ui.button("Dismiss").clicked() {
                            dismissed = true;
                        }
                    });
                });
                if undo {
                    self.local_install = LocalInstallStage::Applied {
                        destination_root,
                        journal_path,
                        transaction_id,
                    };
                    self.start_local_undo();
                } else if dismissed {
                    self.local_install = LocalInstallStage::Idle;
                } else {
                    self.local_install = LocalInstallStage::Applied {
                        destination_root,
                        journal_path,
                        transaction_id,
                    };
                }
            }
            LocalInstallStage::UndoPreview {
                destination_root,
                journal_path,
                transaction_id,
                preview,
            } => {
                ui.add_space(theme_gap());
                let mut confirmed = false;
                let mut cancelled = false;
                widgets::card(ui, |ui| {
                    ui.strong("Confirm undo");
                    ui.label("This will remove the installed cheat file and restore any prior file that was backed up.");
                    ui.horizontal(|ui| {
                        if widgets::action_button(
                            ui,
                            "Confirm undo",
                            widgets::ActionStyle::Primary,
                            true,
                        )
                        .clicked()
                        {
                            confirmed = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancelled = true;
                        }
                    });
                });
                if confirmed {
                    self.local_install = LocalInstallStage::UndoPreview {
                        destination_root,
                        journal_path,
                        transaction_id,
                        preview,
                    };
                    self.confirm_local_undo();
                } else if cancelled {
                    self.local_install = LocalInstallStage::Applied {
                        destination_root,
                        journal_path: Some(journal_path),
                        transaction_id,
                    };
                } else {
                    self.local_install = LocalInstallStage::UndoPreview {
                        destination_root,
                        journal_path,
                        transaction_id,
                        preview,
                    };
                }
            }
            LocalInstallStage::Done { message } => {
                ui.add_space(theme_gap());
                widgets::banner(ui, "Undo complete", &message, widgets::StatusTone::Info);
                if ui.button("Dismiss").clicked() {
                    self.local_install = LocalInstallStage::Idle;
                } else {
                    self.local_install = LocalInstallStage::Done { message };
                }
            }
            LocalInstallStage::Error { message } => {
                ui.add_space(theme_gap());
                widgets::banner(
                    ui,
                    "Local cheat install error",
                    &message,
                    widgets::StatusTone::Blocked,
                );
                if ui.button("Dismiss").clicked() {
                    self.local_install = LocalInstallStage::Idle;
                } else {
                    self.local_install = LocalInstallStage::Error { message };
                }
            }
        }
    }
}

fn theme_gap() -> f32 {
    crate::ui::theme::SECTION_GAP
}

fn format_label(format: UserCheatFormat) -> &'static str {
    match format {
        UserCheatFormat::RetroarchCht => "RetroArch .cht",
        UserCheatFormat::Pcsx2Pnach => "PCSX2 .pnach",
    }
}

fn match_label(state: UserCheatMatchState) -> &'static str {
    match state {
        UserCheatMatchState::Exact => "Exact match",
        UserCheatMatchState::Strong => "Strong match",
        UserCheatMatchState::Possible => "Possible match",
        UserCheatMatchState::Ambiguous => "Review matches",
        UserCheatMatchState::Unsupported => "Not imported",
        UserCheatMatchState::NoMatch => "No match",
    }
}

fn match_explanation(state: UserCheatMatchState) -> &'static str {
    match state {
        UserCheatMatchState::Exact => {
            "Exact match — this file matches a game using strong identity evidence."
        }
        UserCheatMatchState::Strong => {
            "Strong match — the game details match, but an exact file identity was not confirmed."
        }
        UserCheatMatchState::Possible => {
            "Possible match — some details fit. Review the evidence before using this cheat."
        }
        UserCheatMatchState::Ambiguous => {
            "EmuWiz found more than one possible game. Review the matches; it will not guess."
        }
        UserCheatMatchState::Unsupported => {
            "This file was not imported because its format is not supported. Supported formats are RetroArch .cht and PCSX2 .pnach."
        }
        UserCheatMatchState::NoMatch => "No matching game was found in your library.",
    }
}

fn match_tone(state: UserCheatMatchState) -> widgets::StatusTone {
    match state {
        UserCheatMatchState::Exact => widgets::StatusTone::Success,
        UserCheatMatchState::Strong => widgets::StatusTone::Info,
        UserCheatMatchState::Possible | UserCheatMatchState::Ambiguous => {
            widgets::StatusTone::Warning
        }
        UserCheatMatchState::Unsupported | UserCheatMatchState::NoMatch => {
            widgets::StatusTone::Pending
        }
    }
}

fn evidence_text(evidence: &[archivefs_core::patch_manager::UserCheatEvidence]) -> String {
    if evidence.is_empty() {
        "No matching evidence reported.".to_string()
    } else {
        evidence
            .iter()
            .map(|item| format!("{}={}", item.kind, item.value))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn format_diagnostic(diagnostic: &UserCheatDiagnostic) -> String {
    format!("{}: {}", diagnostic.path.display(), diagnostic.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wording_preserves_backend_confidence() {
        assert!(match_explanation(UserCheatMatchState::Exact).starts_with("Exact match"));
        assert!(match_explanation(UserCheatMatchState::Strong).contains("not confirmed"));
        assert!(match_explanation(UserCheatMatchState::Possible).contains("some details"));
        assert!(match_explanation(UserCheatMatchState::Ambiguous).contains("will not guess"));
    }

    #[test]
    fn unsupported_and_unmatched_are_separate() {
        assert_ne!(
            match_label(UserCheatMatchState::Unsupported),
            match_label(UserCheatMatchState::NoMatch)
        );
        assert!(match_explanation(UserCheatMatchState::Unsupported).contains("not supported"));
        assert!(match_explanation(UserCheatMatchState::NoMatch).contains("No matching game"));
    }

    #[test]
    fn no_mutating_action_is_named_by_the_review_surface() {
        for state in [
            UserCheatMatchState::Exact,
            UserCheatMatchState::Strong,
            UserCheatMatchState::Possible,
            UserCheatMatchState::Ambiguous,
            UserCheatMatchState::NoMatch,
        ] {
            assert!(!match_explanation(state).contains("Apply"));
            assert!(!match_explanation(state).contains("Install"));
        }
    }

    #[test]
    fn formats_are_the_two_backend_formats() {
        assert_eq!(
            format_label(UserCheatFormat::RetroarchCht),
            "RetroArch .cht"
        );
        assert_eq!(format_label(UserCheatFormat::Pcsx2Pnach), "PCSX2 .pnach");
    }
}
