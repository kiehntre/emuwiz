//! Beginner-facing, browse-only CheatBase UI.
//!
//! This module is deliberately a thin GUI adapter over the immutable CheatBase
//! provider. It owns no installation or emulator-write path: setup actions use
//! the provider's validated activation APIs, while search and inspection use
//! the provider's read-only catalogue API.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use archivefs_core::patch_manager::{
    CheatBaseCatalogue, CheatBaseCheat, CheatBaseDownloadOptions, CheatBaseError, CheatBaseGame,
    CheatBaseGameSearchRequest, CheatBasePaths, CheatBaseSourceStatus, CheatProviderSourceState,
    HttpsCheatSourceTransport, PageRequest, ProviderGameMatchConfidence, ProviderPage,
    ReadOnlyCheatCatalogue, default_cheatbase_source_root, download_cheatbase_database,
    import_local_cheatbase_database, inspect_cheatbase_source, set_cheatbase_enabled,
    validate_installed_cheatbase_source,
};
use eframe::egui;

use crate::ui::{components as widgets, theme};

const BROWSE_ONLY_COPY: &str = "Browse only — installation from CheatBase is not enabled yet.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheatBaseGameSeed {
    pub(crate) title: String,
    pub(crate) platform: Option<String>,
    pub(crate) region: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TaskKind {
    Setup,
    Search,
    Inspect,
}

