//! Read-only review of user-supplied RetroArch and PCSX2 cheat files.
//!
//! This page intentionally does not share state with CheatBase or the
//! emulator-specific installation workflows. The core importer is an index:
//! it reads bounded local files, reports provenance and matching evidence, and
//! never offers an install operation.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use archivefs_core::patch_manager::{
    UserCheatCandidate, UserCheatDiagnostic, UserCheatFormat, UserCheatImportError,
    UserCheatImportReport, UserCheatLibraryGame, UserCheatMatchState, scan_user_cheat_directory,
    scan_user_cheat_file,
};
use eframe::egui;

use crate::ui::components as widgets;

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
                        self.show_report(ui, report);
                    }
                }
            }
        });
    }

    fn show_report(&mut self, ui: &mut egui::Ui, report: &UserCheatImportReport) {
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
        self.show_candidates(ui, report, "Matched", |candidate| {
            matches!(
                candidate.match_state,
                UserCheatMatchState::Exact | UserCheatMatchState::Strong
            )
        });
        self.show_candidates(ui, report, "Possible matches", |candidate| {
            candidate.match_state == UserCheatMatchState::Possible
        });
        self.show_candidates(ui, report, "Ambiguous matches", |candidate| {
            candidate.match_state == UserCheatMatchState::Ambiguous
        });
        self.show_candidates(ui, report, "Unmatched", |candidate| {
            candidate.match_state == UserCheatMatchState::NoMatch
        });
        self.show_candidates(ui, report, "Unsupported or rejected", |candidate| {
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
                    });
                }
            });
    }
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
