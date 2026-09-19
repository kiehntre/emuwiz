//! GUI v2 is a parallel presentation layer, not a mode of the legacy app.
mod activity;
mod artwork;
mod backend;
mod legacy;
mod library;
mod media_sources;
mod pages;
mod routes;
#[cfg(test)]
mod tests;
mod thumbnail;

use activity::Activity;
use artwork::Artwork;
use backend::{Backend, Command, Event, Payload, Preferences};
use eframe::egui;
use library::{Detail, Filter, Library, SharedLibrary};
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
        };
        app.send(0, Command::Restore);
        app.load(false);
        app
    }
    fn send(&mut self, id: u64, command: Command) {
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
    fn poll(&mut self, context: &egui::Context) {
        self.artwork.begin_frame(context);
        for _ in 0..32 {
            let Ok(event) = self.backend.rx.try_recv() else {
                break;
            };
            match event {
                Event::Started(id) => self.activity.start(id),
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
                                Payload::Preferences(preferences) => {
                                    if !self.interacted {
                                        self.router.current = preferences.route;
                                        self.filter = preferences.filter;
                                        self.filter_dirty = Some(Instant::now());
                                    }
                                }
                                Payload::Done => {}
                            }
                        }
                        Err(error) => {
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
        self.poll(ui.ctx());
        self.show(ui.ctx());
        self.finish_frame();
    }
}