#[derive(Debug)]
enum TaskResult {
    Setup(Result<CheatBaseSourceStatus, String>),
    Search(Result<archivefs_core::patch_manager::CheatBaseGameSearchResult, String>),
    Inspect(Result<(CheatBaseGame, ProviderPage<CheatBaseCheat>), String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheatBasePageAction {
    Download,
    Import(PathBuf),
    Validate,
    Enable,
    Refresh,
    Search,
    Inspect(i64),
}

/// State for the Cheats & Mods CheatBase section. Search/inspection and the
/// potentially large setup operations run off the egui thread; the state is
/// only replaced by typed task results.
#[derive(Default)]
pub(crate) struct CheatBasePageState {
    status: Option<Result<CheatBaseSourceStatus, String>>,
    task: Option<(TaskKind, Receiver<TaskResult>)>,
    title_query: String,
    platform_query: String,
    region_query: String,
    seeded_for: Option<CheatBaseGameSeed>,
    search: Option<Result<archivefs_core::patch_manager::CheatBaseGameSearchResult, String>>,
    selected_game: Option<CheatBaseGame>,
    selected_cheats: Option<ProviderPage<CheatBaseCheat>>,
    inspection_error: Option<String>,
}

impl CheatBasePageState {
    pub(crate) fn poll(&mut self, context: &egui::Context) {
        let Some((kind, receiver)) = self.task.as_ref() else {
            return;
        };
        let kind = kind.clone();
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                context.request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
            Err(TryRecvError::Disconnected) => {
                let message =
                    "The CheatBase background operation stopped unexpectedly.".to_string();
                match kind {
                    TaskKind::Setup => TaskResult::Setup(Err(message)),
                    TaskKind::Search => TaskResult::Search(Err(message)),
                    TaskKind::Inspect => TaskResult::Inspect(Err(message)),
                }
            }
        };
        self.task = None;
        match result {
            TaskResult::Setup(result) => self.status = Some(result),
            TaskResult::Search(result) => {
                self.search = Some(result);
                self.selected_game = None;
                self.selected_cheats = None;
                self.inspection_error = None;
            }
            TaskResult::Inspect(result) => match result {
                Ok((game, cheats)) => {
                    self.selected_game = Some(game);
                    self.selected_cheats = Some(cheats);
                    self.inspection_error = None;
                }
                Err(error) => {
                    self.selected_game = None;
                    self.selected_cheats = None;
                    self.inspection_error = Some(error);
                }
            },
        }
        context.request_repaint();
    }

    pub(crate) fn seed_from_selected(&mut self, seed: Option<CheatBaseGameSeed>) {
        let Some(seed) = seed else {
            return;
        };
        if self.seeded_for.as_ref() == Some(&seed) {
            return;
        }
        self.seeded_for = Some(seed.clone());
        if self.title_query.trim().is_empty() {
            self.title_query = seed.title;
        }
        if self.platform_query.trim().is_empty() {
            self.platform_query = seed.platform.unwrap_or_default();
        }
        if self.region_query.trim().is_empty() {
            self.region_query = seed.region.unwrap_or_default();
        }
    }

    pub(crate) fn handle(&mut self, action: CheatBasePageAction, context: egui::Context) {
        if matches!(action, CheatBasePageAction::Refresh) {
            self.refresh_status();
            return;
        }
        if matches!(
            action,
            CheatBasePageAction::Search | CheatBasePageAction::Inspect(_)
        ) && self.task.is_some()
        {
            return;
        }
        match action {
            CheatBasePageAction::Download => self.start_setup(context, SetupAction::Download),
            CheatBasePageAction::Import(path) => {
                self.start_setup(context, SetupAction::Import(path))
            }
            CheatBasePageAction::Validate => self.start_setup(context, SetupAction::Validate),
            CheatBasePageAction::Enable => self.start_setup(context, SetupAction::Enable),
            CheatBasePageAction::Refresh => unreachable!(),
            CheatBasePageAction::Search => self.start_search(context),
            CheatBasePageAction::Inspect(release_id) => self.start_inspection(context, release_id),
        }
    }

    fn refresh_status(&mut self) {
        self.status = Some(
            default_cheatbase_source_root()
                .and_then(|root| inspect_cheatbase_source(&CheatBasePaths::at(root)))
                .map_err(|error| error.to_string()),
        );
    }

    fn source_paths() -> Result<CheatBasePaths, String> {
        default_cheatbase_source_root()
            .map(CheatBasePaths::at)
            .map_err(|error| error.to_string())
    }

    fn start_setup(&mut self, context: egui::Context, action: SetupAction) {
        if self.task.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.task = Some((TaskKind::Setup, receiver));
        thread::spawn(move || {
            let result = Self::run_setup(action).map_err(|error| error.to_string());
            let _ = sender.send(TaskResult::Setup(result));
            context.request_repaint();
        });
    }

    fn run_setup(action: SetupAction) -> Result<CheatBaseSourceStatus, CheatBaseError> {
        let paths = Self::source_paths().map_err(|message| CheatBaseError {
            kind: archivefs_core::patch_manager::CheatBaseErrorKind::UnsafePath,
            message,
        })?;
        match action {
            SetupAction::Download => Ok(download_cheatbase_database(
                &paths,
                &CheatBaseDownloadOptions::default(),
                &HttpsCheatSourceTransport::new(),
            )?
            .status),
            SetupAction::Import(source) => {
                Ok(import_local_cheatbase_database(&paths, &source)?.status)
            }
            SetupAction::Validate => validate_installed_cheatbase_source(&paths),
            SetupAction::Enable => set_cheatbase_enabled(&paths, true),
        }
    }

    fn start_search(&mut self, context: egui::Context) {
        let Ok(paths) = Self::source_paths() else {
            self.search = Some(Err(
                "CheatBase source location could not be resolved.".to_string()
            ));
            return;
        };
        let request = CheatBaseGameSearchRequest {
            platform_id: nonempty(&self.platform_query),
            title: self.title_query.trim().to_string(),
            region: nonempty(&self.region_query),
            upstream_release_id: None,
            page: PageRequest::games(0),
        };
        let (sender, receiver) = mpsc::channel();
        self.task = Some((TaskKind::Search, receiver));
        thread::spawn(move || {
            let result = CheatBaseCatalogue::open_installed(&paths)
                .and_then(|catalogue| catalogue.search_games(&request))
                .map_err(|error| error.to_string());
            let _ = sender.send(TaskResult::Search(result));
            context.request_repaint();
        });
    }

    fn start_inspection(&mut self, context: egui::Context, release_id: i64) {
        let Ok(paths) = Self::source_paths() else {
            self.inspection_error =
                Some("CheatBase source location could not be resolved.".to_string());
            return;
        };
        let (sender, receiver) = mpsc::channel();
        self.task = Some((TaskKind::Inspect, receiver));
        thread::spawn(move || {
            let result = (|| {
                let catalogue = CheatBaseCatalogue::open_installed(&paths)?;
                let game = catalogue.game(release_id)?.ok_or_else(|| CheatBaseError {
                    kind: archivefs_core::patch_manager::CheatBaseErrorKind::Query,
                    message: "The selected CheatBase game is no longer available.".to_string(),
                })?;
                let cheats = catalogue.cheats(release_id, PageRequest::cheats(0))?;
                Ok((game, cheats))
            })()
            .map_err(|error: CheatBaseError| error.to_string());
            let _ = sender.send(TaskResult::Inspect(result));
            context.request_repaint();
        });
    }

    fn source_is_usable(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|status| status.as_ref().is_ok_and(|status| status.usable))
    }

    fn busy(&self) -> bool {
        self.task.is_some()
    }
}

enum SetupAction {
    Download,
    Import(PathBuf),
    Validate,
    Enable,
}

