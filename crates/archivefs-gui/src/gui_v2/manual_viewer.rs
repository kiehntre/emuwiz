//! Native presentation of the canonical document reader. No archive or image parsing here.
use super::documents::{
    DocumentFitMode, DocumentOpenCapability, DocumentReadingState, GameDocument,
    GameDocumentAssociation,
};
use archivefs_core::manual_document::{
    ManualDocument, ManualDocumentId, ManualDocumentKind, ManualLimits, ManualPageImage,
    ManualReadiness, ManualViewerAction as Action, ManualViewerState, ManualZoom, pdf_render,
};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
};

pub(super) fn capability_text(readiness: Option<ManualReadiness>) -> &'static str {
    match readiness {
        Some(ManualReadiness::Viewable) => "CBZ · Read pages inside EmuWiz.",
        Some(ManualReadiness::InspectOnly { .. }) => {
            if pdf_render::available() {
                "PDF · Read pages inside EmuWiz. Open externally if a page cannot be rendered."
            } else {
                "PDF · Internal rendering is unavailable on this platform. Open externally."
            }
        }
        Some(ManualReadiness::Unsupported { .. }) => {
            "CBR · Recognised; internal reading needs a RAR reader and is not supported yet."
        }
        None => "Internal reading is unavailable: the document could not be safely inspected.",
    }
}

fn internal_supported(readiness: Option<ManualReadiness>) -> bool {
    readiness.is_some_and(|readiness| {
        readiness.can_view()
            || matches!(readiness, ManualReadiness::InspectOnly { .. }) && pdf_render::available()
    })
}

fn association_allows_open(document: &GameDocument) -> bool {
    !matches!(
        document.association,
        GameDocumentAssociation::WeakFilename | GameDocumentAssociation::Unmatched
    )
}

fn open_buttons(ui: &mut egui::Ui, document: &GameDocument) -> (egui::Response, egui::Response) {
    (
        ui.add_enabled(
            association_allows_open(document)
                && document.document_id.is_some()
                && internal_supported(document.readiness),
            egui::Button::new("Open internally"),
        ),
        ui.add_enabled(
            association_allows_open(document)
                && document.document_id.is_some()
                && document.viewer == DocumentOpenCapability::Supported,
            egui::Button::new("Open externally"),
        ),
    )
}

// A single worker survives close/switch. Both slots are bounded to one item;
// a newer request replaces pending work, never the in-flight decode.
#[derive(Default)]
struct Mailbox {
    generation: u64,
    request: Option<Work>,
    result: Option<(u64, Reply)>,
    shutdown: bool,
}

enum Work {
    Open(PathBuf, ManualLimits, Option<ManualDocumentId>),
    Page(Arc<ManualDocument>, usize, u32),
}

enum Reply {
    Open(Result<Arc<ManualDocument>, String>),
    Page(ManualDocumentId, usize, Result<ManualPageImage, String>),
}

fn open_for_viewer(
    path: &std::path::Path,
    limits: &ManualLimits,
    expected: Option<&ManualDocumentId>,
) -> Result<Arc<ManualDocument>, String> {
    let document =
        ManualDocument::open(path, limits).map_err(|error| error.user_message().to_owned())?;
    if expected.is_some_and(|id| id != document.id()) {
        return Err("The document changed after it was inspected. Close and reopen it.".into());
    }
    if document.inspection().kind == ManualDocumentKind::Pdf {
        if document.id().len > pdf_render::MAX_PDF_BYTES {
            return Err(
                "This PDF exceeds the internal renderer's 64 MiB limit. Open externally.".into(),
            );
        }
        document
            .pdf_page_index()
            .map_err(|error| error.user_message().to_owned())?;
    }
    Ok(Arc::new(document))
}

struct Worker(Arc<(Mutex<Mailbox>, Condvar)>);

