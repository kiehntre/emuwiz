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
    CheatCandidateOptions, CheatDestinationRequest, CheatDocument, CheatIssue,
    CheatJourneyApplyApproval, CheatJourneyApplyOptions, CheatJourneyGameIdentity,
    CheatJourneyPreview, CheatJourneyPreviewAction, CheatJourneyUndoConfirmation,
    CheatJourneyUndoOptions, CheatJourneyUndoPreview, CheatOperation, CheatPlatform,
    CheatSourceFormat, CheatTargetFormat, ConversionCapability, DolphinCandidate,
    DolphinInstallPreview, DolphinInstallPreviewRequest, LocalDolphinInstallState,
    LocalPcsx2InstallState, LocalXeniaInstallState, Pcsx2GameIdentity, Pcsx2InstallPreview,
    Pcsx2InstallPreviewRequest, Pcsx2Profile, PreviewProposedAction, SharedApplyConfirmation,
    SharedApplyOptions, SharedApplyStatus, SharedRollbackConfirmation, SharedRollbackOptions,
    SharedRollbackPreview, UserCheatCandidate, UserCheatDiagnostic, UserCheatFormat,
    UserCheatImportError, UserCheatImportReport, UserCheatLibraryGame, UserCheatMatchState,
    XeniaInstallPreview, XeniaInstallPreviewRequest, XeniaProfile, apply_cheat_journey,
    build_dolphin_install_preview, build_pcsx2_install_preview, build_shared_transaction_plan,
    build_xenia_install_preview, check_local_dolphin_install_state,
    check_local_pcsx2_install_state, check_local_xenia_install_state, convert_cheat_document,
    default_shared_backup_root, default_shared_history_root, discover_local_dolphin_cheat_file,
    discover_local_pcsx2_pnach_file, discover_local_retroarch_cheat_file,
    discover_local_xenia_patch_file, execute_shared_apply, execute_shared_rollback,
    generate_shared_operation_id, load_dolphin_destination, load_local_xenia_destination,
    preview_cheat_journey, preview_cheat_journey_undo, preview_shared_rollback,
    scan_user_cheat_directory, scan_user_cheat_file, select_cheat_journey_candidate,
    stage_local_dolphin_codes, stage_local_xenia_patch_file, stage_pcsx2_pnach,
    supported_targets_for, undo_cheat_journey,
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

/// The currently selected game's PCSX2 identity and resolved profile,
/// bound by the caller exactly as `pcsx2_identity_for_workflow` already
/// binds it for the GameHacking-catalogue journey.
pub(crate) struct LocalPcsx2InstallContext {
    pub identity: Pcsx2GameIdentity,
    /// `None` when no eligible PCSX2 profile is selected yet; the install
    /// action stays disabled with that exact reason rather than guessing
    /// a destination.
    pub profile: Option<Pcsx2Profile>,
}

/// One local `.pnach` install attempt's current stage, independent of
/// `LocalInstallStage` (RetroArch) so the two formats never share state.
enum LocalPcsx2InstallStage {
    Idle,
    Blocked {
        source_path: PathBuf,
        message: String,
    },
    AlreadyInstalled {
        source_path: PathBuf,
    },
    Preview {
        source_path: PathBuf,
        profile: Pcsx2Profile,
        preview: Box<Pcsx2InstallPreview>,
    },
    Applied {
        destination_root: PathBuf,
        journal_path: Option<PathBuf>,
    },
    UndoPreview {
        destination_root: PathBuf,
        journal_path: PathBuf,
        preview: Box<SharedRollbackPreview>,
    },
    Done {
        message: String,
    },
    Error {
        message: String,
    },
}

impl Default for LocalPcsx2InstallStage {
    fn default() -> Self {
        Self::Idle
    }
}

/// The currently selected game's already-resolved Dolphin candidate
/// (exact game ID, and when applicable exact disc revision, already
/// matched against the selected profile) plus the profile's own
/// configuration root - bound by the caller exactly as the existing
/// Dolphin Gecko provider workflow binds them, never re-derived here.
pub(crate) struct LocalDolphinInstallContext {
    pub candidate: DolphinCandidate,
    pub configuration_path: PathBuf,
    /// The selected Dolphin profile's own ID, exactly the string
    /// `build_shared_transaction_plan` already scopes every other Dolphin
    /// apply's journal/backup entries under.
    pub profile_id: String,
}

/// Xenia identity/profile binding supplied by Cheats & Mods. Optional fields
/// let the UI explain unresolved or ambiguous state without guessing.
pub(crate) struct LocalXeniaInstallContext {
    pub title_id: Option<String>,
    pub profile: Option<XeniaProfile>,
}

enum LocalXeniaInstallStage {
    Idle,
    Blocked {
        source_path: PathBuf,
        message: String,
    },
    AlreadyInstalled {
        source_path: PathBuf,
    },
    Preview {
        source_path: PathBuf,
        profile_id: String,
        configuration_path: PathBuf,
        preview: Box<XeniaInstallPreview>,
    },
    Applied {
        destination_root: PathBuf,
        journal_path: Option<PathBuf>,
    },
    UndoPreview {
        destination_root: PathBuf,
        preview: Box<SharedRollbackPreview>,
    },
    Done {
        message: String,
    },
    Error {
        message: String,
    },
}

impl Default for LocalXeniaInstallStage {
    fn default() -> Self {
        Self::Idle
    }
}

/// One local Dolphin `.ini` (Gecko/Action Replay) install attempt's
/// current stage, independent of the RetroArch/PCSX2 stages so the three
/// formats never share state. Unlike RetroArch/PCSX2, there is no
/// directory-wide scan to pick a candidate from - the user picks the file
/// directly, so this starts from a file path rather than a report row.
enum LocalDolphinInstallStage {
    Idle,
    Blocked {
        source_path: PathBuf,
        message: String,
    },
    AlreadyInstalled {
        source_path: PathBuf,
    },
    Preview {
        source_path: PathBuf,
        configuration_path: PathBuf,
        profile_id: String,
        preview: Box<DolphinInstallPreview>,
    },
    Applied {
        destination_root: PathBuf,
        journal_path: Option<PathBuf>,
    },
    UndoPreview {
        destination_root: PathBuf,
        journal_path: PathBuf,
        preview: Box<SharedRollbackPreview>,
    },
    Done {
        message: String,
    },
    Error {
        message: String,
    },
}