fn nonempty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.trim().to_string())
}

pub(crate) fn show_cheatbase_page(
    ui: &mut egui::Ui,
    state: &mut CheatBasePageState,
    selected_seed: Option<CheatBaseGameSeed>,
) -> Option<CheatBasePageAction> {
    if state.status.is_none() && !state.busy() {
        state.refresh_status();
    }
    state.seed_from_selected(selected_seed);
    let mut action = None;
    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(
        ui,
        "CheatBase",
        Some("Search and inspect game cheat information from the pinned CheatBase catalogue."),
    );
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong("CheatBase");
            let (label, tone) = source_state_presentation(state.status.as_ref());
            widgets::status_badge(ui, label, tone);
        });
        ui.label("CheatBase is a searchable catalogue of game cheat information.");
        if let Some(Err(error)) = &state.status {
            widgets::banner(
                ui,
                "CheatBase is unavailable",
                &human_error(error),
                widgets::StatusTone::Blocked,
            );
            widgets::technical_details(ui, "cheatbase-status-error", |ui| {
                ui.label(error);
            });
        }
        if let Some(Ok(status)) = &state.status {
            if let Some(error) = &status.last_error {
                widgets::banner(
                    ui,
                    "CheatBase needs attention",
                    &human_error(&error.message),
                    widgets::StatusTone::Warning,
                );
                widgets::technical_details(ui, "cheatbase-last-error", |ui| {
                    ui.label(&error.message);
                });
            }
            if status.usable {
                ui.label("The validated local catalogue is ready to browse.");
            } else if matches!(
                status.state,
                CheatProviderSourceState::Invalid
                    | CheatProviderSourceState::ValidationFailed
                    | CheatProviderSourceState::UnsupportedSchema
            ) {
                ui.label("The local catalogue did not pass validation. Validate it again or choose another copy.");
            } else {
                ui.label("Set up CheatBase to search its catalogue.");
            }
        } else if state.status.is_none() {
            ui.label("Checking whether CheatBase is set up…");
        }
        ui.horizontal_wrapped(|ui| {
            let enabled = !state.busy();
            let source_disabled = state.status.as_ref().is_some_and(|status| {
                status
                    .as_ref()
                    .is_ok_and(|status| status.state == CheatProviderSourceState::Disabled)
            });
            if widgets::action_button(
                ui,
                "Download pinned source",
                widgets::ActionStyle::Primary,
                enabled,
            )
            .clicked()
            {
                action = Some(CheatBasePageAction::Download);
            }
            if widgets::action_button(
                ui,
                "Import local database",
                widgets::ActionStyle::Secondary,
                enabled,
            )
            .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("SQLite database", &["sqlite", "sqlite3", "db"])
                    .pick_file()
            {
                action = Some(CheatBasePageAction::Import(path));
            }
            if widgets::action_button(
                ui,
                "Validate source",
                widgets::ActionStyle::Quiet,
                enabled && state.status.is_some(),
            )
            .clicked()
            {
                action = Some(CheatBasePageAction::Validate);
            }
            if source_disabled
                && widgets::action_button(
                    ui,
                    "Enable CheatBase",
                    widgets::ActionStyle::Secondary,
                    enabled,
                )
                .clicked()
            {
                action = Some(CheatBasePageAction::Enable);
            }
            if widgets::action_button(ui, "Refresh status", widgets::ActionStyle::Quiet, enabled)
                .clicked()
            {
                action = Some(CheatBasePageAction::Refresh);
            }
        });
        widgets::technical_details(ui, "cheatbase-source-details", |ui| {
            if let Some(Ok(status)) = &state.status {
                ui.label(format!("Provider: {}", status.provider.id));
                ui.label(format!("Source path: {}", status.database_path.display()));
                ui.label(format!("Upstream: {}", status.provider.upstream_project));
                ui.label(format!(
                    "Pinned commit: {}",
                    status.validation.as_ref().map_or_else(
                        || "not validated".to_string(),
                        |v| v.upstream_commit.clone()
                    )
                ));
                ui.label(format!(
                    "Pinned SHA-256: {}",
                    archivefs_core::patch_manager::CHEATBASE_EXPECTED_SHA256
                ));
                ui.label(format!("Attribution: {}", status.provenance.verification));
                ui.label(format!("Licence: {}", status.licence.statement));
            }
        });
    });

    if state.source_is_usable() {
        ui.add_space(theme::SECTION_GAP);
        widgets::section_header(
            ui,
            "Search games",
            Some("Search by title, system, and region. Select a result to inspect its cheats."),
        );
        ui.horizontal_wrapped(|ui| {
            ui.label("Title");
            ui.add(
                egui::TextEdit::singleline(&mut state.title_query)
                    .hint_text("Game title")
                    .desired_width(260.0),
            );
            ui.label("System");
            ui.add(
                egui::TextEdit::singleline(&mut state.platform_query)
                    .hint_text("e.g. Nintendo DS")
                    .desired_width(150.0),
            );
            ui.label("Region");
            ui.add(
                egui::TextEdit::singleline(&mut state.region_query)
                    .hint_text("Optional")
                    .desired_width(120.0),
            );
            if widgets::action_button(ui, "Search", widgets::ActionStyle::Primary, !state.busy())
                .clicked()
            {
                action = Some(CheatBasePageAction::Search);
            }
        });
        if let Some(Err(error)) = &state.search {
            widgets::banner(
                ui,
                "Search could not be completed",
                &human_error(error),
                widgets::StatusTone::Warning,
            );
            widgets::technical_details(ui, "cheatbase-search-error", |ui| {
                ui.label(error);
            });
        }
        if let Some(Ok(result)) = &state.search {
            ui.label(format!(
                "{} result{}",
                result.page.total,
                if result.page.total == 1 { "" } else { "s" }
            ));
            ui.label(search_explanation(result));
            for game in &result.page.rows {
                widgets::card(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(&game.title);
                        widgets::status_badge(
                            ui,
                            confidence_label(game.match_confidence),
                            confidence_tone(game.match_confidence),
                        );
                    });
                    ui.label(format!(
                        "{} · {}",
                        game.platform
                            .archivefs_platform_display_name
                            .as_deref()
                            .unwrap_or(&game.upstream_system_name),
                        display_region(game)
                    ));
                    ui.label(format!(
                        "Available cheats: {}",
                        game.cheat_count.unwrap_or(0)
                    ));
                    if game.platform_has_cheat_coverage {
                        ui.label("Nintendo DS Action Replay records can be viewed here.");
                    } else {
                        ui.weak(&game.cheat_coverage_note);
                    }
                    if widgets::action_button(
                        ui,
                        "Select game",
                        widgets::ActionStyle::Secondary,
                        !state.busy(),
                    )
                    .clicked()
                    {
                        action = Some(CheatBasePageAction::Inspect(game.upstream_release_id));
                    }
                    widgets::technical_details(
                        ui,
                        ("cheatbase-game", game.upstream_release_id),
                        |ui| {
                            ui.label(format!(
                                "CheatBase release ID: {}",
                                game.upstream_release_id
                            ));
                            ui.label(format!("CheatBase ROM ID: {}", game.upstream_rom_id));
                            if let Some(serial) = &game.serial {
                                ui.label(format!("Serial: {serial}"));
                            }
                            if let Some(crc) = &game.crc32 {
                                ui.label(format!("CRC32: {crc}"));
                            }
                            if let Some(md5) = &game.md5 {
                                ui.label(format!("MD5: {md5}"));
                            }
                            if let Some(sha1) = &game.sha1 {
                                ui.label(format!("SHA-1: {sha1}"));
                            }
                            for evidence in &game.match_evidence {
                                ui.label(format!("Evidence: {evidence}"));
                            }
                        },
                    );
                });
            }
        }
    }

    if state.busy() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Working with CheatBase…");
        });
    }

    if let Some(game) = &state.selected_game {
        ui.add_space(theme::SECTION_GAP);
        widgets::section_header(
            ui,
            "Selected game",
            Some("Inspect the available records and their source attribution."),
        );
        widgets::card(ui, |ui| {
            ui.strong(&game.title);
            ui.label(
                game.platform
                    .archivefs_platform_display_name
                    .as_deref()
                    .unwrap_or(&game.upstream_system_name),
            );
            ui.label(format!(
                "{} · {}",
                display_region(game),
                if game.revision_verified {
                    "Revision identified"
                } else {
                    "Revision not verified"
                }
            ));
            let count = state.selected_cheats.as_ref().map_or(0, |page| page.total);
            ui.label(format!("Available cheats: {count}"));
            ui.label(BROWSE_ONLY_COPY);
            widgets::technical_details(
                ui,
                ("cheatbase-selected-game", game.upstream_release_id),
                |ui| {
                    ui.label(format!("Release ID: {}", game.upstream_release_id));
                    ui.label(format!("ROM ID: {}", game.upstream_rom_id));
                    ui.label(format!(
                        "Match evidence: {}",
                        if game.match_evidence.is_empty() {
                            "none".to_string()
                        } else {
                            game.match_evidence.join("; ")
                        }
                    ));
                    if let Some(serial) = &game.serial {
                        ui.label(format!("Serial: {serial}"));
                    }
                    if let Some(crc) = &game.crc32 {
                        ui.label(format!("CRC32: {crc}"));
                    }
                },
            );
        });
        if let Some(error) = &state.inspection_error {
            widgets::banner(
                ui,
                "Game details could not be loaded",
                &human_error(error),
                widgets::StatusTone::Warning,
            );
            widgets::technical_details(ui, "cheatbase-inspection-error", |ui| {
                ui.label(error);
            });
        }
        if let Some(cheats) = &state.selected_cheats {
            for cheat in &cheats.rows {
                widgets::card(ui, |ui| {
                    ui.strong(&cheat.name);
                    ui.label(format!("{} · {}", cheat.category, cheat.device.format));
                    if let Some(description) = &cheat.description {
                        ui.label(description);
                    }
                    if let Some(notes) = &cheat.side_effect {
                        ui.label(format!("Note: {notes}"));
                    }
                    ui.label(format!(
                        "Source: {}",
                        cheat.credit.as_deref().unwrap_or("CheatBase")
                    ));
                    ui.weak(BROWSE_ONLY_COPY);
                    widgets::technical_details(ui, ("cheatbase-cheat", cheat.upstream_id), |ui| {
                        ui.label(format!("CheatBase cheat ID: {}", cheat.upstream_id));
                        ui.label(format!("Category ID: {}", cheat.category_id));
                        ui.label(format!("Device: {}", cheat.device.name));
                        if let Some(activation) = &cheat.activation {
                            ui.label(format!("Activation: {activation}"));
                        }
                        ui.label("Raw Action Replay content is viewable only:");
                        ui.code(&cheat.code);
                        if !cheat.truncated_fields.is_empty() {
                            ui.label(format!(
                                "Truncated fields: {}",
                                cheat.truncated_fields.join(", ")
                            ));
                        }
                    });
                });
            }
        }
        widgets::technical_details(ui, "cheatbase-attribution", |ui| {
            if let Some(Ok(status)) = &state.status {
                ui.label(format!("Provider: {}", status.provenance.source));
                ui.label(format!("Maintainer: {}", status.provenance.maintainer));
                ui.label(format!("Origin: {}", status.provenance.origin));
                ui.label(format!(
                    "Upstream project: {}",
                    status.provider.upstream_project
                ));
                ui.label(format!(
                    "Pinned commit: {}",
                    archivefs_core::patch_manager::CHEATBASE_UPSTREAM_COMMIT
                ));
            }
        });
    }
    action
}

