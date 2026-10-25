//! Read-only content analysis of a selected loose PSP ISO from Game Details.
//! Analysis only: no removal, ISO rewriting, conversion or output file. The
//! existing Converter owns ISO→CSO; this panel only links to it.
use super::library::Game;
use crate::ui::components::card;
use archivefs_core::psp_game_slimmer::{
    PspCandidateCategory, PspContentReport, PspSlimAnalysisError, analyze_psp_iso_report,
};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    time::{Duration, Instant},
};

const EXPLANATION: &str = "Read-only analysis of this ISO. Nothing is removed, rewritten or converted, and no file is created. Results describe what was observed in the image when you ran the analysis.";
const UNCERTAINTY: &str = "Categories come from file and directory names only. A name such as MOVIE or FRENCH does not prove the game never reads that data, so none of it is advice to remove anything. Reads are bounded (64 MiB, 100,000 entries, 16 directory levels); the whole image is not hashed.";
const LARGEST_SHOWN: usize = 10;

pub(super) fn applies(game: &Game) -> bool {
    game.archive.archive_kind != "zip"
        && game.platform.to_ascii_lowercase().contains("psp")
        && game
            .archive
            .absolute_path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("iso"))
}

type Outcome = Result<PspContentReport, PspSlimAnalysisError>;

struct Job {
    generation: u64,
    cancel: Arc<AtomicBool>,
    receiver: Receiver<Outcome>,
}

#[derive(Default)]
struct Panel {
    key: Option<(i64, PathBuf)>,
    generation: u64,
    job: Option<Job>,
    report: Option<PspContentReport>,
    error: Option<String>,
    checked: Option<Instant>,
}

impl Panel {
    fn cancel(&mut self) {
        if let Some(job) = self.job.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn select(&mut self, key: (i64, PathBuf)) {
        if self.key.as_ref() != Some(&key) {
            self.cancel();
            self.key = Some(key);
            self.generation = self.generation.wrapping_add(1);
            self.report = None;
            self.error = None;
            self.checked = None;
        }
    }

    fn poll(&mut self) {
        let Some(job) = &self.job else { return };
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(PspSlimAnalysisError::InvalidSource(
                "analysis worker stopped".into(),
            )),
        };
        let same = job.generation == self.generation;
        self.job = None;
        if !same {
            return;
        }
        let path = self.key.as_ref().map(|(_, path)| path.clone());
        match result {
            Ok(report) if path.is_some_and(|p| report.stamp.is_current(&p)) => {
                self.report = Some(report);
                self.error = None;
            }
            Ok(_) => self.error = Some("The ISO changed during analysis. Analyse again.".into()),
            Err(PspSlimAnalysisError::Cancelled) => self.error = Some("Analysis cancelled.".into()),
            Err(error) => self.error = Some(format!("No analysis available. {error}")),
        }
        self.checked = Some(Instant::now());
    }

    fn recheck(&mut self) {
        if self
            .checked
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(1))
        {
            if let (Some(report), Some((_, path))) = (&self.report, &self.key)
                && !report.stamp.is_current(path)
            {
                self.report = None;
                self.error =
                    Some("The ISO changed, was replaced or is unavailable. Analyse again.".into());
            }
            self.checked = Some(Instant::now());
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        let Some((_, path)) = self.key.clone() else {
            return;
        };
        if self.job.is_some() {
            return;
        }
        let (tx, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.job = Some(Job {
            generation: self.generation,
            cancel: cancel.clone(),
            receiver,
        });
        self.report = None;
        self.error = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                analyze_psp_iso_report(&path, cancel)
            }))
            .unwrap_or_else(|_| {
                Err(PspSlimAnalysisError::InvalidSource(
                    "analysis worker stopped".into(),
                ))
            });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
}

fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn percent(part: u64, whole: u64) -> String {
    if whole == 0 {
        "0%".into()
    } else {
        format!("{:.1}%", part as f64 * 100.0 / whole as f64)
    }
}