impl Worker {
    fn start(context: egui::Context) -> Result<Self, String> {
        let shared = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("manual-page-reader".into())
            .spawn(move || {
                loop {
                    let (lock, wake) = &*worker;
                    let mut mailbox = lock.lock().unwrap_or_else(|error| error.into_inner());
                    while mailbox.request.is_none() && !mailbox.shutdown {
                        mailbox = wake
                            .wait(mailbox)
                            .unwrap_or_else(|error| error.into_inner());
                    }
                    if mailbox.shutdown {
                        break;
                    }
                    let generation = mailbox.generation;
                    let Some(work) = mailbox.request.take() else {
                        continue;
                    };
                    drop(mailbox);
                    let result = match work {
                        Work::Open(path, limits, expected) => {
                            Reply::Open(open_for_viewer(&path, &limits, expected.as_ref()))
                        }
                        Work::Page(document, page, dimension) => Reply::Page(
                            document.id().clone(),
                            page,
                            if document.inspection().kind == ManualDocumentKind::Pdf {
                                std::env::current_exe()
                                    .map_err(|error| error.to_string())
                                    .and_then(|executable| {
                                        pdf_render::render_page(
                                            &document,
                                            page,
                                            &executable,
                                            dimension.min(pdf_render::MAX_RENDER_DIMENSION),
                                        )
                                    })
                            } else {
                                document
                                    .decode_page(page)
                                    .map_err(|error| error.user_message().to_owned())
                            },
                        ),
                    };
                    let mut mailbox = lock.lock().unwrap_or_else(|error| error.into_inner());
                    if !mailbox.shutdown && mailbox.generation == generation {
                        mailbox.result = Some((generation, result));
                        drop(mailbox);
                        context.request_repaint();
                    }
                }
            })
            .map_err(|error| format!("Could not start the manual reader: {error}"))?;
        Ok(Self(shared))
    }

    fn request(&self, work: Option<Work>) -> u64 {
        let (lock, wake) = &*self.0;
        let mut mailbox = lock.lock().unwrap_or_else(|error| error.into_inner());
        mailbox.generation = mailbox.generation.wrapping_add(1);
        mailbox.request = work;
        mailbox.result = None;
        wake.notify_one();
        mailbox.generation
    }

    fn take(&self) -> Option<(u64, Reply)> {
        let mut mailbox = self.0.0.lock().unwrap_or_else(|error| error.into_inner());
        mailbox.result.take()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let (lock, wake) = &*self.0;
        let mut mailbox = lock.lock().unwrap_or_else(|error| error.into_inner());
        mailbox.shutdown = true;
        mailbox.request = None;
        mailbox.result = None;
        wake.notify_one();
        // The bounded in-flight read may finish; never block the UI joining it.
    }
}

struct Session {
    path: PathBuf,
    title: String,
    resume: Option<DocumentReadingState>,
    document: Option<Arc<ManualDocument>>,
    state: ManualViewerState,
    generation: u64,
    texture: Option<egui::TextureHandle>,
    reset_scroll: bool,
    max_dimension: u32,
    external: DocumentOpenCapability,
    expected: Option<ManualDocumentId>,
    error: Option<String>,
}

#[derive(Default)]
pub(super) struct ManualViewer {
    worker: Option<Worker>,
    session: Option<Session>,
}

impl ManualViewer {
    fn open_associated(
        &mut self,
        context: &egui::Context,
        document: &GameDocument,
        resume: Option<DocumentReadingState>,
    ) {
        if !association_allows_open(document)
            || !internal_supported(document.readiness)
            || document.document_id.is_none()
        {
            return;
        }
        self.open_checked(
            context,
            document.path.clone(),
            document.title.clone(),
            resume,
            document.document_id.clone(),
        );
        if let Some(session) = &mut self.session {
            session.external = document.viewer;
        }
    }

