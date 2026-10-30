//! Read-only ZIP package inspection. No composition planner or writer is exposed.
use archivefs_core::{
    launch::process_spawn::open_regular_file_nonblocking,
    patch_package_composition::{MAX_PACKAGE_BYTES, PatchPackageInspection, inspect_patch_package},
};
use eframe::egui;
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};
const LIMIT: &str = "Inspect a ZIP snapshot only. Source files and games are unchanged. A private temporary copy is discarded after inspection. Patch structure and package metadata do not establish base-game identity, compatibility or a safe patch order. To create a verified derived copy, select a game and open Mods & Cheats, then Available packages.";
#[derive(Default)]
pub(super) struct State {
    path: Option<PathBuf>,
    loading: Option<Receiver<Result<PatchPackageInspection, String>>>,
    report: Option<PatchPackageInspection>,
    error: Option<String>,
}
pub(super) fn inspect(path: &Path) -> Result<PatchPackageInspection, String> {
    // The general inspector reopens its input. Supply a bounded private copy,
    // so replacement of an external package cannot change the bytes it inspects.
    let file = open_regular_file_nonblocking(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_PACKAGE_BYTES {
        return Err("ZIP exceeds the 512 MiB package limit.".into());
    }
    let mut temp = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    let copied = std::io::copy(&mut file.take(MAX_PACKAGE_BYTES + 1), &mut temp)
        .map_err(|e| e.to_string())?;
    if copied > MAX_PACKAGE_BYTES {
        return Err("ZIP grew beyond the package limit.".into());
    }
    let mut report = inspect_patch_package(temp.path()).map_err(|e| e.to_string())?;
    report.package_path = path.to_path_buf();
    Ok(report)
}
impl State {
    fn start(&mut self, path: PathBuf, context: &egui::Context) {
        self.path = Some(path.clone());
        self.report = None;
        self.error = None;
        let (tx, rx) = mpsc::channel();
        self.loading = Some(rx);
        let context = context.clone();
        std::thread::spawn(move || {
            let _ = tx.send(inspect(&path));
            context.request_repaint();
        });
    }
    fn poll(&mut self) {
        let Some(rx) = &self.loading else { return };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("Package reader stopped. Inspect again.".into()),
        };
        self.loading = None;
        match result {
            Ok(report) => self.report = Some(report),
            Err(error) => self.error = Some(error),
        }
    }
}
pub(super) fn show(ui: &mut egui::Ui, state: &mut State) {
    state.poll();
    ui.heading("Patch-package inspection");
    ui.label(LIMIT);
    ui.label("Limits: 512 entries, 512 MiB expanded bytes, 16 directory levels, 1 MiB metadata. Folder packages are not offered here.");
    ui.horizontal(|ui| {
        if ui
            .add_enabled(state.loading.is_none(), egui::Button::new("Choose ZIP…"))
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("ZIP package", &["zip"])
                .pick_file()
        {
            state.start(path, ui.ctx());
        }
        if ui
            .add_enabled(
                state.loading.is_none() && state.path.is_some(),
                egui::Button::new("Inspect again"),
            )
            .clicked()
        {
            state.start(state.path.clone().unwrap(), ui.ctx());
        }
        if ui.button("Clear").clicked() {
            *state = State::default();
        }
    });
    if state.loading.is_some() {
        ui.spinner();
    }
    if let Some(path) = &state.path {
        ui.label(format!("Snapshot from {}", path.display()));
    }
    if let Some(error) = &state.error {
        ui.label(error);
    }
    if let Some(report) = &state.report {
        ui.label(format!(
            "{} entries · {} expanded bytes · {} patches",
            report.entries,
            report.expanded_bytes,
            report.patches.len()
        ));
        ui.label(format!("Observed ZIP SHA-256: {}", report.package_sha256));
        ui.label("Results remain an observation of the copied bytes, even if the external ZIP later changes. Inspect again for a new snapshot.");
        for warning in &report.warnings {
            ui.label(warning);
        }
        for patch in &report.patches {
            ui.collapsing(
                format!(
                    "{} · {:?} · {:?}",
                    patch.relative_path, patch.inspection.format, patch.inspection.state
                ),
                |ui| {
                    ui.label(format!(
                        "{} bytes · SHA-256 {}",
                        patch.inspection.patch_size, patch.inspection.patch_sha256
                    ));
                    if let Some(error) = &patch.inspection.error {
                        ui.label(error);
                    }
                    for warning in &patch.inspection.warnings {
                        ui.label(warning);
                    }
                },
            );
        }
        if let Some(order) = &report.metadata_patch_order {
            ui.label(format!(
                "Unverified package order claim: {}",
                order.join(" -> ")
            ));
        }
        if let Some(identity) = &report.metadata_source_identity {
            ui.label(format!("Unverified source identity claim: {identity}"));
        }
        if let Some(metadata) = &report.metadata {
            ui.collapsing("Package metadata (untrusted text)", |ui| {
                ui.label(metadata);
            });
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn zip_preview_inspects_nested_patch_and_preserves_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.start_file("nested/test.ips", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"PATCH\0\0\0\0\x01\x07EOF").unwrap();
        zip.finish().unwrap();
        let before = std::fs::read(&path).unwrap();
        let report = inspect(&path).unwrap();
        assert_eq!(report.patches.len(), 1);
        assert_eq!(report.patches[0].relative_path, "nested/test.ips");
        assert!(report.no_changes_made);
        assert_eq!(report.package_path, path);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        let mut state = State {
            path: Some(path),
            report: Some(report),
            ..Default::default()
        };
        let context = egui::Context::default();
        let _ = context.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, &mut state));
        });
    }
    #[test]
    fn oversized_and_non_zip_inputs_fail_without_results() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.zip");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_PACKAGE_BYTES + 1).unwrap();
        assert!(inspect(&path).unwrap_err().contains("limit"));
        assert!(inspect(dir.path()).is_err());
        std::fs::write(&path, b"not a zip").unwrap();
        assert!(inspect(&path).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn fifo_never_blocks_the_reader() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pipe.zip");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        assert!(inspect(&path).is_err());
    }
}
