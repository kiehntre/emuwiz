//! A file and folder picker that works from a controller (Controller
//! Everywhere, slice 4).
//!
//! The native dialog is blocking and not drivable with a pad, so a site asks
//! for a pick with [`request`] and collects the answer with [`take`] on a later
//! frame. While a controller is in use the pick is an in-app list made of
//! ordinary focusable buttons (so the shared controller layer drives it);
//! otherwise the native dialog runs on a worker thread, which also stops it
//! freezing the window. A cancelled pick answers nothing.

use eframe::egui;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const PAGE: usize = 60;

#[derive(Clone, Debug)]
pub(crate) enum PickKind {
    Folder,
    /// Empty means any file.
    File(Vec<String>),
}

#[derive(Clone, Debug)]
pub(crate) struct PickRequest {
    pub(crate) kind: PickKind,
    pub(crate) title: String,
    pub(crate) start: Option<PathBuf>,
}

impl PickRequest {
    pub(crate) fn folder(title: &str) -> Self {
        Self {
            kind: PickKind::Folder,
            title: title.into(),
            start: None,
        }
    }
    pub(crate) fn file(title: &str, extensions: &[&str]) -> Self {
        Self {
            kind: PickKind::File(extensions.iter().map(|e| (*e).to_owned()).collect()),
            title: title.into(),
            start: None,
        }
    }
}

struct Active {
    slot: egui::Id,
    request: PickRequest,
    dir: PathBuf,
    entries: Vec<Entry>,
    page: usize,
    focus_first: bool,
    ids: Vec<egui::Id>,
}

#[derive(Clone)]
struct Entry {
    path: PathBuf,
    name: String,
    dir: bool,
}

#[derive(Default)]
struct Hub {
    answers: HashMap<egui::Id, PathBuf>,
    active: Option<Active>,
    native_running: bool,
}

type Shared = Arc<Mutex<Hub>>;

fn hub(context: &egui::Context) -> Shared {
    let id = egui::Id::new("picker_hub");
    context.data_mut(|d| d.get_temp_mut_or_insert_with(id, Shared::default).clone())
}

fn lock(hub: &Shared) -> std::sync::MutexGuard<'_, Hub> {
    hub.lock().unwrap_or_else(|e| e.into_inner())
}

/// Called every frame by the app: whether a controller is in use decides
/// between the in-app list and the native dialog.
pub(crate) fn set_controller_in_use(context: &egui::Context, in_use: bool) {
    context.data_mut(|d| d.insert_temp(egui::Id::new("picker_controller"), in_use));
}

fn controller_in_use(context: &egui::Context) -> bool {
    context
        .data(|d| d.get_temp::<bool>(egui::Id::new("picker_controller")))
        .unwrap_or(false)
}

/// Whether a pick is open or running for `slot`.
pub(crate) fn busy(context: &egui::Context, slot: egui::Id) -> bool {
    let hub = hub(context);
    let hub = lock(&hub);
    hub.native_running || hub.active.as_ref().is_some_and(|a| a.slot == slot)
}

pub(crate) fn request(context: &egui::Context, slot: egui::Id, request: PickRequest) {
    let shared = hub(context);
    if controller_in_use(context) {
        let dir = start_dir(request.start.as_deref());
        let mut hub = lock(&shared);
        hub.active = Some(Active {
            slot,
            entries: list(&dir, &request.kind),
            dir,
            request,
            page: 0,
            focus_first: true,
            ids: Vec::new(),
        });
        return;
    }
    {
        let mut hub = lock(&shared);
        if hub.native_running {
            return;
        }
        hub.native_running = true;
    }
    let context = context.clone();
    std::thread::spawn(move || {
        let mut dialog = rfd::FileDialog::new().set_title(&request.title);
        if let Some(start) = &request.start {
            dialog = dialog.set_directory(start);
        }
        let picked = match &request.kind {
            PickKind::Folder => dialog.pick_folder(),
            PickKind::File(extensions) => {
                if !extensions.is_empty() && extensions.iter().all(|e| e != "*") {
                    dialog = dialog.add_filter("Files", extensions);
                }
                dialog.pick_file()
            }
        };
        let mut hub = lock(&shared);
        hub.native_running = false;
        if let Some(path) = picked {
            hub.answers.insert(slot, path);
        }
        drop(hub);
        context.request_repaint();
    });
}

/// The path chosen for `slot`, once.
pub(crate) fn take(context: &egui::Context, slot: egui::Id) -> Option<PathBuf> {
    lock(&hub(context)).answers.remove(&slot)
}