impl Default for LocalDolphinInstallStage {
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
    /// Candidate whose conversion capabilities are being reviewed.  The
    /// converter is deliberately preview-only; native install paths below
    /// remain separate and authoritative.
    converter_candidate: Option<usize>,
    technical_details: bool,
    local_install: LocalInstallStage,
    local_pcsx2_install: LocalPcsx2InstallStage,
    local_dolphin_install: LocalDolphinInstallStage,
    local_xenia_install: LocalXeniaInstallStage,
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
        local_pcsx2_install_context: Option<&LocalPcsx2InstallContext>,
        local_dolphin_install_context: Option<&LocalDolphinInstallContext>,
        local_xenia_install_context: Option<&LocalXeniaInstallContext>,
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
            ui.label(
                "Local install is available for RetroArch .cht, PCSX2 .pnach, Dolphin \
                 Gecko/Action Replay .ini files, and Xenia .patch.toml files.",
            );
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
                        self.show_report(
                            ui,
                            report,
                            local_install_context,
                            local_pcsx2_install_context,
                        );
                    }
                }
            }
            self.show_dolphin_local_install_picker(ui, context, local_dolphin_install_context);
        });
        self.show_local_install_panel(ui);
        self.show_local_pcsx2_install_panel(ui);
        self.show_local_dolphin_install_panel(ui);
        self.show_local_xenia_install_picker(ui, local_xenia_install_context);
        self.show_local_xenia_install_panel(ui);
    }

    /// Dolphin's local-file entry point: unlike RetroArch/PCSX2, Dolphin
    /// cheat files are never scanned or matched from a directory listing
    /// here - the file the user picks *is* the one installed, bound to the
    /// already-resolved [`LocalDolphinInstallContext`] for the selected
    /// game. A dedicated `.ini` file picker keeps this fully independent
    /// of the generic multi-format import review above.
    fn show_dolphin_local_install_picker(
        &mut self,
        ui: &mut egui::Ui,
        _context: &egui::Context,
        local_dolphin_install_context: Option<&LocalDolphinInstallContext>,
    ) {
        ui.add_space(theme_gap());
        widgets::card(ui, |ui| {
            ui.strong("Install a local Dolphin cheat file (.ini)");
            ui.label(
                "The file must declare a [Gecko] and/or [ActionReplay] section, exactly like \
                 a Dolphin GameSettings file - the same sections Dolphin itself reads.",
            );
            let Some(install_context) = local_dolphin_install_context else {
                ui.label("Select a Dolphin game and an eligible profile in Cheats & Mods to install a local file.");
                return;
            };
            let can_start = matches!(self.local_dolphin_install, LocalDolphinInstallStage::Idle);
            if widgets::action_button(
                ui,
                "Choose a Dolphin cheat file…",
                widgets::ActionStyle::Primary,
                can_start,
            )
            .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Dolphin GameSettings", &["ini"])
                    .pick_file()
            {
                self.start_local_dolphin_install(
                    path,
                    install_context.candidate.clone(),
                    install_context.configuration_path.clone(),
                    install_context.profile_id.clone(),
                );
            }
        });
    }

    fn show_report(
        &mut self,
        ui: &mut egui::Ui,
        report: &UserCheatImportReport,
        local_install_context: Option<&LocalCheatInstallContext>,
        local_pcsx2_install_context: Option<&LocalPcsx2InstallContext>,
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
        self.show_candidates(
            ui,
            report,
            "Matched",
            local_install_context,
            local_pcsx2_install_context,
            |candidate| {
                matches!(
                    candidate.match_state,
                    UserCheatMatchState::Exact | UserCheatMatchState::Strong
                )
            },
        );
        self.show_candidates(ui, report, "Possible matches", None, None, |candidate| {
            candidate.match_state == UserCheatMatchState::Possible
        });
        self.show_candidates(ui, report, "Ambiguous matches", None, None, |candidate| {
            candidate.match_state == UserCheatMatchState::Ambiguous
        });
        self.show_candidates(ui, report, "Unmatched", None, None, |candidate| {
            candidate.match_state == UserCheatMatchState::NoMatch
        });
        self.show_candidates(
            ui,
            report,
            "Unsupported or rejected",
            None,
            None,
            |candidate| candidate.match_state == UserCheatMatchState::Unsupported,
        );
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
        local_pcsx2_install_context: Option<&LocalPcsx2InstallContext>,
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
                        self.show_conversion_preview(ui, candidate, index);
                        match candidate.format {
                            UserCheatFormat::RetroarchCht => {
                                self.show_install_action(ui, candidate, local_install_context);
                            }
                            UserCheatFormat::Pcsx2Pnach => {
                                self.show_pcsx2_install_action(
                                    ui,
                                    candidate,
                                    local_pcsx2_install_context,
                                );
                            }
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

    /// The install action for exactly one matched PCSX2 `.pnach`
    /// candidate. Everything downstream of the click - staging, preview,
    /// apply, and rollback - is the same, unmodified PCSX2 install-plan
    /// pipeline the GameHacking-catalogue flow already uses; this only
    /// supplies the one file the user picked, resolved into the single
    /// managed cheat it becomes.
    fn show_pcsx2_install_action(
        &mut self,
        ui: &mut egui::Ui,
        candidate: &UserCheatCandidate,
        local_pcsx2_install_context: Option<&LocalPcsx2InstallContext>,
    ) {
        let Some(install_context) = local_pcsx2_install_context else {
            ui.label("Select a PCSX2 game in Cheats & Mods to install this file.");
            return;
        };
        let Some(profile) = install_context.profile.as_ref() else {
            ui.label("Select an eligible PCSX2 profile (Stage 1) before installing a local file.");
            return;
        };
        let path = candidate.provenance.original_path.clone();
        if ui.button("Install this cheat file").clicked() {
            self.start_local_pcsx2_install(path, &install_context.identity, profile.clone());
        }
    }

    fn start_local_pcsx2_install(
        &mut self,
        source_path: PathBuf,
        identity: &Pcsx2GameIdentity,
        profile: Pcsx2Profile,
    ) {
        let discovery = match discover_local_pcsx2_pnach_file(&source_path, identity) {
            Ok(discovery) => discovery,
            Err(error) => {
                self.local_pcsx2_install = LocalPcsx2InstallStage::Blocked {
                    source_path,
                    message: error.to_string(),
                };
                return;
            }
        };
        match check_local_pcsx2_install_state(&profile, &discovery) {
            Ok(LocalPcsx2InstallState::AlreadyInstalled) => {
                self.local_pcsx2_install = LocalPcsx2InstallStage::AlreadyInstalled { source_path };
                return;
            }
            Ok(LocalPcsx2InstallState::New) => {}
            Err(error) => {
                self.local_pcsx2_install = LocalPcsx2InstallStage::Blocked {
                    source_path,
                    message: error.to_string(),
                };
                return;
            }
        }
        let staging_root = match crate::default_generated_pcsx2_local_staging_root() {
            Ok(root) => root,
            Err(message) => {
                self.local_pcsx2_install = LocalPcsx2InstallStage::Error { message };
                return;
            }
        };
        let staged = match stage_pcsx2_pnach(
            &staging_root,
            &profile,
            discovery.detected_serial.as_deref(),
            &discovery.detected_crc,
            std::slice::from_ref(&discovery.cheat),
        ) {
            Ok(staged) => staged,
            Err(error) => {
                self.local_pcsx2_install = LocalPcsx2InstallStage::Blocked {
                    source_path,
                    message: error.to_string(),
                };
                return;
            }
        };
        match build_pcsx2_install_preview(&Pcsx2InstallPreviewRequest {
            selected_archive: identity.archive_path.clone(),
            profile: profile.clone(),
            identity: identity.clone(),
            staged,
        }) {
            Ok(preview) => {
                self.local_pcsx2_install = LocalPcsx2InstallStage::Preview {
                    source_path,
                    profile,
                    preview: Box::new(preview),
                };
            }
            Err(error) => {
                self.local_pcsx2_install = LocalPcsx2InstallStage::Blocked {
                    source_path,
                    message: error.to_string(),
                };
            }
        }
    }

    fn confirm_local_pcsx2_apply(&mut self) {
        let LocalPcsx2InstallStage::Preview {
            profile, preview, ..
        } = std::mem::take(&mut self.local_pcsx2_install)
        else {
            return;
        };
        let history_root = match default_shared_history_root() {
            Ok(root) => root,
            Err(error) => {
                self.local_pcsx2_install = LocalPcsx2InstallStage::Error {
                    message: format!("History root unavailable: {}", error.detail),
                };
                return;
            }
        };
        let backup_root = match default_shared_backup_root() {
            Ok(root) => root,
            Err(error) => {
                self.local_pcsx2_install = LocalPcsx2InstallStage::Error {
                    message: format!("Backup root unavailable: {}", error.detail),
                };
                return;
            }
        };
        let replacement_required = preview
            .report
            .entries
            .iter()
            .any(|entry| entry.proposed_action == PreviewProposedAction::Replace);
        let plan = match build_shared_transaction_plan(
            &preview.report,
            &profile.profile_id,
            "pcsx2-local-file",
            &preview.staged.staging_root,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                self.local_pcsx2_install = LocalPcsx2InstallStage::Error {
                    message: error.detail,
                };
                return;
            }
        };
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let destination_root = profile.configuration_path.clone();
        let result = execute_shared_apply(
            &plan,
            &SharedApplyOptions {
                dry_run: false,
                confirmation: Some(SharedApplyConfirmation {
                    plan_id: plan.plan_id.clone(),
                    general_approved: true,
                    replacement_approved: replacement_required,
                }),
                operation_id: generate_shared_operation_id(),
                timestamp_unix_seconds: timestamp,
                current_context: plan.context.clone(),
                history_root,
                backup_root,
            },
        );
        if result.journal.status == SharedApplyStatus::Success {
            self.local_pcsx2_install = LocalPcsx2InstallStage::Applied {
                destination_root,
                journal_path: result.journal_path,
            };
        } else {
            self.local_pcsx2_install = LocalPcsx2InstallStage::Error {
                message: format!("Apply did not fully succeed: {:?}", result.journal.status),
            };
        }
    }

    fn start_local_pcsx2_undo(&mut self) {
        let LocalPcsx2InstallStage::Applied {
            destination_root,
            journal_path,
        } = std::mem::take(&mut self.local_pcsx2_install)
        else {
            return;
        };
        let Some(journal_path) = journal_path else {
            self.local_pcsx2_install = LocalPcsx2InstallStage::Error {
                message: "No transaction journal was recorded for this apply.".to_string(),
            };
            return;
        };
        let backup_root = match default_shared_backup_root() {
            Ok(root) => root,
            Err(error) => {
                self.local_pcsx2_install = LocalPcsx2InstallStage::Error {
                    message: format!("Backup root unavailable: {}", error.detail),
                };
                return;
            }
        };
        let preview = preview_shared_rollback(&journal_path, &destination_root, &backup_root);
        self.local_pcsx2_install = LocalPcsx2InstallStage::UndoPreview {
            destination_root,
            journal_path,
            preview: Box::new(preview),
        };
    }

    fn confirm_local_pcsx2_undo(&mut self) {
        let LocalPcsx2InstallStage::UndoPreview { preview, .. } =
            std::mem::take(&mut self.local_pcsx2_install)
        else {
            return;
        };
        let history_root = match default_shared_history_root() {
            Ok(root) => root,
            Err(error) => {
                self.local_pcsx2_install = LocalPcsx2InstallStage::Error {
                    message: format!("History root unavailable: {}", error.detail),
                };
                return;
            }
        };
        let backup_root = match default_shared_backup_root() {
            Ok(root) => root,
            Err(error) => {
                self.local_pcsx2_install = LocalPcsx2InstallStage::Error {
                    message: format!("Backup root unavailable: {}", error.detail),
                };
                return;
            }
        };
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let result = execute_shared_rollback(
            &preview,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: preview.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: generate_shared_operation_id(),
                timestamp_unix_seconds: timestamp,
                history_root,
                backup_root,
            },
        );
        if result.status == SharedApplyStatus::Success {
            self.local_pcsx2_install = LocalPcsx2InstallStage::Done {
                message: "The installed cheat file was removed and the prior state was restored."
                    .to_string(),
            };
        } else {
            self.local_pcsx2_install = LocalPcsx2InstallStage::Error {
                message: format!("Undo did not fully succeed: {:?}", result.status),
            };
        }
    }

    /// Mirrors `show_local_install_panel`'s ownership dance for the same
    /// reason: `self.local_pcsx2_install` is taken by value before
    /// rendering so the action buttons' `&mut self` calls never conflict
    /// with a live borrow of it.
    fn show_local_pcsx2_install_panel(&mut self, ui: &mut egui::Ui) {
        let stage = std::mem::take(&mut self.local_pcsx2_install);
        match stage {
            LocalPcsx2InstallStage::Idle => {}
            LocalPcsx2InstallStage::Blocked {
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
                    self.local_pcsx2_install = LocalPcsx2InstallStage::Idle;
                } else {
                    self.local_pcsx2_install = LocalPcsx2InstallStage::Blocked {
                        source_path,
                        message,
                    };
                }
            }
            LocalPcsx2InstallStage::AlreadyInstalled { source_path } => {
                ui.add_space(theme_gap());
                widgets::banner(
                    ui,
                    "Already installed",
                    &format!(
                        "{} is already installed for this game - installing it again would change nothing.",
                        source_path.display()
                    ),
                    widgets::StatusTone::Info,
                );
                if ui.button("Dismiss").clicked() {
                    self.local_pcsx2_install = LocalPcsx2InstallStage::Idle;
                } else {
                    self.local_pcsx2_install =
                        LocalPcsx2InstallStage::AlreadyInstalled { source_path };
                }
            }
            LocalPcsx2InstallStage::Preview {
                source_path,
                profile,
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
                        preview.staged.destination_path.display()
                    ));
                    ui.label(preview.plain_summary.clone());
                    egui::CollapsingHeader::new("Technical details")
                        .default_open(false)
                        .show(ui, |ui| {
                            for line in &preview.technical_details {
                                ui.label(line);
                            }
                        });
                    egui::CollapsingHeader::new("Merged PNACH contents to be written")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.monospace(
                                String::from_utf8_lossy(&preview.staged.contents).into_owned(),
                            );
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
                    self.local_pcsx2_install = LocalPcsx2InstallStage::Preview {
                        source_path,
                        profile,
                        preview,
                    };
                    self.confirm_local_pcsx2_apply();
                } else if cancelled {
                    self.local_pcsx2_install = LocalPcsx2InstallStage::Idle;
                } else {
                    self.local_pcsx2_install = LocalPcsx2InstallStage::Preview {
                        source_path,
                        profile,
                        preview,
                    };
                }
            }
            LocalPcsx2InstallStage::Applied {
                destination_root,
                journal_path,
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
                    self.local_pcsx2_install = LocalPcsx2InstallStage::Applied {
                        destination_root,
                        journal_path,
                    };
                    self.start_local_pcsx2_undo();
                } else if dismissed {
                    self.local_pcsx2_install = LocalPcsx2InstallStage::Idle;
                } else {
                    self.local_pcsx2_install = LocalPcsx2InstallStage::Applied {
                        destination_root,
                        journal_path,
                    };
                }
            }
            LocalPcsx2InstallStage::UndoPreview {
                destination_root,
                journal_path,
                preview,
            } => {
                ui.add_space(theme_gap());
                let mut confirmed = false;
                let mut cancelled = false;
                widgets::card(ui, |ui| {
                    ui.strong("Confirm undo");
                    ui.label("This will remove the installed managed cheat block and restore any prior file content.");
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
                    self.local_pcsx2_install = LocalPcsx2InstallStage::UndoPreview {
                        destination_root,
                        journal_path,
                        preview,
                    };
                    self.confirm_local_pcsx2_undo();
                } else if cancelled {
                    self.local_pcsx2_install = LocalPcsx2InstallStage::Applied {
                        destination_root,
                        journal_path: Some(journal_path),
                    };
                } else {
                    self.local_pcsx2_install = LocalPcsx2InstallStage::UndoPreview {
                        destination_root,
                        journal_path,
                        preview,
                    };
                }
            }
            LocalPcsx2InstallStage::Done { message } => {
                ui.add_space(theme_gap());
                widgets::banner(ui, "Undo complete", &message, widgets::StatusTone::Info);
                if ui.button("Dismiss").clicked() {
                    self.local_pcsx2_install = LocalPcsx2InstallStage::Idle;
                } else {
                    self.local_pcsx2_install = LocalPcsx2InstallStage::Done { message };
                }
            }
            LocalPcsx2InstallStage::Error { message } => {
                ui.add_space(theme_gap());
                widgets::banner(
                    ui,
                    "Local PCSX2 cheat install error",
                    &message,
                    widgets::StatusTone::Blocked,
                );
                if ui.button("Dismiss").clicked() {
                    self.local_pcsx2_install = LocalPcsx2InstallStage::Idle;
                } else {
                    self.local_pcsx2_install = LocalPcsx2InstallStage::Error { message };
                }
            }
        }
    }

    /// Discovers, checks for an already-installed identical state, stages,
    /// and previews - everything downstream of this is the same,
    /// unmodified Dolphin install-plan pipeline
    /// (`build_dolphin_install_preview`) the provider-driven Gecko flow
    /// already uses; this only supplies the one file the user picked,
    /// merged into the codes it becomes.
    fn start_local_dolphin_install(
        &mut self,
        source_path: PathBuf,
        candidate: DolphinCandidate,
        configuration_path: PathBuf,
        profile_id: String,
    ) {
        let discovery = match discover_local_dolphin_cheat_file(&source_path, &candidate) {
            Ok(discovery) => discovery,
            Err(error) => {
                self.local_dolphin_install = LocalDolphinInstallStage::Blocked {
                    source_path,
                    message: error.to_string(),
                };
                return;
            }
        };
        let destination = match load_dolphin_destination(&configuration_path, &candidate.game_id) {
            Ok(destination) => destination,
            Err(error) => {
                self.local_dolphin_install = LocalDolphinInstallStage::Blocked {
                    source_path,
                    message: error.to_string(),
                };
                return;
            }
        };
        if check_local_dolphin_install_state(&destination, &discovery)
            == LocalDolphinInstallState::AlreadyInstalled
        {
            self.local_dolphin_install = LocalDolphinInstallStage::AlreadyInstalled { source_path };
            return;
        }
        let staging_root = match crate::default_generated_dolphin_local_staging_root() {
            Ok(root) => root,
            Err(message) => {
                self.local_dolphin_install = LocalDolphinInstallStage::Error { message };
                return;
            }
        };
        let staged = match stage_local_dolphin_codes(&staging_root, &destination, &discovery) {
            Ok(staged) => staged,
            Err(error) => {
                self.local_dolphin_install = LocalDolphinInstallStage::Blocked {
                    source_path,
                    message: error.to_string(),
                };
                return;
            }
        };
        match build_dolphin_install_preview(&DolphinInstallPreviewRequest {
            selected_archive: candidate.path.clone(),
            configuration_path: configuration_path.clone(),
            game_id: candidate.game_id.clone(),
            revision: candidate.revision,
            staged,
        }) {
            Ok(preview) => {
                self.local_dolphin_install = LocalDolphinInstallStage::Preview {
                    source_path,
                    configuration_path,
                    profile_id,
                    preview: Box::new(preview),
                };
            }
            Err(error) => {
                self.local_dolphin_install = LocalDolphinInstallStage::Blocked {
                    source_path,
                    message: error.to_string(),
                };
            }
        }
    }

    fn confirm_local_dolphin_apply(&mut self) {
        let LocalDolphinInstallStage::Preview {
            configuration_path,
            profile_id,
            preview,
            ..
        } = std::mem::take(&mut self.local_dolphin_install)
        else {
            return;
        };
        let history_root = match default_shared_history_root() {
            Ok(root) => root,
            Err(error) => {
                self.local_dolphin_install = LocalDolphinInstallStage::Error {
                    message: format!("History root unavailable: {}", error.detail),
                };
                return;
            }
        };
        let backup_root = match default_shared_backup_root() {
            Ok(root) => root,
            Err(error) => {
                self.local_dolphin_install = LocalDolphinInstallStage::Error {
                    message: format!("Backup root unavailable: {}", error.detail),
                };
                return;
            }
        };
        let replacement_required = preview
            .report
            .entries
            .iter()
            .any(|entry| entry.proposed_action == PreviewProposedAction::Replace);
        let plan = match build_shared_transaction_plan(
            &preview.report,
            &profile_id,
            "dolphin-local-file",
            &preview.staged.staging_root,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                self.local_dolphin_install = LocalDolphinInstallStage::Error {
                    message: error.detail,
                };
                return;
            }
        };
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let destination_root = configuration_path.clone();
        let result = execute_shared_apply(
            &plan,
            &SharedApplyOptions {
                dry_run: false,
                confirmation: Some(SharedApplyConfirmation {
                    plan_id: plan.plan_id.clone(),
                    general_approved: true,
                    replacement_approved: replacement_required,
                }),
                operation_id: generate_shared_operation_id(),
                timestamp_unix_seconds: timestamp,
                current_context: plan.context.clone(),
                history_root,
                backup_root,
            },
        );
        if result.journal.status == SharedApplyStatus::Success {
            self.local_dolphin_install = LocalDolphinInstallStage::Applied {
                destination_root,
                journal_path: result.journal_path,
            };
        } else {
            self.local_dolphin_install = LocalDolphinInstallStage::Error {
                message: format!("Apply did not fully succeed: {:?}", result.journal.status),
            };
        }
    }

    fn start_local_dolphin_undo(&mut self) {
        let LocalDolphinInstallStage::Applied {
            destination_root,
            journal_path,
        } = std::mem::take(&mut self.local_dolphin_install)
        else {
            return;
        };
        let Some(journal_path) = journal_path else {
            self.local_dolphin_install = LocalDolphinInstallStage::Error {
                message: "No transaction journal was recorded for this apply.".to_string(),
            };
            return;
        };
        let backup_root = match default_shared_backup_root() {
            Ok(root) => root,
            Err(error) => {
                self.local_dolphin_install = LocalDolphinInstallStage::Error {
                    message: format!("Backup root unavailable: {}", error.detail),
                };
                return;
            }
        };
        let preview = preview_shared_rollback(&journal_path, &destination_root, &backup_root);
        self.local_dolphin_install = LocalDolphinInstallStage::UndoPreview {
            destination_root,
            journal_path,
            preview: Box::new(preview),
        };
    }

    fn confirm_local_dolphin_undo(&mut self) {
        let LocalDolphinInstallStage::UndoPreview { preview, .. } =
            std::mem::take(&mut self.local_dolphin_install)
        else {
            return;
        };
        let history_root = match default_shared_history_root() {
            Ok(root) => root,
            Err(error) => {
                self.local_dolphin_install = LocalDolphinInstallStage::Error {
                    message: format!("History root unavailable: {}", error.detail),
                };
                return;
            }
        };
        let backup_root = match default_shared_backup_root() {
            Ok(root) => root,
            Err(error) => {
                self.local_dolphin_install = LocalDolphinInstallStage::Error {
                    message: format!("Backup root unavailable: {}", error.detail),
                };
                return;
            }
        };
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let result = execute_shared_rollback(
            &preview,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: preview.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: generate_shared_operation_id(),
                timestamp_unix_seconds: timestamp,
                history_root,
                backup_root,
            },
        );
        if result.status == SharedApplyStatus::Success {
            self.local_dolphin_install = LocalDolphinInstallStage::Done {
                message: "The installed cheat file was removed and the prior state was restored."
                    .to_string(),
            };
        } else {
            self.local_dolphin_install = LocalDolphinInstallStage::Error {
                message: format!("Undo did not fully succeed: {:?}", result.status),
            };
        }
    }

    /// Mirrors `show_local_pcsx2_install_panel`'s ownership dance for the
    /// same reason: `self.local_dolphin_install` is taken by value before
    /// rendering so the action buttons' `&mut self` calls never conflict
    /// with a live borrow of it.
    fn show_local_dolphin_install_panel(&mut self, ui: &mut egui::Ui) {
        let stage = std::mem::take(&mut self.local_dolphin_install);
        match stage {
            LocalDolphinInstallStage::Idle => {}
            LocalDolphinInstallStage::Blocked {
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
                    self.local_dolphin_install = LocalDolphinInstallStage::Idle;
                } else {
                    self.local_dolphin_install = LocalDolphinInstallStage::Blocked {
                        source_path,
                        message,
                    };
                }
            }
            LocalDolphinInstallStage::AlreadyInstalled { source_path } => {
                ui.add_space(theme_gap());
                widgets::banner(
                    ui,
                    "Already installed",
                    &format!(
                        "{} is already installed, unchanged, for this game - installing it again would change nothing.",
                        source_path.display()
                    ),
                    widgets::StatusTone::Info,
                );
                if ui.button("Dismiss").clicked() {
                    self.local_dolphin_install = LocalDolphinInstallStage::Idle;
                } else {
                    self.local_dolphin_install =
                        LocalDolphinInstallStage::AlreadyInstalled { source_path };
                }
            }
            LocalDolphinInstallStage::Preview {
                source_path,
                configuration_path,
                profile_id,
                preview,
            } => {
                ui.add_space(theme_gap());
                let mut confirmed = false;
                let mut cancelled = false;
                widgets::card(ui, |ui| {
                    ui.strong("Review before installing");
                    ui.label(format!("Source file: {}", source_path.display()));
                    ui.label(format!("Destination: {}", preview.staged.path.display()));
                    ui.label(format!(
                        "{} code(s) merged in this install.",
                        preview.staged.selected_code_count
                    ));
                    for entry in &preview.report.entries {
                        ui.label(format!("{:?}", entry.proposed_action));
                    }
                    egui::CollapsingHeader::new("Merged GameSettings contents to be written")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.monospace(preview.staged.contents.clone());
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
                    self.local_dolphin_install = LocalDolphinInstallStage::Preview {
                        source_path,
                        configuration_path,
                        profile_id: profile_id.clone(),
                        preview,
                    };
                    self.confirm_local_dolphin_apply();
                } else if cancelled {
                    self.local_dolphin_install = LocalDolphinInstallStage::Idle;
                } else {
                    self.local_dolphin_install = LocalDolphinInstallStage::Preview {
                        source_path,
                        configuration_path,
                        profile_id: profile_id.clone(),
                        preview,
                    };
                }
            }
            LocalDolphinInstallStage::Applied {
                destination_root,
                journal_path,
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
                    self.local_dolphin_install = LocalDolphinInstallStage::Applied {
                        destination_root,
                        journal_path,
                    };
                    self.start_local_dolphin_undo();
                } else if dismissed {
                    self.local_dolphin_install = LocalDolphinInstallStage::Idle;
                } else {
                    self.local_dolphin_install = LocalDolphinInstallStage::Applied {
                        destination_root,
                        journal_path,
                    };
                }
            }
            LocalDolphinInstallStage::UndoPreview {
                destination_root,
                journal_path,
                preview,
            } => {
                ui.add_space(theme_gap());
                let mut confirmed = false;
                let mut cancelled = false;
                widgets::card(ui, |ui| {
                    ui.strong("Confirm undo");
                    ui.label("This will remove the installed managed codes and restore any prior file content.");
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
                    self.local_dolphin_install = LocalDolphinInstallStage::UndoPreview {
                        destination_root,
                        journal_path,
                        preview,
                    };
                    self.confirm_local_dolphin_undo();
                } else if cancelled {
                    self.local_dolphin_install = LocalDolphinInstallStage::Applied {
                        destination_root,
                        journal_path: Some(journal_path),
                    };
                } else {
                    self.local_dolphin_install = LocalDolphinInstallStage::UndoPreview {
                        destination_root,
                        journal_path,
                        preview,
                    };
                }
            }
            LocalDolphinInstallStage::Done { message } => {
                ui.add_space(theme_gap());
                widgets::banner(ui, "Undo complete", &message, widgets::StatusTone::Info);
                if ui.button("Dismiss").clicked() {
                    self.local_dolphin_install = LocalDolphinInstallStage::Idle;
                } else {
                    self.local_dolphin_install = LocalDolphinInstallStage::Done { message };
                }
            }
            LocalDolphinInstallStage::Error { message } => {
                ui.add_space(theme_gap());
                widgets::banner(
                    ui,
                    "Local Dolphin cheat install error",
                    &message,
                    widgets::StatusTone::Blocked,
                );
                if ui.button("Dismiss").clicked() {
                    self.local_dolphin_install = LocalDolphinInstallStage::Idle;
                } else {
                    self.local_dolphin_install = LocalDolphinInstallStage::Error { message };
                }
            }
        }
    }
}

