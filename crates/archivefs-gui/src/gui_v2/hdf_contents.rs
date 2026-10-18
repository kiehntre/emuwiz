//! Read-only HDF contents from Game Details. This panel owns no identity,
//! verification, import, extraction or launch actions.
use super::library::Game;
use crate::ui::components::card;
use archivefs_core::amiga_disk::contents::{Directory, DirectoryPage, Inspection, PAGE_ENTRIES};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, TryRecvError},
    },
    time::{Duration, Instant},
};

const EXPLANATION: &str = "Filesystem inspection only. Labels and filenames do not identify a game, verify a DAT match, or establish launch readiness. No mounting, repair, extraction or image writes.";
const LIMITS: &str = "OFS/FFS with 512-byte blocks only. PFS/SFS/MuFS and unknown filesystems are detection-only. Links are not followed. File data, free-space maps and full filesystem integrity are not checked. Listings are lazy: 128 entries per page, at most 4096 entries per directory, depth 32 and 10,000 directory nodes; each request has a bounded read budget. Escaped names are display-only.";

pub(super) fn applies(game: &Game) -> bool {
    game.archive
        .absolute_path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("hdf"))
}

struct Browser {
    inspection: Inspection,
    partition: usize,
    breadcrumbs: Vec<(String, Directory)>,
    offset: usize,
    page: Option<DirectoryPage>,
    error: Option<String>,
}

enum Action {
    Open(PathBuf),
    Root(Browser, usize),
    Directory(Browser, String, Directory),
    Up(Browser),
    Page(Browser, usize),
}

fn inspect(action: Action) -> Result<Browser, String> {
    let mut browser = match action {
        Action::Open(path) => {
            return Inspection::open(&path).map(|inspection| Browser {
                inspection,
                partition: 0,
                breadcrumbs: Vec::new(),
                offset: 0,
                page: None,
                error: None,
            });
        }
        Action::Root(mut browser, partition) => {
            browser.page = None;
            browser.error = None;
            let root = match browser.inspection.root(partition) {
                Ok(root) => root,
                Err(error) => {
                    browser.error = Some(error);
                    return Ok(browser);
                }
            };
            browser.partition = partition;
            browser.breadcrumbs = vec![("/".into(), root)];
            browser.offset = 0;
            browser
        }
        Action::Directory(mut browser, name, directory) => {
            browser.breadcrumbs.push((name, directory));
            browser.offset = 0;
            browser
        }
        Action::Up(mut browser) => {
            if browser.breadcrumbs.len() > 1 {
                browser.breadcrumbs.pop();
            }
            browser.offset = 0;
            browser
        }
        Action::Page(mut browser, offset) => {
            browser.offset = offset;
            browser
        }
    };
    let directory = browser
        .breadcrumbs
        .last()
        .ok_or("Select a partition first")?
        .1;
    browser.page = None;
    browser.error = None;
    match browser.inspection.list(directory, browser.offset) {
        Ok(page) => browser.page = Some(page),
        Err(error) => browser.error = Some(error),
    }
    Ok(browser)
}

struct Job {
    generation: u64,
    receiver: Receiver<Result<Browser, String>>,
}

#[derive(Default)]
struct Panel {
    key: Option<(i64, PathBuf)>,
    generation: u64,
    job: Option<Job>,
    browser: Option<Browser>,
    error: Option<String>,
    checked: Option<Instant>,
}

impl Panel {
    fn select(&mut self, key: (i64, PathBuf)) {
        if self.key.as_ref() != Some(&key) {
            self.key = Some(key);
            self.generation = self.generation.wrapping_add(1);
            self.browser = None;
            self.error = None;
            self.checked = None;
        }
    }

    fn poll(&mut self) {
        let Some(job) = &self.job else { return };
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                Err("HDF inspection worker stopped; inspect again.".into())
            }
        };
        let same = job.generation == self.generation;
        self.job = None;
        if !same {
            return;
        }
        match result {
            Ok(browser) if browser.inspection.is_current() => {
                self.browser = Some(browser);
                self.error = None;
            }
            Ok(_) => self.error = Some("Image changed during inspection. Inspect again.".into()),
            Err(error) => self.error = Some(error),
        }
        self.checked = Some(Instant::now());
    }

    fn recheck(&mut self) {
        if self
            .checked
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(1))
        {
            if self
                .browser
                .as_ref()
                .is_some_and(|b| !b.inspection.is_current())
            {
                self.browser = None;
                self.error = Some("Image changed, was replaced, or is unavailable. Inspect again after remounting.".into());
            }
            self.checked = Some(Instant::now());
        }
    }

    fn start(&mut self, ctx: &egui::Context, action: Action) {
        if self.job.is_some() {
            return;
        }
        let (tx, receiver) = mpsc::channel();
        self.job = Some(Job {
            generation: self.generation,
            receiver,
        });
        self.error = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| inspect(action)))
                .unwrap_or_else(|_| {
                    Err("HDF reader stopped; no contents conclusion is available.".into())
                });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
}

