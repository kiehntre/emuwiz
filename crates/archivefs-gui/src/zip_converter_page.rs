//! Small Converter surface for verified ZIP creation and extraction.

use std::path::PathBuf;

use archivefs_core::zip_converter::{
    ZipOperationResult, ZipPreview, compress_verified, extract_verified, preview_compress,
    preview_extract,
};
use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ZipMode {
    Compress,
    Extract,
}

pub(crate) struct ZipConverterPageState {
    mode: ZipMode,
    source: String,
    destination: String,
    preview: Option<ZipPreview>,
    result: Option<ZipOperationResult>,
    error: Option<String>,
}

impl Default for ZipConverterPageState {
    fn default() -> Self {
        Self {
            mode: ZipMode::Compress,
            source: String::new(),
            destination: String::new(),
            preview: None,
            result: None,
            error: None,
        }
    }
}

impl ZipConverterPageState {
    fn choose_source(&mut self) {
        let path = match self.mode {
            ZipMode::Compress => rfd::FileDialog::new().pick_file(),
            ZipMode::Extract => rfd::FileDialog::new()
                .add_filter("ZIP archive", &["zip"])
                .pick_file(),
        };
        if let Some(path) = path {
            self.source = path.display().to_string();
            self.preview = None;
            self.result = None;
            self.error = None;
        }
    }

    fn choose_folder_source(&mut self) {
        if self.mode == ZipMode::Compress
            && let Some(path) = rfd::FileDialog::new().pick_folder()
        {
            self.source = path.display().to_string();
            self.preview = None;
            self.result = None;
            self.error = None;
        }
    }

    fn choose_destination(&mut self) {
        let path = match self.mode {
            ZipMode::Compress => rfd::FileDialog::new()
                .add_filter("ZIP archive", &["zip"])
                .save_file(),
            ZipMode::Extract => rfd::FileDialog::new().pick_folder(),
        };
        if let Some(path) = path {
            self.destination = path.display().to_string();
            self.preview = None;
            self.result = None;
            self.error = None;
        }
    }

    fn set_mode(&mut self, mode: ZipMode) {
        if self.mode != mode {
            self.mode = mode;
            self.source.clear();
            self.destination.clear();
            self.preview = None;
            self.result = None;
            self.error = None;
        }
    }