impl UserCheatImportPageState {
    fn show_local_xenia_install_picker(
        &mut self,
        ui: &mut egui::Ui,
        context: Option<&LocalXeniaInstallContext>,
    ) {
        ui.add_space(theme_gap());
        widgets::card(ui, |ui| {
            ui.strong("Install a local Xenia patch file (.patch.toml)");
            ui.label("The file is parsed by Xenia's existing strict patch parser and bound to the selected game's verified Title ID. No network access is used.");
            let Some(context) = context else {
                ui.label("Select an Xbox 360/Xenia game in Cheats & Mods first.");
                return;
            };
            if context.title_id.is_none() {
                ui.label("Blocked: the selected game's Xenia Title ID is unresolved or ambiguous.");
            }
            if context.profile.is_none() {
                ui.label("Blocked: select one eligible Xenia profile first.");
            }
            let enabled = context.title_id.is_some()
                && context.profile.is_some()
                && matches!(self.local_xenia_install, LocalXeniaInstallStage::Idle);
            if widgets::action_button(
                ui,
                "Choose a Xenia .patch.toml file…",
                widgets::ActionStyle::Primary,
                enabled,
            )
            .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Xenia patches", &["patch.toml"])
                    .pick_file()
            {
                let profile = context.profile.clone().expect("enabled picker has profile");
                self.start_local_xenia_install(path, context.title_id.clone(), profile);
            }
        });
    }