/// A single context-local controller bounds outstanding work across selections.
/// Session data is temporary and never persisted as catalogue evidence.
pub(super) fn show(ui: &mut egui::Ui, game: &Game) {
    if !applies(game) {
        return;
    }
    let shared = ui.ctx().data_mut(|data| {
        data.get_temp_mut_or_default::<Arc<Mutex<Panel>>>(egui::Id::new("hdf_contents_controller"))
            .clone()
    });
    let mut panel = shared.lock().unwrap();
    panel.select((game.archive.id, game.archive.absolute_path.clone()));
    panel.poll();
    panel.recheck();
    card(ui, |ui| {
        ui.heading("HDF contents · read-only");
        ui.label(EXPLANATION);
        ui.collapsing("Supported structures and limits", |ui| {
            ui.label(LIMITS);
        });
        let busy = panel.job.is_some();
        if ui
            .add_enabled(!busy, egui::Button::new("Inspect / refresh HDF contents"))
            .clicked()
        {
            panel.browser = None;
            panel.start(ui.ctx(), Action::Open(game.archive.absolute_path.clone()));
        }
        if busy {
            ui.spinner();
            ui.label("Reading bounded HDF metadata…");
        }
        if let Some(error) = &panel.error {
            ui.colored_label(ui.visuals().warn_fg_color, error);
        }
        let mut action = None;
        if let Some(browser) = &panel.browser {
            ui.label(format!(
                "Image: {} bytes · {} detected partition(s)",
                browser.inspection.disk().image_size,
                browser.inspection.disk().rdb.partitions.len()
            ));
            ui.label("Partition labels below come from container metadata; a volume label appears only after its filesystem is inspected.");
            for (index, partition) in browser.inspection.disk().rdb.partitions.iter().enumerate() {
                let label = partition
                    .name
                    .as_deref()
                    .unwrap_or("No partition label evidenced");
                let text = format!(
                    "Partition {}: {label} · {:?} · {} bytes at {}",
                    index + 1,
                    partition.filesystem,
                    partition.byte_length,
                    partition.byte_offset
                );
                if ui.button(text).clicked() {
                    action = Some((0, index, None));
                }
            }
            if let Some(error) = &browser.error {
                ui.colored_label(ui.visuals().warn_fg_color, error);
            }
            if let Some(page) = &browser.page {
                ui.separator();
                ui.label(format!(
                    "Volume: {} · {:?} · DOS{} · {}-byte blocks",
                    page.filesystem
                        .volume_label
                        .as_deref()
                        .filter(|label| !label.is_empty())
                        .unwrap_or("No safely representable volume label evidenced"),
                    page.filesystem.family,
                    page.filesystem.dos_type,
                    page.filesystem.block_size
                ));
                ui.label(format!(
                    "Directory: /{}",
                    browser
                        .breadcrumbs
                        .iter()
                        .skip(1)
                        .map(|(name, _)| name.as_str())
                        .collect::<Vec<_>>()
                        .join("/")
                ));
                if browser.breadcrumbs.len() > 1 && ui.button("Up one directory").clicked() {
                    action = Some((1, 0, None));
                }
                egui::ScrollArea::vertical()
                    .id_salt("hdf_directory_entries")
                    .max_height(320.0)
                    .show(ui, |ui| {
                        for entry in &page.entries {
                            ui.horizontal_wrapped(|ui| {
                                if let Some(directory) = entry.directory {
                                    if ui.button(&entry.name).clicked() {
                                        action =
                                            Some((2, 0, Some((entry.name.clone(), directory))));
                                    }
                                } else {
                                    ui.label(&entry.name);
                                }
                                ui.label(&entry.kind);
                                if let Some(size) = entry.size {
                                    ui.label(format!("{size} bytes (header)"));
                                }
                            });
                        }
                    });
                if page.entries.is_empty() {
                    ui.label("No entries returned in this directory page.");
                }
                if let Some(limit) = &page.limitation {
                    ui.colored_label(ui.visuals().warn_fg_color, limit);
                }
                ui.horizontal(|ui| {
                    if browser.offset > 0 && ui.button("Previous page").clicked() {
                        action = Some((3, browser.offset.saturating_sub(PAGE_ENTRIES), None));
                    }
                    if let Some(offset) = page.next_offset
                        && ui.button("Next page").clicked()
                    {
                        action = Some((3, offset, None));
                    }
                });
                ui.label("Observed metadata only; this is not a filesystem integrity check. Refresh after media changes or remounting.");
            }
        }
        if let Some((kind, index, directory)) = action
            && let Some(browser) = panel.browser.take()
        {
            let action = match kind {
                0 => Action::Root(browser, index),
                1 => Action::Up(browser),
                2 => {
                    let (name, directory) = directory.unwrap();
                    Action::Directory(browser, name, directory)
                }
                _ => Action::Page(browser, index),
            };
            panel.start(ui.ctx(), action);
        }
    });
    ui.ctx().request_repaint_after(Duration::from_secs(1));
}

#[cfg(test)]
#[path = "hdf_contents_tests.rs"]
mod tests;
