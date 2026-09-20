//! GUI v2 is a parallel presentation layer, not a mode of the legacy app.
mod activity;
mod artwork;
mod backend;
mod legacy;
mod library;
mod media_sources;
mod mods;
mod native_workflows;
mod pages;
mod problems;
mod routes;
#[cfg(test)]
mod tests;
mod thumbnail;

use crate::playing_library_page::{PlayingLibraryPageAction, PlayingLibraryPageState};
use activity::Activity;
use artwork::Artwork;
use mods::ModsPageState;
use backend::{
    Backend, Command, DuplicateRepairPreview, DuplicateRepairRecord, Event, Payload, Preferences,
    VerificationResult,
};
use eframe::egui;
use library::{Detail, DuplicateReport, Filter, Library, SharedLibrary};
use routes::{Route, Router, Section};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// Run the independent GUI-v2 executable. Existing entry points remain legacy.
pub fn run() -> eframe::Result<()> {
    crate::init_logging();
    let args: Vec<_> = std::env::args_os().collect();
    if args.iter().any(|arg| arg == "--version") {
        println!("{} · GUI v2 milestone 1", crate::gui_version_line());
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--measure-library") {
        match archivefs_core::default_database_path()
            .map_err(|error| error.to_string())
            .and_then(|path| backend::load_library(&path))
        {
            Ok(library) => {
                println!(
                    "GUI v2 read-only load: {} games, {} platforms, {} ms",
                    library.games.len(),
                    library.platforms.len(),
                    library.load_ms
                );
                let index = media_sources::MediaIndex::discover(&library);
                println!(
                    "GUI v2 read-only artwork index: {} covers, {} screenshot groups, {} ms; no artwork network requests",
                    index.covers.len(),
                    index.screenshots.len(),
                    index.elapsed_ms
                );
            }
            Err(error) => eprintln!("Library measurement failed: {error}"),
        }
        return Ok(());
    }
    let handoff = args.iter().position(|arg| arg == "--legacy").map(|index| {
        let section = args
            .get(index + 1)
            .and_then(|arg| serde_json::from_str(&arg.to_string_lossy()).ok())
            .unwrap_or(Section::Advanced);
        (section, args.get(index + 2).map(std::path::PathBuf::from))
    });
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([620.0, 480.0])
        .with_app_id("org.emuwiz.gui-v2");
    if let Some(icon) = crate::app_icon() {
        viewport = viewport.with_icon(icon);
    }
    eframe::run_native(
        if handoff.is_some() {
            "EmuWiz — Legacy / Advanced interface"
        } else {
            "EmuWiz — GUI v2"
        },
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(move |context| {
            if let Some((section, path)) = handoff {
                Ok(Box::new(legacy::LegacyHost::new(
                    context.egui_ctx.clone(),
                    section,
                    path,
                )))
            } else {
                Ok(Box::new(App::new(context.egui_ctx.clone())))
            }
        }),
    )
}

pub(super) fn readable_style(context: &egui::Context) {
    let mut style = (*context.style()).clone();
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(18.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(18.0));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, egui::FontId::proportional(27.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, egui::FontId::proportional(16.0));
    style.spacing.item_spacing = egui::vec2(12.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 9.0);
    style.spacing.interact_size.y = 40.0;
    style.visuals = egui::Visuals::dark();
    style.visuals.override_text_color = Some(egui::Color32::from_rgb(232, 237, 244));
    style.visuals.selection.bg_fill = egui::Color32::from_rgb(35, 91, 147);
    style.visuals.widgets.active.bg_stroke = egui::Stroke::new(2.0_f32, egui::Color32::WHITE);
    context.set_style(style);
}