fn source_state_presentation(
    status: Option<&Result<CheatBaseSourceStatus, String>>,
) -> (&'static str, widgets::StatusTone) {
    match status {
        None => ("Checking status", widgets::StatusTone::Pending),
        Some(Err(_)) => ("Source unavailable", widgets::StatusTone::Blocked),
        Some(Ok(status)) if status.usable => ("Ready", widgets::StatusTone::Success),
        Some(Ok(status)) => match status.state {
            CheatProviderSourceState::Invalid
            | CheatProviderSourceState::ValidationFailed
            | CheatProviderSourceState::UnsupportedSchema => {
                ("Invalid catalogue", widgets::StatusTone::Blocked)
            }
            CheatProviderSourceState::Disabled => {
                ("Source unavailable", widgets::StatusTone::Warning)
            }
            _ => ("Not set up", widgets::StatusTone::Pending),
        },
    }
}

fn confidence_label(confidence: Option<ProviderGameMatchConfidence>) -> &'static str {
    match confidence {
        Some(
            ProviderGameMatchConfidence::ExactHashPlatform
            | ProviderGameMatchConfidence::ExactSerialPlatformRegion
            | ProviderGameMatchConfidence::ExactUpstreamRelease
            | ProviderGameMatchConfidence::ExactTitlePlatformRegionRevision,
        ) => "Exact match",
        Some(ProviderGameMatchConfidence::ExactTitlePlatform) => "Strong match",
        Some(ProviderGameMatchConfidence::ProbableTitlePlatform) => "Possible match",
        Some(ProviderGameMatchConfidence::Ambiguous) => "Choose a result",
        Some(ProviderGameMatchConfidence::NoMatch) | None => "Browse only",
    }
}