    fn open_checked(
        &mut self,
        context: &egui::Context,
        path: PathBuf,
        title: String,
        resume: Option<DocumentReadingState>,
        expected: Option<ManualDocumentId>,
    ) {
        let mut session = Session {
            path: path.clone(),
            title,
            resume,
            document: None,
            state: ManualViewerState::closed(),
            generation: 0,
            texture: None,
            reset_scroll: true,
            max_dimension: context
                .input(|input| input.max_texture_side)
                .min(2048)
                .max(1) as u32,
            external: DocumentOpenCapability::NoHandler,
            expected: expected.clone(),
            error: None,
        };
        if self.worker.is_none() {
            match Worker::start(context.clone()) {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => session.error = Some(error),
            }
        }
        if let Some(worker) = &self.worker {
            // Tighten the canonical limit to the renderer's actual upload ceiling.
            let limits = ManualLimits {
                max_image_dimension: context
                    .input(|input| input.max_texture_side)
                    .min(ManualLimits::default().max_image_dimension as usize)
                    as u32,
                ..ManualLimits::default()
            };
            session.generation = worker.request(Some(Work::Open(path, limits, expected)));
        }
        self.session = Some(session);
        context.request_repaint();
    }

    #[cfg(test)]
    pub(super) fn open(
        &mut self,
        context: &egui::Context,
        path: PathBuf,
        title: String,
        resume: Option<DocumentReadingState>,
    ) {
        self.open_checked(context, path, title, resume, None);
    }

    fn poll(&mut self, context: &egui::Context) {
        if let Some((generation, reply)) = self.worker.as_ref().and_then(Worker::take) {
            self.accept(context, generation, reply);
        }
    }