struct Notice {
    message: String,
    technical: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlayingLibraryJobKind {
    Preview,
    Apply,
}

#[derive(Clone, Debug)]
struct PlayingLibraryJob {
    id: u64,
    kind: PlayingLibraryJobKind,
    generation: u64,
    input_fingerprint: String,
}

pub(super) struct App {
    router: Router,
    backend: Backend,
    library: SharedLibrary,
    indices: Vec<usize>,
    filter: Filter,
    filter_generation: u64,
    filter_inflight: bool,
    filter_dirty: Option<Instant>,
    detail: Option<Detail>,
    detail_pending: Option<i64>,
    detail_generation: u64,
    detail_failed: Option<i64>,
    activity: Activity,
    artwork: Artwork,
    load_job: Option<u64>,
    artwork_job: Option<u64>,
    index_job: Option<u64>,
    preferences_dirty: Option<Instant>,
    interacted: bool,
    loaded: bool,
    notice: Option<Notice>,
    confirm_scan: bool,
    screenshots: bool,
    check_platform: Option<String>,
    verification: Option<VerificationResult>,
    verification_job: Option<u64>,
    duplicate_report: Option<DuplicateReport>,
    duplicate_job: Option<u64>,
    duplicate_ignored: std::collections::HashSet<String>,
    problem_selected: Option<String>,
    repair_preview: Option<DuplicateRepairPreview>,
    repair_confirm: bool,
    repair_job: Option<u64>,
    repair_history: Vec<DuplicateRepairRecord>,
    undo_confirm: Option<usize>,
    undo_job: Option<u64>,
    repair_result: Option<String>,
    playing_library: PlayingLibraryPageState,
    playing_library_history: Vec<archivefs_core::dat::rename_apply::model::RenameTransaction>,
    playing_library_job: Option<PlayingLibraryJob>,
    playing_library_generation: u64,
    mods: ModsPageState,
    native_workflows: Option<native_workflows::NativeWorkflows>,
    mrwiz_dismissed: bool,
}

impl App {
    fn new(context: egui::Context) -> Self {
        readable_style(&context);
        let backend = Backend::start(context.clone());
        let mut app = Self {
            router: Router::default(),
            backend,
            library: Arc::new(Library::default()),
            indices: Vec::new(),
            filter: Filter::default(),
            filter_generation: 0,
            filter_inflight: false,
            filter_dirty: None,
            detail: None,
            detail_pending: None,
            detail_generation: 0,
            detail_failed: None,
            activity: Activity::default(),
            artwork: Artwork::start(context),
            load_job: None,
            artwork_job: None,
            index_job: None,
            preferences_dirty: None,
            interacted: false,
            loaded: false,
            notice: None,
            confirm_scan: false,
            screenshots: false,
            check_platform: None,
            verification: None,
            verification_job: None,
            duplicate_report: None,
            duplicate_job: None,
            duplicate_ignored: std::collections::HashSet::new(),
            problem_selected: None,
            repair_preview: None,
            repair_confirm: false,
            repair_job: None,
            repair_history: Vec::new(),
            undo_confirm: None,
            undo_job: None,
            repair_result: None,
            playing_library: PlayingLibraryPageState::load(),
            playing_library_history: Vec::new(),
            playing_library_job: None,
            playing_library_generation: 0,
            mods: ModsPageState::default(),
            native_workflows: None,
            mrwiz_dismissed: false,
        };
        app.send(0, Command::Restore);
        app.send(0, Command::LoadRepairHistory);
        app.load(false);
        app
    }
    fn send(&mut self, id: u64, command: Command) -> bool {
        if let Err(error) = self.backend.send(id, command) {
            self.activity.finish(
                id,
                "This action could not start. Reopen GUI v2 to retry.".into(),
                Some(error.clone()),
            );
            self.notice = Some(Notice {
                message: error,
                technical: String::new(),
            });
            false
        } else {
            true
        }
    }
    fn load(&mut self, scan: bool) {
        if self.load_job.is_some() {
            return;
        }
        let id = self.activity.queue(
            if scan {
                "Scanning your game folders"
            } else {
                "Loading your library"
            },
            Route::Section(Section::Games),
            false,
        );
        self.load_job = Some(id);
        self.send(id, Command::Load { scan });
    }
    fn go(&mut self, route: Route) {
        self.router.go(route);
        self.navigation_changed();
    }
    fn navigation_changed(&mut self) {
        self.interacted = true;
        self.preferences_dirty = Some(Instant::now());
        self.screenshots = false;
        self.detail_generation += 1;
        self.detail_failed = None;
        // Cancelled artwork remains resumable; entering a game/browser resumes it.
        self.artwork.paused = false;
    }
    fn back(&mut self) {
        self.router.back();
        self.navigation_changed();
    }
    fn change_filter(&mut self) {
        self.filter_generation += 1;
        self.filter_dirty = Some(Instant::now());
        self.interacted = true;
        self.preferences_dirty = Some(Instant::now());
    }
    fn check_platform(&mut self, platform: String) {
        self.check_platform = Some(platform);
        self.verification = None;
        self.go(Route::Section(Section::Check));
    }
    fn start_verification(&mut self) {
        let Some(platform) = self.check_platform.clone() else {
            return;
        };
        let games = self
            .library
            .games
            .iter()
            .filter(|game| game.platform == platform)
            .cloned()
            .collect::<Vec<_>>();
        let id = self.activity.queue(
            &format!("Checking {platform}"),
            Route::Section(Section::Check),
            true,
        );
        let cancel = self
            .activity
            .jobs
            .get(&id)
            .and_then(|job| job.cancel.clone())
            .unwrap_or_else(|| Arc::new(std::sync::atomic::AtomicBool::new(false)));
        self.verification_job = Some(id);
        self.send(
            id,
            Command::Verify {
                platform,
                games,
                cancel,
            },
        );
    }
    fn start_duplicate_scan(&mut self) {
        if self.duplicate_job.is_some() {
            return;
        }
        let id = self.activity.queue(
            "Finding exact duplicate files",
            Route::Section(Section::Duplicates),
            true,
        );
        self.duplicate_job = Some(id);
        self.send(
            id,
            Command::ScanDuplicates {
                games: self.library.games.clone(),
            },
        );
    }
    fn start_duplicate_preview(&mut self, index: usize) {
        let Some(report) = self.duplicate_report.as_ref() else {
            return;
        };
        let Some(group) = report.exact_groups.get(index).cloned() else {
            return;
        };
        let id = self.activity.queue(
            "Preparing the duplicate quarantine preview",
            Route::Section(Section::Problems),
            false,
        );
        self.repair_preview = None;
        self.repair_result = None;
        self.send(
            id,
            Command::PrepareDuplicateRepair {
                group: Box::new(group),
            },
        );
    }
    fn apply_duplicate_preview(&mut self) {
        if self.repair_job.is_some() {
            return;
        }
        let Some(preview) = self.repair_preview.clone() else {
            return;
        };
        let id = self.activity.queue(
            "Applying the approved duplicate quarantine",
            Route::Section(Section::History),
            true,
        );
        let cancel = self
            .activity
            .jobs
            .get(&id)
            .and_then(|job| job.cancel.clone())
            .unwrap_or_else(|| Arc::new(std::sync::atomic::AtomicBool::new(false)));
        self.repair_job = Some(id);
        self.repair_confirm = false;
        self.send(
            id,
            Command::ApplyDuplicateRepair {
                preview: Box::new(preview),
                cancel,
            },
        );
    }
    fn undo_history_entry(&mut self, index: usize) {
        if self.undo_job.is_some() {
            return;
        }
        let Some(record) = self.repair_history.get(index).cloned() else {
            return;
        };
        let id = self.activity.queue(
            "Undoing the duplicate quarantine",
            Route::Section(Section::History),
            false,
        );
        self.undo_job = Some(id);
        self.undo_confirm = None;
        self.send(
            id,
            Command::UndoDuplicateRepair {
                record: Box::new(record),
                cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
        );
    }
    fn legacy(&mut self, section: Section) {
        let path = self
            .router
            .current
            .game()
            .and_then(|id| self.library.game(id))
            .map(|game| game.archive.absolute_path.clone());
        let id = self.activity.queue(
            "Opening the existing workflow",
            self.router.current.clone(),
            false,
        );
        self.send(id, Command::Legacy { section, path });
    }

    fn start_playing_library_job(&mut self, kind: PlayingLibraryJobKind) {
        if self.playing_library_job.is_some() {
            return;
        }
        self.playing_library_generation = self.playing_library_generation.wrapping_add(1);
        let generation = self.playing_library_generation;
        let input_fingerprint = self.playing_library.input_fingerprint();
        let id = self.activity.queue(
            match kind {
                PlayingLibraryJobKind::Preview => "Planning your playing library",
                PlayingLibraryJobKind::Apply => "Building your playing library",
            },
            Route::Section(Section::Build),
            false,
        );
        self.playing_library_job = Some(PlayingLibraryJob {
            id,
            kind,
            generation,
            input_fingerprint,
        });
        let state = Box::new(self.playing_library.clone());
        let command = match kind {
            PlayingLibraryJobKind::Preview => Command::PlayingLibraryPreview { state, generation },
            PlayingLibraryJobKind::Apply => Command::PlayingLibraryApply { state, generation },
        };
        if !self.send(id, command) {
            self.playing_library_job = None;
        }
    }

    fn handle_playing_library_action(&mut self, action: PlayingLibraryPageAction) {
        use PlayingLibraryPageAction::*;
        match action {
            Preview => {
                self.start_playing_library_job(PlayingLibraryJobKind::Preview);
            }
            SelectFamily(name) => self.playing_library.select_family(name),
            RequestApply => self.playing_library.request_apply(),
            CancelApply => self.playing_library.cancel_apply(),
            ConfirmApply => {
                self.start_playing_library_job(PlayingLibraryJobKind::Apply);
            }
            RollbackLast => {
                let id = self.activity.queue(
                    "Undoing the playing library build",
                    Route::Section(Section::History),
                    false,
                );
                self.playing_library.rollback_last();
                if let Some(error) = self.playing_library.apply_error().map(str::to_owned) {
                    self.activity.finish(
                        id,
                        "Undo was refused because the destination is no longer in the expected state.".into(),
                        Some(error),
                    );
                } else {
                    if let Some(transaction) = self.playing_library.applied().cloned()
                        && let Some(existing) = self
                            .playing_library_history
                            .iter_mut()
                            .rev()
                            .find(|item| item.transaction_id == transaction.transaction_id)
                    {
                        *existing = transaction;
                    }
                    self.activity
                        .finish(id, "Playing library build undone safely.".into(), None);
                }
            }
            // These destinations are intentionally kept in the existing
            // specialised page. The v2 generic Build Library journey does
            // not expose frontend-specific publication workflows.
            SelectEsdePlatform(_)
            | PreviewEsde
            | RequestEsdePublish
            | CancelEsdePublish
            | ConfirmEsdePublish
            | RequestEsdeRecovery
            | CancelEsdeRecovery
            | ConfirmEsdeRecovery
            | PreviewRomm
            | RollbackRomm
            | PreviewRetroDeck
            | RollbackRetroDeck => {}
        }
    }

    fn finish_playing_library_job(
        &mut self,
        kind: PlayingLibraryJobKind,
        state: Box<PlayingLibraryPageState>,
        generation: u64,
    ) {
        let Some(job) = self.playing_library_job.take() else {
            return;
        };
        if job.kind != kind || job.generation != generation {
            return;
        }
        match kind {
            PlayingLibraryJobKind::Preview => {
                if generation != self.playing_library_generation
                    || job.input_fingerprint != self.playing_library.input_fingerprint()
                {
                    self.activity.finish(
                        job.id,
                        "The preview was discarded because the library settings changed. Plan again to review the new settings.".into(),
                        None,
                    );
                    return;
                }
                self.playing_library = *state;
                if let Some(error) = self.playing_library.error().map(str::to_owned) {
                    self.activity.finish(
                        job.id,
                        "The preview could not be completed. Nothing was changed.".into(),
                        Some(error.clone()),
                    );
                    self.notice = Some(Notice {
                        message: "The playing-library preview needs attention.".into(),
                        technical: error,
                    });
                } else {
                    self.activity.finish(
                        job.id,
                        "Preview ready. Review the selected games before creating links.".into(),
                        None,
                    );
                }
            }
            PlayingLibraryJobKind::Apply => {
                let transaction = state.applied().cloned();
                let error = state.apply_error().map(str::to_owned);
                self.playing_library.merge_async_apply_result(*state);
                if let Some(error) = error {
                    self.activity.finish(
                        job.id,
                        "The playing library was not completed. Your original collection remains unchanged.".into(),
                        Some(error),
                    );
                } else if let Some(transaction) = transaction {
                    if !self
                        .playing_library_history
                        .iter()
                        .any(|item| item.transaction_id == transaction.transaction_id)
                    {
                        self.playing_library_history.push(transaction.clone());
                    }
                    self.activity.finish(
                        job.id,
                        format!(
                            "Playing library created: {} links. Your original collection was not changed.",
                            transaction.applied_count()
                        ),
                        None,
                    );
                } else {
                    self.activity
                        .finish(job.id, "No playing-library changes were made.".into(), None);
                }
            }
        }
    }

    fn invalidate_changed_playing_library_plan(&mut self) {
        let Some(job) = self.playing_library_job.as_ref() else {
            return;
        };
        if job.kind == PlayingLibraryJobKind::Preview
            && job.generation == self.playing_library_generation
            && job.input_fingerprint != self.playing_library.input_fingerprint()
        {
            // The worker owns a snapshot. Once the user edits any planning
            // input, its eventual result must not be accepted—even if the
            // user changes the value back before the worker replies.
            self.playing_library_generation = self.playing_library_generation.wrapping_add(1);
        }
    }

    fn poll(&mut self, context: &egui::Context) {
        self.artwork.begin_frame(context);
        for _ in 0..32 {
            let Ok(event) = self.backend.rx.try_recv() else {
                break;
            };
            match event {
                Event::Started(id) => self.activity.start(id),
                Event::Progress {
                    id,
                    done,
                    total,
                    item,
                } => {
                    if let Some(job) = self.activity.jobs.get_mut(&id) {
                        job.progress = Some((done, total));
                        job.item = Some(item);
                    }
                }
                Event::Finished { id, outcome } => {
                    if self.load_job == Some(id) {
                        self.load_job = None;
                    }
                    match outcome {
                        Ok(payload) => {
                            self.activity.finish(
                                id,
                                "Complete. Open the result to continue.".into(),
                                None,
                            );
                            match payload {
                                Payload::Library(library) => {
                                    self.activity.finish(id, format!("{} games available to browse. Original game files were not changed.", library.games.len()), None);
                                    if let Some(warning) = &library.scan_warning {
                                        self.activity.finish(
                                            id,
                                            warning.clone(),
                                            Some(warning.clone()),
                                        );
                                        self.notice = Some(Notice {
                                            message: warning.clone(),
                                            technical: String::new(),
                                        });
                                    }
                                    self.library = library;
                                    self.loaded = true;
                                    self.indices.clear();
                                    self.change_filter();
                                    self.detail = None;
                                    self.detail_failed = None;
                                    self.artwork.reload(self.library.clone());
                                    let id = self.activity.queue(
                                        "Finding existing artwork",
                                        Route::Section(Section::Games),
                                        false,
                                    );
                                    self.activity.start(id);
                                    self.index_job = Some(id);
                                }
                                Payload::Filter {
                                    indices,
                                    generation,
                                } => {
                                    self.filter_inflight = false;
                                    if generation == self.filter_generation {
                                        self.indices = indices;
                                    }
                                }
                                Payload::Detail { detail, generation } => {
                                    self.detail_pending = None;
                                    if generation == self.detail_generation {
                                        self.detail = Some(detail);
                                    }
                                }
                                Payload::Verification(result) => {
                                    self.verification_job = None;
                                    self.verification = Some(result);
                                    self.router.current = Route::Section(Section::Check);
                                }
                                Payload::Duplicates(report) => {
                                    self.duplicate_job = None;
                                    self.duplicate_report = Some(report);
                                }
                                Payload::DuplicatePreview(preview) => {
                                    self.repair_preview = Some(*preview);
                                    self.repair_result = None;
                                }
                                Payload::DuplicateApplied(record) => {
                                    self.repair_job = None;
                                    self.repair_history.push(*record);
                                    self.repair_preview = None;
                                    self.repair_result = Some("The duplicate copy was moved to EmuWiz quarantine. Your original content was not deleted, and this change can be undone from History.".into());
                                }
                                Payload::DuplicateUndone(record) => {
                                    self.undo_job = None;
                                    let record = *record;
                                    if let Some(existing) =
                                        self.repair_history.iter_mut().find(|item| {
                                            item.transaction.transaction_id
                                                == record.transaction.transaction_id
                                        })
                                    {
                                        *existing = record;
                                    }
                                    self.repair_result =
                                        Some("The quarantined file was restored safely.".into());
                                }
                                Payload::RepairHistory {
                                    duplicates,
                                    playing_libraries,
                                } => {
                                    self.repair_history = duplicates;
                                    self.playing_library_history = playing_libraries;
                                }
                                Payload::Preferences(preferences) => {
                                    if !self.interacted {
                                        self.router.current = preferences.route;
                                        self.filter = preferences.filter;
                                        self.filter_dirty = Some(Instant::now());
                                    }
                                }
                                Payload::PlayingLibraryPreview { state, generation } => {
                                    self.finish_playing_library_job(
                                        PlayingLibraryJobKind::Preview,
                                        state,
                                        generation,
                                    );
                                }
                                Payload::PlayingLibraryApply { state, generation } => {
                                    self.finish_playing_library_job(
                                        PlayingLibraryJobKind::Apply,
                                        state,
                                        generation,
                                    );
                                }
                                Payload::Done => {}
                            }
                        }
                        Err(error) => {
                            if self.playing_library_job.as_ref().is_some_and(|job| job.id == id) {
                                self.playing_library_job = None;
                            }
                            if self.repair_job == Some(id) {
                                self.repair_job = None;
                            }
                            if self.undo_job == Some(id) {
                                self.undo_job = None;
                            }
                            self.detail_failed = self.detail_pending.take();
                            self.filter_inflight = false;
                            let title = self
                                .activity
                                .jobs
                                .get(&id)
                                .map(|job| job.title.as_str())
                                .unwrap_or("This action");
                            let message = format!(
                                "{title} could not finish. You can keep browsing the last loaded games. Retry the action, or open Legacy / Advanced interface to check setup."
                            );
                            self.activity
                                .finish(id, message.clone(), Some(error.clone()));
                            self.notice = Some(Notice {
                                message,
                                technical: error,
                            });
                        }
                    }
                }
            }
        }
        if !self.artwork.index_loading
            && let Some(id) = self.index_job.take()
        {
            self.activity.finish(
                id,
                "Artwork locations checked. Pictures load only when you browse them.".into(),
                None,
            );
        }
        if self
            .filter_dirty
            .is_some_and(|time| time.elapsed() >= Duration::from_millis(120))
            && !self.filter_inflight
            && self.loaded
        {
            self.filter_dirty = None;
            self.filter_inflight = true;
            self.send(
                0,
                Command::Filter {
                    library: self.library.clone(),
                    filter: self.filter.clone(),
                    generation: self.filter_generation,
                },
            );
        }
        if let Some(id) = self.router.current.game()
            && self.detail_pending.is_none()
            && self.detail_failed != Some(id)
            && self.detail.as_ref().is_none_or(|detail| detail.game != id)
            && let Some(game) = self.library.game(id).cloned()
        {
            let job = self.activity.queue(
                "Checking this game's saved information and installed emulators",
                Route::Game(id),
                false,
            );
            self.detail_pending = Some(id);
            self.send(
                job,
                Command::Detail {
                    game: Box::new(game),
                    generation: self.detail_generation,
                },
            );
        }
        if self
            .preferences_dirty
            .is_some_and(|time| time.elapsed() > Duration::from_millis(500))
        {
            self.preferences_dirty = None;
            self.send(
                0,
                Command::Save(Preferences {
                    route: self.router.current.clone(),
                    filter: self.filter.clone(),
                }),
            );
        }
        if self.activity.running() > 0
            || self.artwork.active() > 0
            || self.filter_dirty.is_some()
            || self.preferences_dirty.is_some()
        {
            context.request_repaint_after(Duration::from_millis(100));
        }
    }
    fn finish_frame(&mut self) {
        self.artwork.end_frame();
        if self.artwork.active() > 0 && self.artwork_job.is_none() {
            let id = self
                .activity
                .queue("Loading artwork", Route::Section(Section::Games), true);
            self.activity.start(id);
            self.artwork_job = Some(id);
        }
        if let Some(id) = self.artwork_job {
            if let Some(job) = self.activity.jobs.get_mut(&id) {
                job.progress = Some((self.artwork.completed, self.artwork.requested));
                job.summary = "Loading visible pictures. Off-screen requests are cancelled.".into();
                if job
                    .cancel
                    .as_ref()
                    .is_some_and(|cancel| cancel.load(std::sync::atomic::Ordering::Relaxed))
                {
                    self.artwork.cancel();
                }
            }
            if self.artwork.active() == 0 {
                let summary = format!(
                    "{} picture requests finished; {} cancelled off screen; {} pictures could not load. Games remain available. Open a game to retry an unavailable picture.",
                    self.artwork.completed, self.artwork.cancelled, self.artwork.failures
                );
                self.activity.finish(
                    id,
                    summary,
                    (self.artwork.failures > 0).then(|| {
                        "Per-picture errors and timings are in the game's Advanced details.".into()
                    }),
                );
                self.artwork_job = None;
                self.artwork.requested = 0;
                self.artwork.completed = 0;
                self.artwork.failures = 0;
                self.artwork.cancelled = 0;
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.mods.poll();
        if let Some(workflows) = &mut self.native_workflows {
            workflows.poll(ui.ctx(), &mut self.activity);
        }
        self.poll(ui.ctx());
        self.show(ui.ctx());
        self.finish_frame();
    }
}
