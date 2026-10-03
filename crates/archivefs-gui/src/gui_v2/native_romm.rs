//! Live, read-only RomM browsing from Sources & Providers.
//! The promoted core browser is the sole HTTP/identity implementation.
use super::super::{
    activity::Activity,
    backend::{Backend, Command, Event},
    routes::{Route, Section},
};
use archivefs_core::identity_source::romm::browser::{
    RommBrowseError, RommBrowseFilter, RommBrowsePage, RommGameDetail, RommGameSummary,
    RommPlatformSummary, RommServerInfo, RommServerStatus,
};
use archivefs_core::identity_source::romm::config::RommToken;
use archivefs_core::identity_source::settings::ProviderSettings;
use eframe::egui;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};

#[cfg(test)]
#[path = "native_romm/tests.rs"]
mod tests;
#[path = "native_romm/worker.rs"]
mod worker;

const PAGE_SIZE: u32 = 50;
const SEARCH_DELAY: Duration = Duration::from_millis(350);
const READ_ONLY: &str = "EmuWiz can browse this RomM library and show information that may help identify games. It will not upload, delete or change anything on your RomM server.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::gui_v2) enum Problem {
    NotConfigured,
    Disabled,
    Settings,
    Credentials,
    NameLookup,
    Backend(RommBrowseError),
    WorkerStopped,
}
impl Problem {
    fn explanation(&self) -> (&'static str, &'static str, &'static str) {
        use RommBrowseError as E;
        match self {
            Self::NotConfigured => (
                "RomM isn't connected yet.",
                "Add your RomM server address and credentials to browse its library from EmuWiz.",
                "Open RomM Settings to get started.",
            ),
            Self::Disabled => (
                "RomM is turned off in settings.",
                "Your saved RomM connection is disabled.",
                "Enable RomM in settings, then connect.",
            ),
            Self::Settings => (
                "RomM settings need attention.",
                "EmuWiz couldn't read a usable saved configuration.",
                "Review the RomM server address and settings.",
            ),
            Self::Credentials => (
                "RomM login details need attention.",
                "EmuWiz couldn't read the saved credentials.",
                "Review the login details in RomM Settings.",
            ),
            Self::NameLookup => (
                "EmuWiz couldn't find that RomM server.",
                "The address may be wrong, its hostname may not resolve from this machine, or the server may be offline.",
                "Check the server address in settings, or retry when the server is available.",
            ),
            Self::Backend(E::AuthRequired | E::AuthFailed) => (
                "RomM rejected the login details.",
                "The server requires a valid login with permission to browse games.",
                "Review your login details in RomM Settings, then retry.",
            ),
            Self::Backend(E::Tls) => (
                "EmuWiz couldn't verify RomM's security certificate.",
                "The secure connection couldn't be verified.",
                "Check the server certificate and this machine's clock, then retry.",
            ),
            Self::Backend(
                E::UnsupportedVersion
                | E::UnsupportedCapability
                | E::UnsupportedFilter
                | E::SchemaIncompatibility,
            ) => (
                "This RomM server can't provide this browsing feature yet.",
                "EmuWiz reached RomM, but the server doesn't provide the features this browser needs.",
                "Check the RomM version and server setup. Version information is available under Details.",
            ),
            Self::Backend(E::Timeout) => (
                "RomM took too long to reply.",
                "The connection or server may be busy or unavailable.",
                "Check that RomM is running, then retry.",
            ),
            Self::Backend(E::Unreachable) => (
                "Couldn't connect to RomM.",
                "The server couldn't be reached from this machine.",
                "Check the server address and network connection, then retry.",
            ),
            Self::Backend(E::EndpointRefused) => (
                "The RomM server address needs attention.",
                "The address doesn't meet EmuWiz's connection policy.",
                "Review the address in RomM Settings. Use your trusted local RomM server.",
            ),
            Self::Backend(E::InvalidRequest | E::LimitExceeded) => (
                "That search or request couldn't be used.",
                "It exceeds this browser's supported limits.",
                "Use a shorter search or clear the platform filter, then retry.",
            ),
            Self::Backend(E::RateLimited) => (
                "RomM needs a little time.",
                "The server asked EmuWiz to wait before sending more requests.",
                "Wait a moment, then retry.",
            ),
            Self::Backend(E::PaginationInconsistency) => (
                "RomM's game list couldn't be shown consistently.",
                "The returned page doesn't agree with the request. The library may have changed while you were browsing.",
                "Start at the first page, or retry when the library is ready.",
            ),
            Self::Backend(E::Cancelled) => (
                "RomM browsing was stopped.",
                "The request was cancelled.",
                "Retry when you're ready.",
            ),
            Self::Backend(_) => (
                "RomM couldn't complete this request.",
                "The server reply was unavailable, inconsistent or couldn't be read safely.",
                "Retry, or check the server and its version in RomM Settings.",
            ),
            Self::WorkerStopped => (
                "RomM browsing couldn't continue.",
                "The background operation stopped unexpectedly.",
                "Close and reopen the browser, then retry.",
            ),
        }
    }
}