fn confidence_tone(confidence: Option<ProviderGameMatchConfidence>) -> widgets::StatusTone {
    match confidence {
        Some(
            ProviderGameMatchConfidence::ExactHashPlatform
            | ProviderGameMatchConfidence::ExactSerialPlatformRegion
            | ProviderGameMatchConfidence::ExactUpstreamRelease
            | ProviderGameMatchConfidence::ExactTitlePlatformRegionRevision,
        ) => widgets::StatusTone::Success,
        Some(ProviderGameMatchConfidence::ExactTitlePlatform) => widgets::StatusTone::Info,
        Some(
            ProviderGameMatchConfidence::ProbableTitlePlatform
            | ProviderGameMatchConfidence::Ambiguous,
        ) => widgets::StatusTone::Warning,
        Some(ProviderGameMatchConfidence::NoMatch) | None => widgets::StatusTone::Pending,
    }
}

fn display_region(game: &CheatBaseGame) -> &str {
    if !game.release_region.is_empty() {
        &game.release_region
    } else {
        &game.rom_region
    }
}

fn human_error(error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("not installed") {
        "CheatBase is not set up yet. Download or import a validated catalogue.".to_string()
    } else if lower.contains("requires a title") {
        "Enter a game title before searching.".to_string()
    } else if lower.contains("hash") || lower.contains("sha-") || lower.contains("integrity") {
        "The catalogue failed its integrity check. Import a trusted pinned copy and validate it again.".to_string()
    } else if lower.contains("unsupported") || lower.contains("schema") {
        "This catalogue format is not supported. Choose the pinned CheatBase source.".to_string()
    } else if lower.contains("permission") || lower.contains("unreadable") {
        "CheatBase could not read the local catalogue. Check its access and try again.".to_string()
    } else {
        "CheatBase reported a problem. Open Technical details for the exact reason.".to_string()
    }
}