    fn preview(&mut self) {
        self.error = None;
        self.result = None;
        let source = PathBuf::from(self.source.trim());
        let destination = PathBuf::from(self.destination.trim());
        if self.source.trim().is_empty() || self.destination.trim().is_empty() {
            self.error = Some("Choose a source and destination first.".into());
            return;
        }
        let result = match self.mode {
            ZipMode::Compress => preview_compress(&source, &destination),
            ZipMode::Extract => preview_extract(&source, &destination),
        };
        match result {
            Ok(preview) => self.preview = Some(preview),
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn execute(&mut self) {
        self.error = None;
        let source = PathBuf::from(self.source.trim());
        let destination = PathBuf::from(self.destination.trim());
        let result = match self.mode {
            ZipMode::Compress => compress_verified(&source, &destination),
            ZipMode::Extract => extract_verified(&source, &destination),
        };
        match result {
            Ok(result) => self.result = Some(result),
            Err(error) => self.error = Some(error.to_string()),
        }
    }
}

pub(crate) fn show(ui: &mut egui::Ui, state: &mut ZipConverterPageState) {
    ui.heading("ZIP Converter");
    ui.label("Create or unpack ZIP files with safe paths and verification.");
    ui.horizontal(|ui| {
        if ui
            .selectable_label(state.mode == ZipMode::Compress, "Compress")
            .clicked()
        {
            state.set_mode(ZipMode::Compress);
        }
        if ui
            .selectable_label(state.mode == ZipMode::Extract, "Extract")
            .clicked()
        {
            state.set_mode(ZipMode::Extract);
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Source:");
        ui.add_sized(
            [ui.available_width().clamp(220.0, 560.0), 24.0],
            egui::TextEdit::singleline(&mut state.source),
        );
        if ui.button("Choose file").clicked() {
            state.choose_source();
        }
        if state.mode == ZipMode::Compress && ui.button("Choose folder").clicked() {
            state.choose_folder_source();
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label(if state.mode == ZipMode::Compress {
            "ZIP destination:"
        } else {
            "Folder destination:"
        });
        ui.add_sized(
            [ui.available_width().clamp(220.0, 560.0), 24.0],
            egui::TextEdit::singleline(&mut state.destination),
        );
        if ui.button("Choose destination").clicked() {
            state.choose_destination();
        }
    });
    if ui.button("Preview").clicked() {
        state.preview();
    }
    if let Some(preview) = &state.preview {
        ui.separator();
        ui.label("Review the output before creating it. The source is unchanged and existing destinations are refused.");
        ui.label(format!(
            "{} files · {} bytes",
            preview
                .entries
                .iter()
                .filter(|entry| !entry.directory)
                .count(),
            preview.total_size
        ));
        ui.label(format!("Destination: {}", preview.destination.display()));
        egui::ScrollArea::vertical()
            .max_height(140.0)
            .show(ui, |ui| {
                for entry in preview.entries.iter().take(200) {
                    ui.label(format!(
                        "{}{} · {} bytes",
                        entry.name,
                        if entry.directory { "/" } else { "" },
                        entry.size
                    ));
                }
                if preview.entries.len() > 200 {
                    ui.label("Preview truncated; all entries are still verified.");
                }
            });
        if ui
            .button(if state.mode == ZipMode::Compress {
                "Create ZIP"
            } else {
                "Extract ZIP"
            })
            .clicked()
        {
            state.execute();
        }
    }
    if let Some(result) = &state.result {
        ui.colored_label(egui::Color32::from_rgb(90, 190, 110), "Verified");
        ui.label(if state.mode == ZipMode::Compress {
            "ZIP created and verified."
        } else {
            "ZIP extracted and verified."
        });
        ui.label(&result.verification);
    }
    if let Some(error) = &state.error {
        let collision = error.contains("Destination already exists");
        ui.colored_label(
            egui::Color32::from_rgb(220, 150, 80),
            if collision {
                "Blocked"
            } else {
                "Needs attention"
            },
        );
        if collision {
            ui.label("Blocked: this output already exists. EmuWiz will not replace it; choose another destination.");
        } else {
            ui.label(error);
        }
        crate::ui::components::technical_details(ui, ("zip-conversion-error",), |ui| {
            ui.label(error);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered_text(output: &egui::FullOutput) -> String {
        fn collect(shape: &egui::Shape, output: &mut String) {
            match shape {
                egui::Shape::Text(text) => {
                    output.push_str(text.galley.text());
                    output.push('\n');
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, output);
                    }
                }
                _ => {}
            }
        }
        let mut text = String::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut text);
        }
        text
    }

    #[test]
    fn mode_switch_clears_previous_preview() {
        let mut state = ZipConverterPageState {
            preview: Some(ZipPreview {
                source: "a".into(),
                destination: "b".into(),
                entries: Vec::new(),
                total_size: 0,
            }),
            ..Default::default()
        };
        state.set_mode(ZipMode::Extract);
        assert!(state.preview.is_none());
        assert!(state.source.is_empty());
    }

    #[test]
    fn zip_preview_explains_copy_safety_before_create() {
        let mut state = ZipConverterPageState {
            source: "games/source".into(),
            destination: "out.zip".into(),
            preview: Some(ZipPreview {
                source: "games/source".into(),
                destination: "out.zip".into(),
                entries: Vec::new(),
                total_size: 0,
            }),
            ..Default::default()
        };
        let context = egui::Context::default();
        let output = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| show(ui, &mut state));
        });
        let text = rendered_text(&output);
        assert!(text.contains("Review the output before creating it"));
        assert!(text.contains("source is unchanged"));
        assert!(text.contains("Destination: out.zip"));
        assert!(text.contains("Create ZIP"));
        assert!(state.result.is_none());
    }

    #[test]
    fn zip_destination_conflict_is_a_blocker_not_an_overwrite_action() {
        let mut state = ZipConverterPageState {
            error: Some("Destination already exists.".into()),
            ..Default::default()
        };
        let context = egui::Context::default();
        let output = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| show(ui, &mut state));
        });
        let text = rendered_text(&output);
        assert!(text.contains("Blocked"));
        assert!(text.contains("EmuWiz will not replace it"));
        assert!(!text.contains("Force"));
        assert!(state.result.is_none());
    }
}