    fn accept(&mut self, context: &egui::Context, generation: u64, reply: Reply) {
        let Some(session) = &mut self.session else {
            return;
        };
        if generation != session.generation {
            return;
        }
        match reply {
            Reply::Open(Ok(document)) => {
                session.expected = Some(document.id().clone());
                let inspection = document.inspection();
                session.external = super::documents::external_capability(
                    &session.path,
                    match inspection.kind {
                        ManualDocumentKind::Pdf => super::documents::GameDocumentFormat::Pdf,
                        ManualDocumentKind::Cbz => super::documents::GameDocumentFormat::Cbz,
                        ManualDocumentKind::Cbr => super::documents::GameDocumentFormat::Cbr,
                    },
                );
                if !internal_supported(Some(inspection.readiness)) {
                    session.error = Some(capability_text(Some(inspection.readiness)).into());
                    return;
                }
                session
                    .state
                    .open(document.id().clone(), inspection.page_count.unwrap_or(0));
                if let Some(saved) = session
                    .resume
                    .as_ref()
                    .filter(|saved| saved.document_id.as_ref() == Some(document.id()))
                {
                    if let Some(page) = saved.last_page.filter(|page| *page > 0) {
                        session
                            .state
                            .go_to_page(page.min(session.state.page_count()).saturating_sub(1));
                    }
                    match saved.fit_mode {
                        Some(DocumentFitMode::Width) => {
                            session.state.apply(Action::FitWidth);
                        }
                        Some(DocumentFitMode::Page) => {
                            session.state.apply(Action::FitPage);
                        }
                        None => restore_zoom(&mut session.state, saved.zoom_percent),
                    }
                }
                session.document = Some(document);
                self.request_page();
            }
            Reply::Open(Err(error)) => session.error = Some(error),
            Reply::Page(id, page, result) => {
                if session.state.document() != Some(&id) || session.state.current_page() != page {
                    return;
                }
                match result {
                    Ok(image) => {
                        let name = format!("manual:{id:?}:{page}:{generation}");
                        session.texture = Some(context.load_texture(
                            name,
                            egui::ColorImage::from_rgba_unmultiplied(
                                [image.width as usize, image.height as usize],
                                &image.rgba,
                            ),
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                    Err(error) => session.error = Some(error),
                }
            }
        }
    }

    fn request_page(&mut self) {
        let (Some(worker), Some(session)) = (&self.worker, &mut self.session) else {
            return;
        };
        let Some(document) = &session.document else {
            return;
        };
        session.texture = None;
        session.error = None;
        session.reset_scroll = true;
        session.generation = worker.request(Some(Work::Page(
            document.clone(),
            session.state.current_page(),
            session.max_dimension,
        )));
    }

    fn action(&mut self, action: Action) {
        if action == Action::Close {
            if let Some(session) = &mut self.session {
                session.state.apply(action);
            }
            self.session = None;
            if let Some(worker) = &self.worker {
                worker.request(None);
            }
            return;
        }
        let Some(session) = &mut self.session else {
            return;
        };
        // A bad page may be skipped. Source changes still fail every read against
        // the original document identity; only reopening can adopt a new file.
        let page = session.state.current_page();
        session.state.apply(action);
        if page != session.state.current_page() {
            self.request_page();
        }
    }

    fn reading(&self) -> Option<(PathBuf, DocumentReadingState)> {
        let session = self.session.as_ref()?;
        session.state.is_open().then(|| {
            (
                session.path.clone(),
                DocumentReadingState {
                    document_id: session.state.document().cloned(),
                    fit_mode: match session.state.zoom() {
                        ManualZoom::FitPage => Some(DocumentFitMode::Page),
                        ManualZoom::FitWidth => Some(DocumentFitMode::Width),
                        _ => None,
                    },
                    last_page: Some(session.state.page_number()),
                    zoom_percent: match session.state.zoom() {
                        ManualZoom::Percent(percent) => Some(percent),
                        _ => None,
                    },
                },
            )
        })
    }

    fn show(&mut self, context: &egui::Context) -> Option<(PathBuf, ManualDocumentId)> {
        self.poll(context);
        if let Some(action) = keyboard_action(context) {
            self.action(action);
            context.request_repaint();
        }
        let Some(session) = &mut self.session else {
            return None;
        };
        let mut action = None;
        let mut external = None;
        egui::CentralPanel::default().show(context, |ui| {
            ui.heading(&session.title);
            ui.horizontal_wrapped(|ui| {
                let enabled = session.state.is_open();
                for (label, command, available) in [
                    ("First", Action::FirstPage, session.state.can_go_previous()),
                    (
                        "Previous",
                        Action::PreviousPage,
                        session.state.can_go_previous(),
                    ),
                ] {
                    if ui
                        .add_enabled(enabled && available, egui::Button::new(label))
                        .clicked()
                    {
                        action = Some(command);
                    }
                }
                ui.label(format!(
                    "Page {} of {}",
                    session.state.page_number(),
                    session.state.page_count()
                ));
                for (label, command, available) in [
                    ("Next", Action::NextPage, session.state.can_go_next()),
                    ("Last", Action::LastPage, session.state.can_go_next()),
                    ("Zoom −", Action::ZoomOut, true),
                    ("Zoom +", Action::ZoomIn, true),
                    ("Fit Page", Action::FitPage, true),
                    ("Fit Width", Action::FitWidth, true),
                ] {
                    if ui
                        .add_enabled(enabled && available, egui::Button::new(label))
                        .clicked()
                    {
                        action = Some(command);
                    }
                }
                ui.label(match session.state.zoom() {
                    ManualZoom::FitPage => "Fit Page".into(),
                    ManualZoom::FitWidth => "Fit Width".into(),
                    ManualZoom::Percent(percent) => format!("{percent}%"),
                });
                if ui
                    .add_enabled(
                        session.external == DocumentOpenCapability::Supported
                            && session.expected.is_some(),
                        egui::Button::new("Open externally"),
                    )
                    .clicked()
                {
                    external = session
                        .expected
                        .clone()
                        .map(|id| (session.path.clone(), id));
                }
                if ui.button("Close").clicked() {
                    action = Some(Action::Close);
                }
            });
            if let Some(page) = session
                .document
                .as_ref()
                .and_then(|doc| doc.inspection().pages.get(session.state.current_page()))
            {
                ui.weak(&page.name);
            }
            if let Some(error) = &session.error {
                ui.colored_label(crate::ui::theme::WARNING, error);
                ui.label("Try another page, open externally, or close and reopen the document.");
            } else if let Some(texture) = &session.texture {
                let mut scroll = egui::ScrollArea::both()
                    .id_salt("manual-page")
                    .auto_shrink([false, false]);
                if std::mem::take(&mut session.reset_scroll) {
                    scroll = scroll.scroll_offset(egui::Vec2::ZERO);
                }
                scroll.show_viewport(ui, |ui, viewport| {
                    let size =
                        display_size(texture.size_vec2(), viewport.size(), session.state.zoom());
                    ui.add(egui::Image::new((texture.id(), size)));
                });
            } else {
                ui.spinner();
                ui.label(if session.document.is_some() {
                    "Reading page…"
                } else {
                    "Opening document…"
                });
            }
        });
        if let Some(action) = action {
            self.action(action);
            context.request_repaint();
        }
        external
    }
}

fn restore_zoom(state: &mut ManualViewerState, percent: Option<u16>) {
    let Some(percent) = percent else { return };
    // Traverse the core's supported steps; don't maintain a second zoom table.
    while state.apply(Action::ZoomOut) {}
    loop {
        if state.zoom() == ManualZoom::Percent(percent) {
            return;
        }
        if !state.apply(Action::ZoomIn) {
            break;
        }
    }
    state.apply(Action::FitPage);
}

fn display_size(original: egui::Vec2, available: egui::Vec2, zoom: ManualZoom) -> egui::Vec2 {
    let scale = match zoom {
        ManualZoom::FitPage => {
            (available.x.max(1.0) / original.x).min(available.y.max(1.0) / original.y)
        }
        ManualZoom::FitWidth => available.x.max(1.0) / original.x,
        ManualZoom::Percent(percent) => f32::from(percent) / 100.0,
    };
    original * scale
}

fn keyboard_action(context: &egui::Context) -> Option<Action> {
    context.input_mut(|input| {
        [
            (egui::Key::Escape, Action::Close),
            (egui::Key::ArrowLeft, Action::PreviousPage),
            (egui::Key::ArrowUp, Action::PreviousPage),
            (egui::Key::PageUp, Action::PreviousPage),
            (egui::Key::ArrowRight, Action::NextPage),
            (egui::Key::ArrowDown, Action::NextPage),
            (egui::Key::PageDown, Action::NextPage),
            (egui::Key::Home, Action::FirstPage),
            (egui::Key::End, Action::LastPage),
            (egui::Key::Plus, Action::ZoomIn),
            (egui::Key::Equals, Action::ZoomIn),
            (egui::Key::Minus, Action::ZoomOut),
        ]
        .into_iter()
        .find_map(|(key, action)| {
            input
                .consume_key(egui::Modifiers::NONE, key)
                .then_some(action)
        })
    })
}

impl super::App {
    pub(super) fn manual_document_actions(
        &mut self,
        ui: &mut egui::Ui,
        game_id: i64,
        document: &GameDocument,
    ) {
        let (internal, external) = open_buttons(ui, document);
        if internal.clicked() {
            self.manual_viewer.open_associated(
                ui.ctx(),
                document,
                self.document_preferences
                    .reading
                    .get(&document.path)
                    .cloned(),
            );
        }
        if external.clicked() {
            let Some(id) = document.document_id.clone() else {
                return;
            };
            let job =
                self.activity
                    .queue("Opening local document", super::Route::Game(game_id), false);
            self.send(
                job,
                super::Command::OpenDocument {
                    path: document.path.clone(),
                    expected: id,
                },
            );
        }
    }

    pub(super) fn show_manual_viewer(&mut self, context: &egui::Context) -> bool {
        if self.manual_viewer.session.is_none() {
            return false;
        }
        let before = self.manual_viewer.reading();
        if let Some((path, expected)) = self.manual_viewer.show(context) {
            let job =
                self.activity
                    .queue("Opening local document", self.router.current.clone(), false);
            self.send(job, super::Command::OpenDocument { path, expected });
        }
        if let Some((path, reading)) = self.manual_viewer.reading().or(before) {
            if self.document_preferences.reading.get(&path) != Some(&reading) {
                self.document_preferences.reading.insert(path, reading);
                self.preferences_dirty = Some(std::time::Instant::now());
            }
        }
        if self.manual_viewer.session.is_none() {
            self.document_cache = None;
        }
        // Even the closing frame belongs to the viewer: Escape must not also run Back.
        true
    }
}

#[cfg(test)]
pub(super) mod tests;