fn start_dir(start: Option<&Path>) -> PathBuf {
    start
        .filter(|p| p.is_dir())
        .map(Path::to_path_buf)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_dir())
        })
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn list(dir: &Path, kind: &PickKind) -> Vec<Entry> {
    let mut out: Vec<Entry> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let path = entry.path();
            let is_dir = path.is_dir();
            let show = match kind {
                PickKind::Folder => is_dir,
                PickKind::File(extensions) => {
                    is_dir
                        || extensions.is_empty()
                        || extensions.iter().any(|e| {
                            e == "*"
                                || path
                                    .extension()
                                    .is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case(e))
                        })
                }
            };
            show.then_some(Entry {
                path,
                name,
                dir: is_dir,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

fn shortcuts() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        out.push(("Home".to_owned(), home));
    }
    for root in ["/run/media", "/media", "/mnt"] {
        let path = PathBuf::from(root);
        if path.is_dir() {
            out.push((root.to_owned(), path));
        }
    }
    out.push(("Computer".to_owned(), PathBuf::from("/")));
    out
}

enum Step {
    Open(PathBuf),
    Choose(PathBuf),
    Page(isize),
    Cancel,
}

/// Draw the in-app picker, if one is open. Call once a frame, before the pages.
pub(crate) fn show(context: &egui::Context) {
    let shared = hub(context);
    let Some(mut active) = lock(&shared).active.take() else {
        return;
    };
    // Keep focus inside the picker: the pad moves focus in tab order, which
    // would otherwise wander onto the page behind it.
    let focused = context.memory(|m| m.focused());
    if focused.is_none_or(|f| !active.ids.contains(&f)) && !active.ids.is_empty() {
        context.memory_mut(|m| m.request_focus(active.ids[0]));
    }
    let escape = context.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    let mut step = escape.then_some(Step::Cancel);
    let mut ids = Vec::new();
    let mut want_focus = std::mem::take(&mut active.focus_first);
    let modal = egui::Modal::new(egui::Id::new("picker_modal")).show(context, |ui| {
        ui.set_width(720.0);
        ui.label(
            egui::RichText::new(&active.request.title)
                .strong()
                .size(20.0),
        );
        ui.label(active.dir.display().to_string());
        ui.add_space(6.0);
        let mut row = |ui: &mut egui::Ui, text: &str| -> bool {
            let response = ui.add_sized(
                [ui.available_width(), 38.0],
                egui::Button::new(egui::RichText::new(text).size(17.0))
                    .wrap_mode(egui::TextWrapMode::Truncate),
            );
            ids.push(response.id);
            if std::mem::take(&mut want_focus) {
                response.request_focus();
            }
            if response.gained_focus() {
                response.scroll_to_me(None);
            }
            response.clicked()
        };
        ui.horizontal_wrapped(|ui| {
            for (name, path) in shortcuts() {
                if ui.button(&name).clicked() {
                    step = Some(Step::Open(path));
                }
            }
        });
        let is_folder = matches!(active.request.kind, PickKind::Folder);
        if is_folder && row(ui, "✔ Use this folder") {
            step = Some(Step::Choose(active.dir.clone()));
        }
        egui::ScrollArea::vertical()
            .max_height(380.0)
            .show(ui, |ui| {
                if let Some(parent) = active.dir.parent()
                    && row(ui, "⬆ Up one folder")
                {
                    step = Some(Step::Open(parent.to_path_buf()));
                }
                if active.page > 0 && row(ui, "◀ Previous page") {
                    step = Some(Step::Page(-1));
                }
                let from = active.page * PAGE;
                for entry in active.entries.iter().skip(from).take(PAGE) {
                    let label = if entry.dir {
                        format!("📁 {}", entry.name)
                    } else {
                        entry.name.clone()
                    };
                    if row(ui, &label) {
                        step = Some(if entry.dir {
                            Step::Open(entry.path.clone())
                        } else {
                            Step::Choose(entry.path.clone())
                        });
                    }
                }
                if from + PAGE < active.entries.len() && row(ui, "Next page ▶") {
                    step = Some(Step::Page(1));
                }
                if active.entries.is_empty() {
                    ui.label("Nothing to show here.");
                }
            });
        ui.add_space(6.0);
        if row(ui, "Cancel") {
            step = Some(Step::Cancel);
        }
    });
    if modal.should_close() && step.is_none() {
        step = Some(Step::Cancel);
    }
    active.ids = ids;
    match step {
        None => lock(&shared).active = Some(active),
        Some(Step::Cancel) => {}
        Some(Step::Choose(path)) => {
            lock(&shared).answers.insert(active.slot, path);
        }
        Some(Step::Open(dir)) => {
            active.entries = list(&dir, &active.request.kind);
            active.dir = dir;
            active.page = 0;
            active.focus_first = true;
            lock(&shared).active = Some(active);
        }
        Some(Step::Page(delta)) => {
            active.page = active.page.saturating_add_signed(delta);
            active.focus_first = true;
            lock(&shared).active = Some(active);
        }
    }
    context.request_repaint();
}

/// Whether the in-app picker is on screen (the page behind must stay inert).
pub(crate) fn open(context: &egui::Context) -> bool {
    lock(&hub(context)).active.is_some()
}