    /// Shows the neutral conversion service without coupling the GUI to any
    /// parser or emulator writer.  The current bounded import report does not
    /// expose individual code lines, so it is represented honestly as one
    /// opaque operation: targets and their reasons remain reviewable, while
    /// the UI can never present an incomplete conversion as safe to apply.
    fn show_conversion_preview(
        &mut self,
        ui: &mut egui::Ui,
        candidate: &UserCheatCandidate,
        index: usize,
    ) {
        if !ui
            .button(if self.converter_candidate == Some(index) {
                "Hide conversion preview"
            } else {
                "Convert cheat"
            })
            .clicked()
        {
            return;
        }
        self.converter_candidate = (self.converter_candidate != Some(index)).then_some(index);
        if self.converter_candidate != Some(index) {
            return;
        }
        let source_format = match candidate.format {
            UserCheatFormat::RetroarchCht => CheatSourceFormat::RetroArch,
            UserCheatFormat::Pcsx2Pnach => CheatSourceFormat::Pnach,
        };
        let platform = candidate
            .platform_hint
            .as_deref()
            .map(|value| {
                let lower = value.to_ascii_lowercase();
                if lower.contains("gamecube") || lower == "gc" {
                    CheatPlatform::GameCube
                } else if lower == "wii" {
                    CheatPlatform::Wii
                } else if lower.contains("playstation 2") || lower == "ps2" {
                    CheatPlatform::Ps2
                } else if lower.contains("nintendo ds") || lower == "nds" {
                    CheatPlatform::NintendoDs
                } else {
                    CheatPlatform::Other(value.to_string())
                }
            })
            .unwrap_or_else(|| CheatPlatform::Other("unknown".into()));
        let document = CheatDocument {
            title: candidate.provenance.original_filename.clone(),
            platform,
            source_format: source_format.clone(),
            operations: vec![CheatOperation::UnsupportedRaw {
                source_format,
                raw: candidate.provenance.original_filename.clone(),
                reason: "Individual operations are not exposed by the bounded import report."
                    .into(),
            }],
            issues: vec![CheatIssue::RawPreserved],
            provenance: vec![candidate.provenance.original_path.display().to_string()],
        };
        widgets::card(ui, |ui| {
            ui.strong("Conversion preview");
            ui.label("Choose a target format to review what can be converted safely.");
            for capability in supported_targets_for(&document) {
                let target = target_label(&capability.target);
                let status = match capability.capability {
                    ConversionCapability::Exact => "Exact".to_string(),
                    ConversionCapability::Lossy { .. } => "Lossy".to_string(),
                    ConversionCapability::Unsupported { reason } => {
                        format!("Unsupported: {reason}")
                    }
                };
                ui.label(format!("{target}: {status}"));
            }
            let target = supported_targets_for(&document)
                .first()
                .map(|entry| entry.target.clone());
            if let Some(target) = target {
                let preview = convert_cheat_document(&document, target);
                ui.label(format!(
                    "Operations: {} exact, {} lossy, {} unsupported",
                    preview.exact_operations,
                    preview.lossy_operations,
                    preview.unsupported_operations
                ));
                ui.label(if preview.can_apply {
                    "Can convert"
                } else {
                    "Can't convert safely"
                });
            }
            ui.label("No emulator files were written.");
        });
    }