#[derive(Clone)]
pub(in crate::gui_v2) enum Request {
    Settings,
    Connect,
    Games {
        offset: u32,
        filter: RommBrowseFilter,
    },
    Detail(u64),
    Cover(Box<RommGameSummary>),
}
impl Request {
    fn label(&self) -> &'static str {
        match self {
            Self::Settings => "Checking RomM settings",
            Self::Connect => "Connecting to RomM",
            Self::Games { .. } => "Loading RomM games",
            Self::Detail(_) => "Loading RomM game details",
            Self::Cover(_) => "Loading RomM artwork",
        }
    }
}
pub(in crate::gui_v2) enum Reply {
    Settings(Box<ProviderSettings>, Option<RommToken>),
    Connection {
        info: RommServerInfo,
        platforms: Vec<RommPlatformSummary>,
        page: Option<RommBrowsePage>,
        token: RommToken,
    },
    Page(RommBrowsePage, RommToken),
    Detail(RommGameDetail, RommToken),
    Cover {
        id: u64,
        image: Option<egui::ColorImage>,
    },
}
pub(in crate::gui_v2) struct Work {
    pub request: Request,
    pub roots: Vec<PathBuf>,
    pub cancel: Arc<AtomicBool>,
    pub reply: Sender<Result<Reply, Problem>>,
    #[cfg(test)]
    pub root: Option<PathBuf>,
}
pub(in crate::gui_v2) fn run(work: Work) {
    #[cfg(not(test))]
    let result = worker::run(work.request, &work.roots, &work.cancel);
    #[cfg(test)]
    let result = if let Some(root) = work.root {
        worker::run_with(
            &root,
            work.request,
            &work.roots,
            &archivefs_core::identity_source::net_policy::SystemResolver,
            &archivefs_core::identity_source::romm::client::UreqTransport::new(),
            &work.cancel,
        )
    } else {
        worker::run(work.request, &work.roots, &work.cancel)
    };
    let _ = work.reply.send(result);
}

