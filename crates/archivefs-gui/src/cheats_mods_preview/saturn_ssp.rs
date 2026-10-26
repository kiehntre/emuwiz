//! Per-selected-game read-only SSP package/disc review. No application seam.
use crate::saturn_evidence_cache::{Cache, Observed};
use archivefs_core::saturn_patch_readiness::ssp::{self, Preview};
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct State {
    patch: String,
    cache: Cache<(PathBuf, PathBuf), Preview>,
}
impl State {
    fn key(&self, disc: &Path) -> (PathBuf, PathBuf) {
        (disc.to_owned(), PathBuf::from(self.patch.trim()))
    }
    fn start(&mut self, disc: PathBuf, recheck: bool, context: egui::Context) {
        let key = self.key(&disc);
        self.cache.update(&key);
        let previous = if recheck {
            self.cache.value().and_then(|r| r.as_ref().ok()).cloned()
        } else {
            None
        };
        self.cache.invalidate();
        let patch = key.1.clone();
        self.cache.start(key, context, move || {
            Observed::inspect(&disc, Some(&patch), || {
                if recheck {
                    previous
                        .ok_or_else(|| "Inspect patch and disc first.".to_string())
                        .and_then(|r| ssp::revalidate(&r))
                } else {
                    ssp::preview(&patch, &disc)
                }
            })
        });
    }
    fn show(&mut self, ui: &mut egui::Ui, disc: &Path) {
        self.cache.update(&self.key(disc));
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
        ui.heading("Saturn SSP patch preview");
        ui.label("Inspect a local package against this selected disc. Preview only: original media is never patched, rebuilt or converted.");
        ui.label(format!("Selected media: {}", disc.display()));
        ui.add_enabled_ui(!self.cache.busy(), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Local SSP package");
                if ui.text_edit_singleline(&mut self.patch).changed() {
                    self.cache.update(&self.key(disc));
                }
                if ui.button("Choose SSP…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("SSP package", &["ssp"])
                        .pick_file()
                {
                    self.patch = path.display().to_string();
                    self.cache.update(&self.key(disc));
                }
                if ui
                    .add_enabled(
                        !self.patch.trim().is_empty(),
                        egui::Button::new("Inspect SSP and disc"),
                    )
                    .clicked()
                {
                    self.start(disc.to_path_buf(), false, ui.ctx().clone());
                }
            });
        });
        if self.cache.busy() {
            ui.spinner();
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        if let Some(error) = &self.cache.error {
            ui.colored_label(crate::ui::theme::warning(ui), error);
        }
        match self.cache.value() {
            Some(Ok(report)) => render(ui, report),
            Some(Err(error)) => {
                ui.colored_label(crate::ui::theme::warning(ui), error);
            }
            None if !self.cache.busy() && !self.patch.trim().is_empty() => {
                ui.label("No current patch/disc evidence. Inspect again after changing the patch or source files.");
            }
            _ => {}
        }
        if ui
            .add_enabled(
                self.cache.value().is_some_and(Result::is_ok) && !self.cache.busy(),
                egui::Button::new("Recheck source evidence"),
            )
            .clicked()
        {
            self.start(disc.to_path_buf(), true, ui.ctx().clone());
        }
        ui.add_enabled(false, egui::Button::new("Apply SSP (unsupported)"));
        ui.label(ssp::APPLY_BLOCKER);
    }
}

fn render(ui: &mut egui::Ui, report: &Preview) {
    ui.label("Source optical preflight: checked snapshot; source metadata unchanged");
    for limit in &report.optical_layout.limits {
        ui.label(limit);
    }
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
        for track in &report.manifest.tracks { ui.label(format!("Track {} · {:?} · {} sectors · INDEX00 {:?} · INDEX01 {} · stored pregap {:?} · generated pregap {:?} · POSTGAP {:?} · {}", track.number, track.mode, track.sector_count, track.index_00_frame, track.index_01_frame, track.in_file_pregap_frames, track.declared_pregap_frames, track.postgap_frames, track.source_file.display())); }
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
        assert!(!state.cache.busy());
        assert!(state.cache.value().is_none());
        let shapes = format!("{:?}", output.shapes);
        assert!(shapes.contains("Saturn SSP patch preview"));
        assert!(shapes.contains("Apply SSP (unsupported)"));
        assert!(shapes.contains("Apply disabled"));
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use std::{fs, io::Write};
    fn fixture(root: &Path) -> (PathBuf, PathBuf) {
        let cue = root.join("disc.cue");
        fs::write(&cue, "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\nFILE \"audio.bin\" BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n").unwrap();
        let mut bytes = vec![b' '; 2048];
        bytes[..16].copy_from_slice(b"SEGA SEGASATURN ");
        bytes[32..42].copy_from_slice(b"T-1234G   ");
        bytes[42..48].copy_from_slice(b"V1.000");
        bytes[56..64].copy_from_slice(b"CD-1/1  ");
        fs::write(root.join("data.bin"), bytes).unwrap();
        fs::write(root.join("audio.bin"), [0; 2352]).unwrap();
        let patch = root.join("patch.ssp");
        let mut zip = zip::ZipWriter::new(fs::File::create(&patch).unwrap());
        zip.start_file("version.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"synthetic package").unwrap();
        zip.finish().unwrap();
        (cue, patch)
    }
    #[test]
    fn ssp_preview_reuses_report_but_invalidates_sources_package_and_selection() {
        for change in ["audio.bin", "patch.ssp", "selection"] {
            let root = tempfile::tempdir().unwrap();
            let (cue, patch) = fixture(root.path());
            let mut state = State {
                patch: patch.display().to_string(),
                ..Default::default()
            };
            state.start(cue.clone(), false, Default::default());
            state.cache.finish_for_test();
            state.cache.update(&state.key(&cue));
            let report = state.cache.value().unwrap().as_ref().unwrap();
            assert!(!report.can_apply());
            assert!(report.optical_layout.refusals.is_empty());
            for _ in 0..10 {
                state.cache.update(&state.key(&cue));
                assert!(state.cache.value().is_some());
                assert!(!state.cache.busy());
            }
            if change == "selection" {
                state.patch = root.path().join("other.ssp").display().to_string();
            } else {
                std::thread::sleep(std::time::Duration::from_millis(10));
                let path = root.path().join(change);
                let mut bytes = fs::read(&path).unwrap();
                bytes[0] ^= 1;
                fs::write(path, bytes).unwrap();
            }
            state.cache.update(&state.key(&cue));
            assert!(state.cache.value().is_none(), "{change}");
        }
    }
}