fn search_explanation(result: &archivefs_core::patch_manager::CheatBaseGameSearchResult) -> String {
    let base = match result.confidence {
        ProviderGameMatchConfidence::Ambiguous => {
            "Several results match. Select the game you want to inspect.".to_string()
        }
        ProviderGameMatchConfidence::NoMatch => "No matching games were found.".to_string(),
        ProviderGameMatchConfidence::ExactHashPlatform
        | ProviderGameMatchConfidence::ExactSerialPlatformRegion
        | ProviderGameMatchConfidence::ExactUpstreamRelease
        | ProviderGameMatchConfidence::ExactTitlePlatformRegionRevision => {
            "The catalogue found an exact match.".to_string()
        }
        ProviderGameMatchConfidence::ExactTitlePlatform => {
            "The catalogue found a strong title and system match.".to_string()
        }
        ProviderGameMatchConfidence::ProbableTitlePlatform => {
            "The catalogue found a possible title and system match.".to_string()
        }
    };
    if result.explanation.contains("identity-metadata") {
        format!("{base} This record has identity information but no CheatBase cheats.")
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    // These tests build a default page and then set the one or two fields
    // the scenario needs; a full struct literal per case would obscure
    // which field each test actually exercises.
    #![allow(clippy::field_reassign_with_default)]
    use super::*;

    fn rendered_text_contains(output: &egui::FullOutput, needle: &str) -> bool {
        fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
            match shape {
                egui::Shape::Text(text) => text.galley.text().contains(needle),
                egui::Shape::Vec(nested) => {
                    nested.iter().any(|shape| shape_contains(shape, needle))
                }
                _ => false,
            }
        }
        output
            .shapes
            .iter()
            .any(|clipped| shape_contains(&clipped.shape, needle))
    }

    fn render(state: &mut CheatBasePageState) -> egui::FullOutput {
        let context = egui::Context::default();
        context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                let _ = show_cheatbase_page(ui, state, None);
            });
        })
    }

    fn status(usable: bool, state: CheatProviderSourceState) -> CheatBaseSourceStatus {
        CheatBaseSourceStatus {
            format_version: 1,
            provider: archivefs_core::patch_manager::cheatbase_provider_identity(),
            state,
            enabled: true,
            usable,
            database_path: PathBuf::from("/private/catalogue.sqlite"),
            fingerprint: None,
            source_fingerprint: None,
            validation: None,
            last_error: None,
            provenance: archivefs_core::patch_manager::cheatbase_provenance(),
            licence: archivefs_core::patch_manager::cheatbase_licence(),
            licence_status:
                archivefs_core::patch_manager::CheatProviderLicenceStatus::NotEstablished,
            cheat_coverage_platforms: vec!["Nintendo DS".to_string()],
            identity_metadata_platforms: Vec::new(),
            browse_only: true,
            install_supported: false,
        }
    }

    fn game() -> CheatBaseGame {
        CheatBaseGame {
            upstream_release_id: 77,
            upstream_rom_id: 88,
            title: "Example DS Game".to_string(),
            platform: archivefs_core::patch_manager::ProviderPlatformMapping {
                upstream_id: 24,
                upstream_name: "Nintendo DS".to_string(),
                archivefs_platform_id: Some("nintendo-ds".to_string()),
                archivefs_platform_display_name: Some("Nintendo DS".to_string()),
                status: archivefs_core::patch_manager::PlatformMappingStatus::Exact,
                explanation: "Exact canonical mapping".to_string(),
            },
            upstream_system_name: "Nintendo DS".to_string(),
            rom_region: "USA".to_string(),
            release_region: "USA".to_string(),
            serial: Some("NTR-EXAMPLE".to_string()),
            crc32: Some("1234ABCD".to_string()),
            md5: None,
            sha1: None,
            rom_size: None,
            release_date: None,
            cheat_count: Some(1),
            platform_has_cheat_coverage: true,
            cheat_coverage_note: "Action Replay DS; browse only".to_string(),
            cheat_device_formats: vec!["Action Replay DS".to_string()],
            match_confidence: Some(ProviderGameMatchConfidence::ExactTitlePlatform),
            match_evidence: vec!["title and system".to_string()],
            revision_verified: false,
        }
    }

    fn cheat() -> CheatBaseCheat {
        CheatBaseCheat {
            upstream_id: 99,
            name: "Infinite lives".to_string(),
            activation: Some("Activate in game".to_string()),
            description: Some("Keeps the player alive.".to_string()),
            side_effect: None,
            folder: None,
            category_id: 1,
            category: "Gameplay".to_string(),
            category_description: None,
            code: "RAW ACTION REPLAY CODE".to_string(),
            device: archivefs_core::patch_manager::CheatBaseDeviceSummary {
                upstream_id: 10,
                name: "Action Replay DS".to_string(),
                format: "Action Replay DS".to_string(),
                compatibility:
                    archivefs_core::patch_manager::DeviceFormatCompatibility::ReferenceOnly,
            },
            credit: Some("Community contributor".to_string()),
            truncated_fields: Vec::new(),
        }
    }

    #[test]
    fn source_states_use_beginner_wording() {
        assert_eq!(source_state_presentation(None).0, "Checking status");
        assert_eq!(
            source_state_presentation(Some(&Err("x".to_string()))).0,
            "Source unavailable"
        );
        let mut status = CheatBaseSourceStatus {
            format_version: 1,
            provider: archivefs_core::patch_manager::cheatbase_provider_identity(),
            state: CheatProviderSourceState::NotInstalled,
            enabled: true,
            usable: false,
            database_path: PathBuf::from("/private/catalogue.sqlite"),
            fingerprint: None,
            source_fingerprint: None,
            validation: None,
            last_error: None,
            provenance: archivefs_core::patch_manager::cheatbase_provenance(),
            licence: archivefs_core::patch_manager::cheatbase_licence(),
            licence_status:
                archivefs_core::patch_manager::CheatProviderLicenceStatus::NotEstablished,
            cheat_coverage_platforms: vec!["Nintendo DS".to_string()],
            identity_metadata_platforms: Vec::new(),
            browse_only: true,
            install_supported: false,
        };
        assert_eq!(
            source_state_presentation(Some(&Ok(status.clone()))).0,
            "Not set up"
        );
        status.usable = true;
        status.state = CheatProviderSourceState::Ready;
        assert_eq!(source_state_presentation(Some(&Ok(status))).0, "Ready");
    }

    #[test]
    fn confidence_is_never_stronger_than_backend_value() {
        assert_eq!(
            confidence_label(Some(ProviderGameMatchConfidence::ExactHashPlatform)),
            "Exact match"
        );
        assert_eq!(
            confidence_label(Some(ProviderGameMatchConfidence::ExactTitlePlatform)),
            "Strong match"
        );
        assert_eq!(
            confidence_label(Some(ProviderGameMatchConfidence::ProbableTitlePlatform)),
            "Possible match"
        );
        assert_eq!(
            confidence_label(Some(ProviderGameMatchConfidence::Ambiguous)),
            "Choose a result"
        );
        assert_eq!(
            confidence_label(Some(ProviderGameMatchConfidence::NoMatch)),
            "Browse only"
        );
    }

    #[test]
    fn state_starts_without_install_or_apply_actions() {
        let state = CheatBasePageState::default();
        assert!(!state.source_is_usable());
        assert!(!state.busy());
        assert_eq!(
            BROWSE_ONLY_COPY,
            "Browse only — installation from CheatBase is not enabled yet."
        );
    }

    #[test]
    fn selected_game_seed_only_fills_empty_search_fields() {
        let mut state = CheatBasePageState::default();
        state.seed_from_selected(Some(CheatBaseGameSeed {
            title: "Example".to_string(),
            platform: Some("Nintendo DS".to_string()),
            region: Some("USA".to_string()),
        }));
        assert_eq!(state.title_query, "Example");
        assert_eq!(state.platform_query, "Nintendo DS");
        assert_eq!(state.region_query, "USA");
        state.title_query = "Different".to_string();
        state.seed_from_selected(Some(CheatBaseGameSeed {
            title: "Other".to_string(),
            platform: Some("Nintendo DS".to_string()),
            region: Some("EUR".to_string()),
        }));
        assert_eq!(state.title_query, "Different");
    }

    #[test]
    fn source_card_is_understandable_when_not_configured() {
        let mut state = CheatBasePageState::default();
        state.status = Some(Ok(status(false, CheatProviderSourceState::NotInstalled)));
        let output = render(&mut state);
        assert!(rendered_text_contains(&output, "CheatBase"));
        assert!(rendered_text_contains(&output, "Not set up"));
        assert!(rendered_text_contains(&output, "Download pinned source"));
        assert!(rendered_text_contains(&output, "Import local database"));
    }

    #[test]
    fn ready_source_shows_browse_controls_and_keeps_paths_hidden() {
        let mut state = CheatBasePageState::default();
        state.status = Some(Ok(status(true, CheatProviderSourceState::Ready)));
        let output = render(&mut state);
        assert!(rendered_text_contains(&output, "Ready"));
        assert!(rendered_text_contains(&output, "Search games"));
        assert!(rendered_text_contains(&output, "Technical details"));
        assert!(!rendered_text_contains(
            &output,
            "/private/catalogue.sqlite"
        ));
        assert!(!rendered_text_contains(&output, "Install"));
        assert!(!rendered_text_contains(&output, "Apply"));
    }

    #[test]
    fn search_results_lead_with_game_details_not_internal_ids() {
        let mut state = CheatBasePageState::default();
        state.status = Some(Ok(status(true, CheatProviderSourceState::Ready)));
        let selected = game();
        state.search = Some(Ok(
            archivefs_core::patch_manager::CheatBaseGameSearchResult {
                confidence: ProviderGameMatchConfidence::Ambiguous,
                explanation: "Multiple plausible CheatBase releases require user selection"
                    .to_string(),
                page: ProviderPage {
                    offset: 0,
                    limit: 50,
                    total: 2,
                    rows: vec![selected],
                    has_more: false,
                },
            },
        ));
        let output = render(&mut state);
        assert!(rendered_text_contains(&output, "Example DS Game"));
        assert!(rendered_text_contains(&output, "Nintendo DS"));
        assert!(rendered_text_contains(&output, "Select game"));
        assert!(
            state.selected_game.is_none(),
            "ambiguous results must not be auto-selected"
        );
        assert!(!rendered_text_contains(&output, "CheatBase release ID: 77"));
        assert!(!rendered_text_contains(&output, "1234ABCD"));
    }

    #[test]
    fn selected_game_shows_count_attribution_and_browse_only_ds_truth() {
        let mut state = CheatBasePageState::default();
        state.status = Some(Ok(status(true, CheatProviderSourceState::Ready)));
        state.selected_game = Some(game());
        state.selected_cheats = Some(ProviderPage {
            offset: 0,
            limit: 100,
            total: 1,
            rows: vec![cheat()],
            has_more: false,
        });
        let output = render(&mut state);
        assert!(rendered_text_contains(&output, "Available cheats: 1"));
        assert!(rendered_text_contains(&output, "Infinite lives"));
        assert!(rendered_text_contains(&output, "Community contributor"));
        assert!(rendered_text_contains(&output, BROWSE_ONLY_COPY));
        assert!(!rendered_text_contains(&output, "RAW ACTION REPLAY CODE"));
        assert!(!rendered_text_contains(&output, "CheatBase cheat ID: 99"));
    }

    #[test]
    fn invalid_source_has_safe_state_and_actionable_copy() {
        let mut state = CheatBasePageState::default();
        state.status = Some(Ok(status(false, CheatProviderSourceState::Invalid)));
        let output = render(&mut state);
        assert!(rendered_text_contains(&output, "Invalid catalogue"));
        assert!(rendered_text_contains(&output, "Validate it again"));
        assert!(!rendered_text_contains(&output, "Search games"));
    }
}