struct Running {
    id: u64,
    generation: u64,
    cancel: Arc<AtomicBool>,
    reply: Receiver<Result<Reply, Problem>>,
}
#[derive(Default)]
struct Lane {
    backend: Option<Backend>,
    running: Option<Running>,
    pending: Option<Request>,
}
impl Drop for Lane {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl Lane {
    fn cancel(&mut self) {
        self.pending = None;
        if let Some(running) = &self.running {
            running.cancel.store(true, Ordering::Release);
        }
    }
    fn active(&self) -> bool {
        self.running.is_some() || self.pending.is_some()
    }
    fn poll(
        &mut self,
        ctx: &egui::Context,
        activity: &mut Activity,
        generation: u64,
        roots: &[PathBuf],
        #[cfg(test)] root: Option<&std::path::Path>,
    ) -> Option<(u64, Result<Reply, Problem>)> {
        let mut completed = None;
        if let Some(backend) = &self.backend {
            for event in backend.rx.try_iter() {
                if let Event::Finished { id, outcome } = event
                    && self.running.as_ref().is_some_and(|r| r.id == id)
                {
                    let running = self.running.take().unwrap();
                    let result = if outcome.is_ok() {
                        running
                            .reply
                            .try_recv()
                            .unwrap_or(Err(Problem::WorkerStopped))
                    } else {
                        Err(Problem::WorkerStopped)
                    };
                    if running.generation != generation || running.cancel.load(Ordering::Acquire) {
                        activity.supersede(id, "A newer browser request replaced this one.".into());
                    } else {
                        let error = result.as_ref().err().map(|e| e.explanation().0.to_owned());
                        activity.finish(
                            id,
                            error
                                .clone()
                                .unwrap_or_else(|| "RomM information is ready.".into()),
                            error,
                        );
                        completed = Some((running.generation, result));
                    }
                }
            }
        }
        if self.running.is_none()
            && let Some(request) = self.pending.take()
        {
            let id = activity.queue(
                request.label(),
                Route::Section(Section::SourcesProviders),
                true,
            );
            let cancel = activity.jobs[&id].cancel.clone().unwrap();
            let (reply, receiver) = mpsc::channel();
            let backend = self
                .backend
                .get_or_insert_with(|| Backend::start(ctx.clone()));
            if backend
                .send(
                    id,
                    Command::NativeRomm(Box::new(Work {
                        request,
                        roots: roots.to_vec(),
                        cancel: cancel.clone(),
                        reply,
                        #[cfg(test)]
                        root: root.map(std::path::Path::to_path_buf),
                    })),
                )
                .is_err()
            {
                activity.finish(
                    id,
                    "The browser worker is unavailable.".into(),
                    Some("Background worker unavailable.".into()),
                );
                completed = Some((generation, Err(Problem::WorkerStopped)));
            } else {
                activity.start(id);
                self.running = Some(Running {
                    id,
                    generation,
                    cancel,
                    reply: receiver,
                });
            }
        }
        completed
    }
}

pub(super) struct State {
    open: bool,
    generation: u64,
    roots: Vec<PathBuf>,
    browse: Lane,
    art: Lane,
    configured: bool,
    settings_loaded: bool,
    saved_settings: Option<Box<ProviderSettings>>,
    info: Option<RommServerInfo>,
    token: Option<RommToken>,
    platforms: Vec<RommPlatformSummary>,
    page: Option<Arc<RommBrowsePage>>,
    search: String,
    platform: Option<u64>,
    offset: u32,
    edited: Option<Instant>,
    selected: Option<u64>,
    last_selected: Option<u64>,
    detail: Option<Arc<RommGameDetail>>,
    problem: Option<Problem>,
    retry: Request,
    covers: HashMap<u64, Option<egui::TextureHandle>>,
    text_focused: bool,
    visible_last_frame: bool,
    #[cfg(test)]
    test_root: Option<PathBuf>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            generation: 0,
            roots: vec![],
            browse: Lane::default(),
            art: Lane::default(),
            configured: false,
            settings_loaded: false,
            saved_settings: None,
            info: None,
            token: None,
            platforms: vec![],
            page: None,
            search: String::new(),
            platform: None,
            offset: 0,
            edited: None,
            selected: None,
            last_selected: None,
            detail: None,
            problem: None,
            retry: Request::Connect,
            covers: HashMap::new(),
            text_focused: false,
            visible_last_frame: false,
            #[cfg(test)]
            test_root: None,
        }
    }
}
impl State {
    pub(super) fn settings_draft(&self) -> crate::romm_config::RommConfigDraft {
        let mut settings = self.saved_settings.as_deref().cloned().unwrap_or_default();
        // Unsafe credentials/query data must not become text in the existing editor.
        if settings.source.url.contains(['@', '?', '#']) {
            settings.source.url.clear();
        } else {
            settings.source.url = self.display(&settings.source.url);
        }
        crate::romm_config::RommConfigDraft::from_settings(&settings)
    }
    pub(super) fn is_open(&self) -> bool {
        self.open
    }
    pub(super) fn open(&mut self, roots: Vec<PathBuf>) {
        self.open = true;
        self.roots = roots;
        if !self.settings_loaded {
            self.queue(Request::Settings);
        }
    }
    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.edited = None;
        self.browse.cancel();
        self.art.cancel();
        self.covers.retain(|_, image| image.is_some());
    }
    fn queue(&mut self, request: Request) {
        self.invalidate();
        if matches!(request, Request::Connect) {
            self.info = None;
            self.page = None;
            self.selected = None;
            self.detail = None;
            self.covers.clear();
        }
        self.problem = None;
        self.retry = request.clone();
        self.browse.pending = Some(request);
    }
    fn filter(&self) -> RommBrowseFilter {
        RommBrowseFilter {
            text: (!self.search.trim().is_empty()).then(|| self.search.trim().to_owned()),
            platform_id: self.platform,
        }
    }
    fn games(&mut self, offset: u32) {
        self.offset = offset;
        self.selected = None;
        self.detail = None;
        self.page = None;
        self.covers.clear();
        self.queue(Request::Games {
            offset,
            filter: self.filter(),
        });
    }
    fn edit_search(&mut self, now: Instant) {
        self.invalidate();
        self.offset = 0;
        self.page = None;
        self.detail = None;
        self.selected = None;
        self.covers.clear();
        self.problem = None;
        self.edited = Some(now);
    }
    fn back(&mut self) {
        if self.selected.take().is_some() {
            self.invalidate();
            self.detail = None;
        } else {
            self.open = false;
            self.invalidate();
        }
    }
    pub(super) fn settings_opened(&mut self) {
        self.invalidate();
        self.settings_loaded = false;
        self.info = None;
        self.token = None;
        self.page = None;
        self.problem = None;
        self.visible_last_frame = false;
    }
    pub(super) fn poll(&mut self, ctx: &egui::Context, activity: &mut Activity) {
        // Consume Back before the shared page navigation sees it beneath this window.
        if self.open
            && self.visible_last_frame
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::ALT, egui::Key::ArrowLeft))
        {
            self.back();
        }
        self.visible_last_frame = false;
        if self.open
            && self
                .edited
                .is_some_and(|time| time.elapsed() >= SEARCH_DELAY)
        {
            self.games(0);
        } else if self.edited.is_some() {
            ctx.request_repaint_after(SEARCH_DELAY);
        }
        if let Some((generation, result)) = self.browse.poll(
            ctx,
            activity,
            self.generation,
            &self.roots,
            #[cfg(test)]
            self.test_root.as_deref(),
        ) {
            self.absorb(ctx, generation, result);
        }
        if let Some((generation, Ok(reply))) = self.art.poll(
            ctx,
            activity,
            self.generation,
            &self.roots,
            #[cfg(test)]
            self.test_root.as_deref(),
        ) {
            self.absorb(ctx, generation, Ok(reply));
        }
    }
    fn absorb(
        &mut self,
        ctx: &egui::Context,
        generation: u64,
        result: Result<Reply, Problem>,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        match result {
            Err(error) => {
                if matches!(self.retry, Request::Settings) {
                    self.settings_loaded = true;
                }
                self.problem = Some(error);
            }
            Ok(Reply::Settings(settings, token)) => {
                self.settings_loaded = true;
                self.configured = !settings.source.url.trim().is_empty();
                self.problem = if !self.configured {
                    Some(Problem::NotConfigured)
                } else if !settings.source.enabled {
                    Some(Problem::Disabled)
                } else {
                    None
                };
                self.token = token;
                self.saved_settings = Some(settings);
            }
            Ok(Reply::Connection {
                info,
                platforms,
                page,
                token,
            }) => {
                self.problem = info.error.clone().map(Problem::Backend);
                if self.problem.is_none() && !info.capabilities.games {
                    self.problem = Some(Problem::Backend(RommBrowseError::UnsupportedCapability));
                }
                self.info = Some(info);
                self.token = Some(token);
                self.platforms = platforms;
                self.page = page.map(Arc::new);
                self.search.clear();
                self.platform = None;
                self.offset = 0;
                self.selected = None;
                self.last_selected = None;
                self.detail = None;
                self.covers.clear();
            }
            Ok(Reply::Page(page, token)) => {
                self.page = Some(Arc::new(page));
                self.token = Some(token);
            }
            Ok(Reply::Detail(detail, token)) => {
                if self.selected == Some(detail.game.id) {
                    self.detail = Some(Arc::new(detail));
                    self.token = Some(token);
                } else {
                    return false;
                }
            }
            Ok(Reply::Cover { id, image }) => {
                self.covers.insert(
                    id,
                    image.map(|image| {
                        ctx.load_texture(
                            format!("native-romm-{}-{id}", self.generation),
                            image,
                            egui::TextureOptions::LINEAR,
                        )
                    }),
                );
            }
        }
        true
    }
    fn connected(&self) -> bool {
        self.info.as_ref().is_some_and(|s| {
            matches!(
                s.status,
                RommServerStatus::Supported | RommServerStatus::PartiallySupported
            )
        })
    }
    fn empty_message(&self) -> String {
        if !self.search.trim().is_empty() {
            format!(
                "No games matched '{}'. Try another search or clear the platform filter.",
                self.display(self.search.trim())
            )
        } else if self.platform.is_some() {
            "No games are visible for this platform. Try All platforms or check this account's access in RomM.".into()
        } else {
            "RomM has no games to show for this connection. If you expected games, check the library and this account's access in RomM, then refresh.".into()
        }
    }
    /// Returns true only for the existing settings action; no write/download action exists.
    pub(super) fn show(&mut self, ctx: &egui::Context) -> bool {
        if !self.open {
            return false;
        }
        self.visible_last_frame = true;
        if !self.settings_loaded && !self.browse.active() {
            self.queue(Request::Settings);
        }
        let escape = !self.text_focused
            && !egui::Popup::is_any_open(ctx)
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let back =
            ctx.input_mut(|input| input.consume_key(egui::Modifiers::ALT, egui::Key::ArrowLeft));
        if back || (escape && !self.text_focused && !egui::Popup::is_any_open(ctx)) {
            self.back();
        }
        if !self.open {
            return false;
        }
        let mut open = self.open;
        let mut settings = false;
        let mut go_back = false;
        let screen = ctx.content_rect().size();
        egui::Window::new("RomM — Your library")
            .id(egui::Id::new("native-romm-browser"))
            .open(&mut open)
            .collapsible(false)
            .default_size(egui::vec2(850.0, 620.0))
            .min_size(egui::vec2(360.0, 280.0))
            .max_size(screen - egui::vec2(24.0, 50.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("native-romm-body")
                    .max_height((screen.y - 150.0).max(170.0))
                    .show(ui, |ui| {
                        settings = self.body(ui);
                    });
                ui.separator();
                if ui
                    .button(if self.selected.is_some() {
                        "Back to games"
                    } else {
                        "Back to Sources & Providers"
                    })
                    .clicked()
                {
                    go_back = true;
                }
            });
        if !open {
            self.open = false;
            self.invalidate();
        } else if go_back {
            self.back();
        }
        settings
    }
    fn body(&mut self, ui: &mut egui::Ui) -> bool {
        let mut settings = false;
        crate::ui::components::card(ui, |ui| {
            if self.browse.active() && self.info.is_none() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.strong(if matches!(self.retry, Request::Settings) {
                        "Checking RomM settings…"
                    } else {
                        "Connecting to RomM…"
                    });
                });
            } else if self.connected() && self.problem.is_none() {
                ui.strong("Connected to your RomM server.");
            } else if self.problem.is_none() {
                ui.strong("Your RomM settings are ready.");
                ui.label("Connect to browse your library.");
            }
            if let Some(problem) = &self.problem {
                let (heading, message, next) = problem.explanation();
                ui.strong(heading);
                ui.label(message);
                if !matches!(problem, Problem::NotConfigured) {
                    ui.label(
                        "Nothing on your RomM server or in your EmuWiz library has been changed.",
                    );
                }
                ui.label(next);
            }
            ui.horizontal_wrapped(|ui| {
                if !self.browse.active()
                    && (self.configured || matches!(self.problem, Some(Problem::Settings)))
                    && ui
                        .button(if self.problem.is_some() {
                            "Retry"
                        } else if self.connected() {
                            "Reconnect"
                        } else {
                            "Connect"
                        })
                        .clicked()
                {
                    self.queue(if self.problem.is_some() {
                        self.retry.clone()
                    } else {
                        Request::Connect
                    });
                }
                if matches!(
                    self.problem,
                    Some(Problem::Backend(RommBrowseError::PaginationInconsistency))
                ) && !self.browse.active()
                    && ui.button("Start at first page").clicked()
                {
                    self.games(0);
                }
                if ui.button("Open RomM Settings").clicked() {
                    settings = true;
                }
            });
            ui.collapsing("Details", |ui| {
                if let Some(info) = &self.info {
                    ui.label(format!("Server: {}", self.display(&info.server_id)));
                    if let Some(version) = &info.version { ui.label(format!("RomM version: {}", self.display(version))); }
                    ui.label(format!("Available features: platforms {}, browsing {}, search {}, platform filter {}, details {}", info.capabilities.platforms, info.capabilities.games, info.capabilities.text_search, info.capabilities.platform_filter, info.capabilities.game_detail));
                }
                if let Some(error) = &self.problem { ui.label(format!("Browser result: {error:?}")); }
            });
        });
        ui.add_space(6.0);
        ui.strong("READ ONLY");
        ui.label(READ_ONLY);
        if !self.connected() || !self.info.as_ref().is_some_and(|i| i.capabilities.games) {
            return settings;
        }
        ui.add_space(8.0);
        if self.selected.is_some() {
            self.detail_ui(ui);
            return settings;
        }
        ui.heading("Your RomM library");
        let capabilities = self.info.as_ref().unwrap().capabilities.clone();
        let mut changed_platform = false;
        let mut search_changed = false;
        ui.horizontal_wrapped(|ui| {
            ui.label("Search");
            let response = ui.add_enabled(
                capabilities.text_search,
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search your RomM games…")
                    .char_limit(128)
                    .desired_width(250.0),
            );
            self.text_focused = response.has_focus();
            search_changed = response.changed();
            if !self.search.is_empty() && ui.button("Clear search").clicked() {
                self.search.clear();
                search_changed = true;
            }
        });
        if !capabilities.text_search {
            ui.weak("Search is unavailable on this RomM server. You can still browse its pages.");
        }
        ui.horizontal_wrapped(|ui| {
            ui.label("Platform");
            ui.add_enabled_ui(
                capabilities.platform_filter && !self.platforms.is_empty(),
                |ui| {
                    egui::ComboBox::from_id_salt("native-romm-platform")
                        .selected_text(
                            self.platforms
                                .iter()
                                .find(|p| Some(p.id) == self.platform)
                                .map_or_else(
                                    || "All platforms".into(),
                                    |p| self.display(platform_label(p)),
                                ),
                        )
                        .show_ui(ui, |ui| {
                            changed_platform |= ui
                                .selectable_value(&mut self.platform, None, "All platforms")
                                .changed();
                            for platform in &self.platforms {
                                let name = self.display(platform_label(platform));
                                changed_platform |= ui
                                    .selectable_value(&mut self.platform, Some(platform.id), name)
                                    .changed();
                            }
                        });
                },
            );
            if ui
                .add_enabled(!self.browse.active(), egui::Button::new("Refresh games"))
                .clicked()
            {
                self.games(self.offset);
            }
        });
        if capabilities.platforms && self.platforms.is_empty() {
            ui.weak("No platforms are visible to this connection. The server may be empty or this login may have limited access.");
        }
        if !capabilities.platform_filter {
            ui.weak("Platform filtering is unavailable on this server.");
        }
        if !capabilities.game_detail {
            ui.weak("This server can list games, but doesn't provide game details to EmuWiz.");
        }
        if changed_platform {
            self.games(0);
        } else if search_changed {
            self.edit_search(Instant::now());
        }
        if self.browse.active() || self.edited.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Loading games… You can keep typing or change the platform.");
            });
        }
        let Some(page) = self.page.clone() else {
            return settings;
        };
        ui.horizontal_wrapped(|ui| {
            if !page.games.is_empty() {
                let start = u64::from(page.offset) + 1;
                let end = u64::from(page.offset) + page.games.len() as u64;
                ui.label(match page.total {
                    Some(total) => format!("Showing {start}–{end} of {total}"),
                    None => format!("Showing {start}–{end}"),
                });
            }
            if ui
                .add_enabled(
                    page.previous_offset.is_some() && !self.browse.active(),
                    egui::Button::new("Previous"),
                )
                .clicked()
            {
                self.games(page.previous_offset.unwrap());
            }
            if ui
                .add_enabled(
                    page.next_offset.is_some() && !self.browse.active(),
                    egui::Button::new("Next"),
                )
                .clicked()
            {
                self.games(page.next_offset.unwrap());
            }
        });
        if page.games.is_empty() {
            if page.previous_offset.is_some() {
                ui.label("There are no games on this page. Use Previous to return to your results, or reconnect to refresh the library.");
            } else {
                ui.label(self.empty_message());
            }
            return settings;
        }
        for game in &page.games {
            ui.push_id(game.id, |ui| {
                crate::ui::components::card(ui, |ui| {
                    ui.horizontal(|ui| {
                        self.cover_ui(ui, game, egui::vec2(44.0, 62.0));
                        ui.vertical(|ui| {
                            let title = self
                                .display(game.identity.title.as_deref().unwrap_or("Untitled game"));
                            let button = egui::Button::new(title)
                                .selected(self.last_selected == Some(game.id))
                                .wrap();
                            if ui.add_enabled(capabilities.game_detail, button).clicked() {
                                self.selected = Some(game.id);
                                self.last_selected = Some(game.id);
                                self.detail = None;
                                self.text_focused = false;
                                self.queue(Request::Detail(game.id));
                            }
                            ui.weak(self.display(&self.game_platform(game)));
                            if !game.identity.regions.is_empty() {
                                ui.weak(self.display(&game.identity.regions.join(", ")));
                            }
                        });
                    });
                });
            });
        }
        settings
    }
    fn cover_ui(&mut self, ui: &mut egui::Ui, game: &RommGameSummary, box_size: egui::Vec2) {
        let (rect, _) = ui.allocate_exact_size(box_size, egui::Sense::hover());
        if let Some(Some(texture)) = self.covers.get(&game.id) {
            let size = crate::gamer_artwork::fit_within(box_size, texture.size_vec2());
            ui.painter().image(
                texture.id(),
                egui::Rect::from_center_size(rect.center(), size),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            crate::ui::platform_artwork::paint_platform_glyph_at(
                ui.painter(),
                rect.center(),
                rect.width(),
                ui.visuals().weak_text_color(),
                &crate::ui::platform_artwork::platform_asset_id(&platform_name(game), false),
            );
            if game.artwork.cover.is_some()
                && !self.covers.contains_key(&game.id)
                && !self.art.active()
                && ui.is_rect_visible(rect)
            {
                self.covers.insert(game.id, None);
                self.art.pending = Some(Request::Cover(Box::new(game.clone())));
            }
        }
    }
    fn detail_ui(&mut self, ui: &mut egui::Ui) {
        if self.browse.active() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Loading game details…");
            });
        }
        let Some(detail) = self.detail.clone() else {
            return;
        };
        let game = &detail.game;
        ui.heading(self.display(game.identity.title.as_deref().unwrap_or("Untitled game")));
        if let Some(description) = &game.identity.synopsis {
            ui.label(self.display(description));
        }
        if !game.identity.genres.is_empty() {
            ui.label(self.display(&game.identity.genres.join(" · ")));
        }
        if let Some(year) = game.identity.release_year {
            ui.label(format!("Released: {year}"));
        }
        ui.horizontal_wrapped(|ui| {
            self.cover_ui(ui, game, egui::vec2(90.0, 126.0));
            ui.vertical(|ui| {
                ui.label(self.display(&self.game_platform(game)));
                if !game.identity.regions.is_empty() {
                    ui.label(format!(
                        "Region: {}",
                        self.display(&game.identity.regions.join(", "))
                    ));
                }
                if let Some(revision) = &game.identity.revision {
                    ui.label(format!("Revision: {}", self.display(revision)));
                }
                if let Some(filename) = &game.filename {
                    ui.label(format!("File: {}", self.display(filename)));
                }
                if game.artwork.cover.is_none() {
                    ui.weak("No cover artwork is available. Browsing still works.");
                }
            });
        });
        ui.add_space(8.0);
        ui.strong("Identity clues from RomM");
        ui.label(format!(
            "RomM suggests this may be {} for {}.",
            self.display(game.identity.title.as_deref().unwrap_or("this game")),
            self.display(&self.game_platform(game))
        ));
        ui.label("This is external evidence from RomM. EmuWiz's verified identity and your chosen metadata remain unchanged.");
        for hash in &game.identity.hashes {
            ui.label(format!(
                "{:?}: {}",
                hash.algorithm,
                self.display(&hash.value)
            ));
        }
        for id in &game.identity.metadata_provider_ids {
            ui.label(format!(
                "{} ID: {}",
                self.display(&id.provider),
                self.display(&id.id)
            ));
        }
        if !detail.files.is_empty() {
            ui.strong("Files listed by RomM");
            for file in &detail.files {
                ui.label(self.display(file.filename.as_deref().unwrap_or("Unnamed file")));
            }
        }
        ui.collapsing("Details", |ui| {
            ui.label(format!("RomM item: {}", game.id));
            ui.label(format!(
                "Observed from: {}",
                self.display(&game.provenance.server_id)
            ));
            ui.label("Related files are provider relationships; no disc order is inferred.");
        });
    }
    fn game_platform(&self, game: &RommGameSummary) -> String {
        let id = game
            .identity
            .provider_platform_id
            .as_deref()
            .and_then(|v| v.parse::<u64>().ok());
        self.platforms
            .iter()
            .find(|p| Some(p.id) == id)
            .and_then(|p| p.name.clone())
            .or_else(|| game.identity.platform_candidate.clone())
            .unwrap_or_else(|| "Unknown platform".into())
    }
    /// Provider strings are untrusted display text. Keep the original typed evidence intact.
    fn display(&self, value: &str) -> String {
        let mut text = self.token.as_ref().map_or_else(
            || value.to_owned(),
            |token| {
                token.with_header_value(|header| {
                    value
                        .replace(header, "[hidden]")
                        .replace(header.strip_prefix("Bearer ").unwrap(), "[hidden]")
                })
            },
        );
        let mut start = 0;
        while let Some(scheme) = text[start..].find("://") {
            let authority = start + scheme + 3;
            let end = text[authority..]
                .find(['/', '?', '#', ' ', '\n'])
                .map_or(text.len(), |i| authority + i);
            if let Some(at) = text[authority..end].rfind('@') {
                text.replace_range(authority..authority + at, "[hidden]");
                start = authority + "[hidden]@".len();
            } else {
                start = end;
            }
        }
        text
    }
}
fn platform_label(platform: &RommPlatformSummary) -> &str {
    platform.name.as_deref().unwrap_or("Unnamed platform")
}

fn platform_name(game: &RommGameSummary) -> String {
    game.identity
        .platform_candidate
        .clone()
        .or_else(|| game.identity.provider_platform_name.clone())
        .unwrap_or_else(|| "Unknown platform".into())
}