/// Returns true when the user asked to open the existing Converter.
pub(super) fn show(ui: &mut egui::Ui, game: &Game) -> bool {
    if !applies(game) {
        return false;
    }
    let shared = ui.ctx().data_mut(|data| {
        data.get_temp_mut_or_default::<Arc<Mutex<Panel>>>(egui::Id::new("psp_iso_analyser"))
            .clone()
    });
    let mut panel = shared.lock().unwrap();
    panel.select((game.archive.id, game.archive.absolute_path.clone()));
    panel.poll();
    panel.recheck();
    let mut open_converter = false;
    card(ui, |ui| {
        ui.heading("PSP ISO content analysis · read-only");
        ui.label(EXPLANATION);
        let busy = panel.job.is_some();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!busy, egui::Button::new("Analyse ISO contents"))
                .clicked()
            {
                panel.start(ui.ctx());
            }
            if busy && ui.button("Cancel analysis").clicked() {
                panel.cancel();
                panel.error = Some("Analysis cancelled.".into());
            }
        });
        if busy {
            ui.spinner();
            ui.label("Reading bounded ISO metadata in the background…");
        }
        if let Some(error) = &panel.error {
            ui.colored_label(ui.visuals().warn_fg_color, error);
        }
        if let Some(report) = &panel.report {
            show_report(ui, report);
        }
        ui.separator();
        ui.label("To make a smaller copy, use the existing verified ISO to CSO conversion. The original ISO is never changed.");
        if ui.button("Open Converter (ISO to CSO)").clicked() {
            open_converter = true;
        }
    });
    if panel.job.is_some() {
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    } else {
        ui.ctx().request_repaint_after(Duration::from_secs(1));
    }
    open_converter
}

fn show_report(ui: &mut egui::Ui, report: &PspContentReport) {
    let a = &report.analysis;
    let total = a.total_size;
    ui.label(format!(
        "Image: {} · {} entries scanned · {} read for this analysis",
        size(total),
        a.entries_scanned,
        size(a.read_bytes)
    ));
    if let Some(identity) = &a.identity {
        ui.label(format!(
            "From PARAM.SFO (observed, not verified): {} · {}",
            identity.title.as_deref().unwrap_or("no title"),
            identity.disc_id.as_deref().unwrap_or("no disc ID")
        ));
    }
    ui.strong("Where the space goes");
    egui::Grid::new("psp_analyser_categories").show(ui, |ui| {
        let mut rows: Vec<(String, usize, u64)> = a
            .category_totals
            .iter()
            .map(|t| (category_label(t.category), t.files, t.bytes))
            .collect();
        rows.sort_by_key(|row| std::cmp::Reverse(row.2));
        for (label, files, bytes) in rows {
            ui.label(label);
            ui.label(format!("{files} file(s)"));
            ui.label(size(bytes));
            ui.label(percent(bytes, total));
            ui.end_row();
        }
        ui.label("Other files (no name match)");
        ui.label("");
        ui.label(size(report.uncategorised_bytes));
        ui.label(percent(report.uncategorised_bytes, total));
        ui.end_row();
        ui.label("Not attributed to a file");
        ui.label("");
        ui.label(size(report.unattributed_bytes));
        ui.label(percent(report.unattributed_bytes, total));
        ui.end_row();
    });
    ui.strong("Largest entries");
    for file in a.largest_files.iter().take(LARGEST_SHOWN) {
        ui.label(format!("{} · {}", file.path, size(file.size)));
    }
    if a.largest_files.len() > LARGEST_SHOWN {
        ui.collapsing("More large entries", |ui| {
            for file in a.largest_files.iter().skip(LARGEST_SHOWN) {
                ui.label(format!("{} · {}", file.path, size(file.size)));
            }
        });
    }
    ui.strong("Storage estimates (qualified)");
    ui.label(format!(
        "Name-matched content, upper bound: {}. Not safe to remove, not an achievable saving, and nothing here removes it.",
        size(a.estimated_candidate_savings)
    ));
    match &report.compression {
        Some(c) => ui.label(format!(
            "ISO to CSO compression, sampled from {} of {} sectors: roughly {} to {} saved. The Converter reports the real size and verifies it.",
            c.sampled_blocks,
            c.total_blocks,
            size(c.estimated_saved_low),
            size(c.estimated_saved_high)
        )),
        None => ui.label("ISO to CSO compression could not be estimated; the Converter will measure it."),
    };
    ui.collapsing("Evidence and uncertainty", |ui| {
        ui.label(UNCERTAINTY);
        for note in report.notes.iter().chain(a.warnings.iter()) {
            ui.label(format!("• {note}"));
        }
        ui.label(format!(
            "Layout: PSP_GAME {} · SYSDIR/EBOOT.BIN {} · UMD_DATA.BIN {}",
            yes_no(a.layout.psp_game_dir_present),
            yes_no(a.layout.eboot_bin_present),
            yes_no(a.layout.umd_data_bin_present)
        ));
        ui.label("Whole-file SHA-256 was not computed; freshness is checked from size, modified time and file identity.");
    });
}

fn yes_no(value: bool) -> &'static str {
    if value { "seen" } else { "not seen" }
}

fn category_label(category: PspCandidateCategory) -> String {
    format!("Name suggests {}", category.label())
}

#[cfg(test)]
#[path = "psp_iso_analyser_tests.rs"]
mod tests;