    fn start_local_xenia_install(
        &mut self,
        source_path: PathBuf,
        title_id: Option<String>,
        profile: XeniaProfile,
    ) {
        let discovery = match discover_local_xenia_patch_file(&source_path, title_id.as_deref()) {
            Ok(value) => value,
            Err(error) => {
                self.local_xenia_install = LocalXeniaInstallStage::Blocked {
                    source_path,
                    message: error.to_string(),
                };
                return;
            }
        };
        let file_name = source_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        let destination =
            match load_local_xenia_destination(&profile.configuration_path, &file_name) {
                Ok(value) => value,
                Err(error) => {
                    self.local_xenia_install = LocalXeniaInstallStage::Blocked {
                        source_path,
                        message: error.to_string(),
                    };
                    return;
                }
            };
        if check_local_xenia_install_state(&destination, &discovery)
            == LocalXeniaInstallState::AlreadyInstalled
        {
            self.local_xenia_install = LocalXeniaInstallStage::AlreadyInstalled { source_path };
            return;
        }
        let staging_root = match crate::default_generated_xenia_staging_root() {
            Ok(root) => root,
            Err(message) => {
                self.local_xenia_install = LocalXeniaInstallStage::Error { message };
                return;
            }
        };
        let staged = match stage_local_xenia_patch_file(
            &staging_root,
            &file_name,
            &discovery,
            destination.document.as_ref(),
        ) {
            Ok(value) => value,
            Err(error) => {
                self.local_xenia_install = LocalXeniaInstallStage::Blocked {
                    source_path,
                    message: error.to_string(),
                };
                return;
            }
        };
        match build_xenia_install_preview(&XeniaInstallPreviewRequest {
            selected_archive: source_path.clone(),
            configuration_path: profile.configuration_path.clone(),
            title_id: discovery.candidate.title_id.clone(),
            compatibility: discovery.candidate.compatibility,
            staged,
        }) {
            Ok(preview) => {
                self.local_xenia_install = LocalXeniaInstallStage::Preview {
                    source_path,
                    profile_id: profile.profile_id,
                    configuration_path: profile.configuration_path,
                    preview: Box::new(preview),
                }
            }
            Err(error) => {
                self.local_xenia_install = LocalXeniaInstallStage::Blocked {
                    source_path,
                    message: error.to_string(),
                }
            }
        }
    }

