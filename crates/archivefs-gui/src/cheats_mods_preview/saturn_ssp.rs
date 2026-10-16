//! Per-selected-game read-only SSP package/disc review. No application seam.
use archivefs_core::saturn_patch_readiness::ssp::{self, Preview};
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver},
    },
};

#[derive(Default)]
struct State {
    patch: String,
    loaded_patch: Option<PathBuf>,
    report: Option<Preview>,
    error: Option<String>,
    worker: Option<Receiver<Result<Preview, String>>>,
}
impl State {
    fn start(&mut self, disc: PathBuf, recheck: bool) {
        if self.worker.is_some() {
            return;
        }
        let previous = self.report.take();
        let patch = PathBuf::from(self.patch.trim());
        self.loaded_patch = Some(patch.clone());
        self.error = None;
        let (tx, rx) = mpsc::channel();
        self.worker = Some(rx);
        std::thread::spawn(move || {
            let result = if recheck {
                previous
                    .ok_or_else(|| "Inspect patch and disc first.".to_string())
                    .and_then(|r| ssp::revalidate(&r))
            } else {
                ssp::preview(&patch, &disc)
            };
            let _ = tx.send(result);
        });
    }
    fn poll(&mut self) {
        if let Some(receiver) = &self.worker {
            match receiver.try_recv() {
                Ok(result) => {
                    self.worker = None;
                    match result {
                        Ok(report) => self.report = Some(report),
                        Err(error) => self.error = Some(error),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.worker = None;
                    self.report = None;
                    self.error = Some(
                        "SSP inspection stopped. No compatibility or output is verified.".into(),
                    );
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self
            .loaded_patch
            .as_ref()
            .is_some_and(|p| p != &PathBuf::from(self.patch.trim()))
        {
            self.report = None;
        }
    }
    fn show(&mut self, ui: &mut egui::Ui, disc: &Path) {
        self.poll();
        ui.heading("Saturn SSP patch preview");
        ui.label("Inspect a local package against this selected disc. Preview only: original media is never patched, rebuilt or converted.");
        ui.label(format!("Selected media: {}", disc.display()));
        ui.add_enabled_ui(self.worker.is_none(), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Local SSP package");
                if ui.text_edit_singleline(&mut self.patch).changed() {
                    self.report = None;
                    self.error = None;
                    self.loaded_patch = None;
                }
                if ui.button("Choose SSP…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("SSP package", &["ssp"])
                        .pick_file()
                {
                    self.patch = path.display().to_string();
                    self.report = None;
                }
                if ui
                    .add_enabled(
                        !self.patch.trim().is_empty(),
                        egui::Button::new("Inspect SSP and disc"),
                    )
                    .clicked()
                {
                    self.start(disc.to_path_buf(), false);
                }
            });
        });
        if self.worker.is_some() {
            ui.spinner();
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        if let Some(error) = &self.error {
            ui.colored_label(crate::ui::theme::warning(ui), error);
        }
        if let Some(report) = &self.report {
            render(ui, report);
        }
        if ui
            .add_enabled(
                self.report.is_some() && self.worker.is_none(),
                egui::Button::new("Recheck source evidence"),
            )
            .clicked()
        {
            self.start(disc.to_path_buf(), true);
        }
        ui.add_enabled(false, egui::Button::new("Apply SSP (unsupported)"));
        ui.label(ssp::APPLY_BLOCKER);
    }
}

fn render(ui: &mut egui::Ui, report: &Preview) {
    ui.strong("Compatibility: Unknown — no verified SSP base binding");
    ui.label("An inspected ZIP and native Saturn identity do not prove that this patch fits this game or disc revision. Title/filename matches and per-file changes.md5 declarations are not disc compatibility evidence.");
    if let Some(id) = &report.manifest.system_id {
        ui.label(format!(
            "Observed native identity: {} · {} · {} · region {}",
            id.fact.product_number, id.fact.version, id.fact.device_info, id.fact.area_symbols
        ));
    }
    ui.monospace(format!("Disc manifest SHA-256: {}", report.manifest_sha256));
    ui.monospace(format!("Local package SHA-256: {}", report.package.sha256));
    ui.label("These hashes bind the exact observed local bytes; they do not authenticate the patch publisher or independently verify an official game release.");
    ui.label(if report.package.version_text.is_some() {
        "Packaging: SSP-style ZIP with a version.txt declaration; payload semantics unverified"
    } else {
        "Packaging: ZIP; SSP convention unconfirmed (no version.txt)"
    });
    if let Some(version) = &report.package.version_text {
        ui.label(format!(
            "version.txt declaration (untrusted, not an inferred application version): {version}"
        ));
    }
    if let Some(md5) = &report.package.changes_md5_text {
        ui.collapsing(
            "Declared changes.md5 text (not verified against disc files)",
            |ui| {
                ui.monospace(md5);
            },
        );
    }
    ui.collapsing(
        "Package entries — CRC checked and SHA-256 hashed, never extracted",
        |ui| {
            for member in &report.package.members {
                ui.monospace(format!(
                    "{} · {} bytes · {}",
                    member.name,
                    member.logical_bytes,
                    member.sha256.as_deref().unwrap_or("directory")
                ));
            }
        },
    );
    ui.collapsing("Original optical layout evidence", |ui| {
        for track in &report.manifest.tracks { ui.label(format!("Track {} · {:?} · {} sectors · INDEX00 {:?} · INDEX01 {} · stored pregap {:?} · generated pregap {:?} · {}", track.number, track.mode, track.sector_count, track.index_00_frame, track.index_01_frame, track.in_file_pregap_frames, track.declared_pregap_frames, track.source_file.display())); }
        ui.label("Stored pregap bytes and declared/generated gaps remain distinct. This preview writes no tracks or CUE files and makes no byte-exact output claim.");
    });
    ui.label("Requirements: complete CUE/BIN set, native Saturn System ID, local unencrypted stored/deflated ZIP; package ≤64 MiB, expanded ≤128 MiB, ≤512 entries, metadata text ≤16 KiB, disc components ≤1 GiB. UTF-8 member names/text only. Unknown payload features remain unsupported.");
    ui.hyperlink_to(
        "Patcher author's ZIP/version.txt guidance",
        ssp::FORMAT_REFERENCE,
    );
    ui.hyperlink_to(
        "Observed OpenROM producer layout (pinned source)",
        ssp::PRODUCER_REFERENCE,
    );
}

pub(super) fn show(ui: &mut egui::Ui, disc: &Path) {
    // Context-local state avoids extending another agent's shared workflow
    // model. The exact selected path scopes all jobs/results to this game.
    let state = ui.data_mut(|data| {
        data.get_temp_mut_or_default::<Arc<Mutex<State>>>(egui::Id::new((
            "saturn_ssp_preview",
            disc,
        )))
        .clone()
    });
    if let Ok(mut state) = state.try_lock() {
        state.show(ui, disc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_preview_surface_has_no_enabled_apply_or_implicit_read() {
        let mut state = State::default();
        let context = egui::Context::default();
        let output = context.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.show(ui, Path::new("/synthetic/not-read.cue"))
            });
        });
        assert!(state.worker.is_none());
        assert!(state.report.is_none());
        let shapes = format!("{:?}", output.shapes);
        assert!(shapes.contains("Saturn SSP patch preview"));
        assert!(shapes.contains("Apply SSP (unsupported)"));
        assert!(shapes.contains("Apply disabled"));
    }
}