    fn confirm_local_xenia_apply(&mut self) {
        let LocalXeniaInstallStage::Preview {
            profile_id,
            configuration_path,
            preview,
            ..
        } = std::mem::take(&mut self.local_xenia_install)
        else {
            return;
        };
        let (Ok(history_root), Ok(backup_root)) =
            (default_shared_history_root(), default_shared_backup_root())
        else {
            self.local_xenia_install = LocalXeniaInstallStage::Error {
                message: "Shared history/backup roots are unavailable.".to_string(),
            };
            return;
        };
        let plan = match build_shared_transaction_plan(
            &preview.report,
            &profile_id,
            "xenia-local-file",
            &preview.staged.staging_root,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                self.local_xenia_install = LocalXeniaInstallStage::Error {
                    message: format!("{error:?}"),
                };
                return;
            }
        };
        let replacement = plan
            .entries
            .iter()
            .any(|entry| entry.proposed_action == PreviewProposedAction::Replace);
        let result = execute_shared_apply(
            &plan,
            &SharedApplyOptions {
                dry_run: false,
                confirmation: Some(SharedApplyConfirmation {
                    plan_id: plan.plan_id.clone(),
                    general_approved: true,
                    replacement_approved: replacement,
                }),
                operation_id: generate_shared_operation_id(),
                timestamp_unix_seconds: now_unix_seconds(),
                current_context: plan.context.clone(),
                history_root,
                backup_root,
            },
        );
        if result.journal.status == SharedApplyStatus::Success {
            self.local_xenia_install = LocalXeniaInstallStage::Applied {
                destination_root: configuration_path,
                journal_path: result.journal_path,
            };
        } else {
            self.local_xenia_install = LocalXeniaInstallStage::Error {
                message: format!("Apply did not fully succeed: {:?}", result.journal.status),
            };
        }
    }

    fn show_local_xenia_install_panel(&mut self, ui: &mut egui::Ui) {
        let stage = std::mem::take(&mut self.local_xenia_install);
        match stage {
            LocalXeniaInstallStage::Idle => {}
            LocalXeniaInstallStage::Blocked {
                source_path,
                message,
            } => {
                widgets::banner(
                    ui,
                    &format!("Cannot install {}", source_path.display()),
                    &message,
                    widgets::StatusTone::Blocked,
                );
                if !ui.button("Dismiss").clicked() {
                    self.local_xenia_install = LocalXeniaInstallStage::Blocked {
                        source_path,
                        message,
                    };
                }
            }
            LocalXeniaInstallStage::AlreadyInstalled { source_path } => {
                widgets::banner(
                    ui,
                    "Already installed",
                    &format!("{} is already installed unchanged.", source_path.display()),
                    widgets::StatusTone::Info,
                );
                if !ui.button("Dismiss").clicked() {
                    self.local_xenia_install =
                        LocalXeniaInstallStage::AlreadyInstalled { source_path };
                }
            }
            LocalXeniaInstallStage::Preview {
                source_path,
                profile_id,
                configuration_path,
                preview,
            } => {
                let mut confirm = false;
                let mut cancel = false;
                widgets::card(ui, |ui| {
                    ui.strong("Review local Xenia patch before installing");
                    ui.label(format!("Source path: {}", source_path.display()));
                    ui.label(format!(
                        "Selected Xenia Title ID: {}",
                        preview
                            .report
                            .entries
                            .first()
                            .and_then(|entry| entry.verified_identity.clone())
                            .unwrap_or_else(|| "unknown".to_string())
                    ));
                    ui.label(format!("Destination: {}", preview.staged.path.display()));
                    ui.label(format!(
                        "Parsed patch entries: {}",
                        preview.staged.selected_patch_count
                    ));
                    for entry in &preview.report.entries {
                        ui.label(format!(
                            "Mutation: {:?} ({:?})",
                            entry.proposed_action, entry.destination_state
                        ));
                    }
                    egui::CollapsingHeader::new("Exact mutation contents")
                        .default_open(false)
                        .show(ui, |ui| ui.monospace(&preview.staged.contents));
                    if widgets::action_button(
                        ui,
                        "Confirm install",
                        widgets::ActionStyle::Primary,
                        true,
                    )
                    .clicked()
                    {
                        confirm = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
                if confirm {
                    self.local_xenia_install = LocalXeniaInstallStage::Preview {
                        source_path,
                        profile_id,
                        configuration_path,
                        preview,
                    };
                    self.confirm_local_xenia_apply();
                } else if !cancel {
                    self.local_xenia_install = LocalXeniaInstallStage::Preview {
                        source_path,
                        profile_id,
                        configuration_path,
                        preview,
                    };
                }
            }
            LocalXeniaInstallStage::Applied {
                destination_root,
                journal_path,
            } => {
                widgets::status_badge(ui, "Installed", widgets::StatusTone::Success);
                if let Some(path) = journal_path.as_ref() {
                    ui.label(format!("Transaction journal: {}", path.display()));
                }
                if ui.button("Undo this install").clicked() {
                    if let Some(path) = journal_path {
                        self.local_xenia_install = LocalXeniaInstallStage::UndoPreview {
                            destination_root: destination_root.clone(),
                            preview: Box::new(preview_shared_rollback(
                                &path,
                                &destination_root,
                                &default_shared_backup_root().unwrap_or_default(),
                            )),
                        };
                    }
                } else {
                    self.local_xenia_install = LocalXeniaInstallStage::Applied {
                        destination_root,
                        journal_path,
                    };
                }
            }
            LocalXeniaInstallStage::UndoPreview {
                destination_root,
                preview,
            } => {
                let mut confirm = false;
                if widgets::action_button(ui, "Confirm undo", widgets::ActionStyle::Primary, true)
                    .clicked()
                {
                    confirm = true;
                }
                if confirm {
                    let (Ok(history_root), Ok(backup_root)) =
                        (default_shared_history_root(), default_shared_backup_root())
                    else {
                        self.local_xenia_install = LocalXeniaInstallStage::Error {
                            message: "Shared history/backup roots unavailable.".to_string(),
                        };
                        return;
                    };
                    let result = execute_shared_rollback(
                        &preview,
                        &SharedRollbackOptions {
                            confirmation: SharedRollbackConfirmation {
                                preview_id: preview.preview_id.clone(),
                                approved: true,
                            },
                            rollback_operation_id: generate_shared_operation_id(),
                            timestamp_unix_seconds: now_unix_seconds(),
                            history_root,
                            backup_root,
                        },
                    );
                    self.local_xenia_install = if result.status == SharedApplyStatus::Success {
                        LocalXeniaInstallStage::Done {
                            message: "Xenia patch install undone and prior state restored."
                                .to_string(),
                        }
                    } else {
                        LocalXeniaInstallStage::Error {
                            message: format!("Undo did not fully succeed: {:?}", result.status),
                        }
                    };
                } else {
                    self.local_xenia_install = LocalXeniaInstallStage::UndoPreview {
                        destination_root,
                        preview,
                    };
                }
            }
            LocalXeniaInstallStage::Done { message } => {
                widgets::banner(ui, "Undo complete", &message, widgets::StatusTone::Info);
                if !ui.button("Dismiss").clicked() {
                    self.local_xenia_install = LocalXeniaInstallStage::Done { message };
                }
            }
            LocalXeniaInstallStage::Error { message } => {
                widgets::banner(
                    ui,
                    "Local Xenia install error",
                    &message,
                    widgets::StatusTone::Blocked,
                );
                if !ui.button("Dismiss").clicked() {
                    self.local_xenia_install = LocalXeniaInstallStage::Error { message };
                }
            }
        }
    }
}

fn now_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
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

fn target_label(target: &CheatTargetFormat) -> &'static str {
    match target {
        CheatTargetFormat::DolphinActionReplay => "Dolphin Action Replay",
        CheatTargetFormat::Gecko => "Gecko",
        CheatTargetFormat::Pnach => "PCSX2 PNACH",
        CheatTargetFormat::RetroArch => "RetroArch",
        CheatTargetFormat::ActionReplayDs => "Action Replay DS",
        CheatTargetFormat::GameSharkPs2 => "PS2 GameShark",
        CheatTargetFormat::CodeBreakerPs2 => "PS2 CodeBreaker",
        CheatTargetFormat::DolphinOnFrame => "Dolphin On-Frame",
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
