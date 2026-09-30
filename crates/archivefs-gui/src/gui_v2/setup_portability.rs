//! Settings-only setup portability workflow. Owns state, workers and rendering;
//! never applies an import or writes an emulator configuration.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use archivefs_core::setup_portability::{
    SetupCoverage, SetupImportPreview, SetupManifest, SetupPathKind, SetupPathRemaps,
    collect_setup_default, export_setup_new, preview_setup_import, read_setup_manifest,
};
use archivefs_core::source_root_migration::MigrationClassification;
use eframe::egui;

#[derive(Default)]
pub(super) struct SetupPortabilityState {
    export: Option<SetupManifest>,
    imported: Option<SetupManifest>,
    preview: Option<SetupImportPreview>,
    remaps: SetupPathRemaps,
    worker: Option<Receiver<Result<Outcome, String>>>,
    status: Option<String>,
    // Built from the live native window; headless tests leave this unset.
    dialog: Option<rfd::FileDialog>,
}

enum Outcome {
    Collected(SetupManifest),
    Imported(SetupManifest, SetupImportPreview),
    Reviewed(SetupImportPreview),
    Exported,
}

impl SetupPortabilityState {
    pub(super) fn set_dialog_parent(&mut self, parent: &eframe::CreationContext<'_>) {
        self.dialog = Some(rfd::FileDialog::new().set_parent(parent));
    }

    fn dialog(&self, title: &str) -> rfd::FileDialog {
        self.dialog.clone().unwrap_or_default().set_title(title)
    }

    pub(super) fn render(&mut self, ui: &mut egui::Ui) {
        self.poll();
        ui.separator();
        ui.heading("Move your setup to another device");
        ui.label("Export a setup summary, or review one from another device. Import preview leaves your current settings unchanged. Applying imports is not available yet.");
        ui.label(
            "Game files, passwords, saves and emulator configuration contents are not included.",
        );
        let busy = self.worker.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Export setup…").clicked() {
                    self.export = None;
                    self.start(ui.ctx(), || collect_setup_default().map(Outcome::Collected));
                }
                if ui.button("Preview setup file…").clicked() {
                    if let Some(path) = self
                        .dialog("Preview EmuWiz setup — no changes applied")
                        .add_filter("EmuWiz setup", &["json"])
                        .pick_file()
                    {
                        self.export = None;
                        self.imported = None;
                        self.preview = None;
                        self.remaps.clear();
                        self.start(ui.ctx(), move || {
                            let manifest = read_setup_manifest(&path)?;
                            let preview = preview_setup_import(&manifest, &SetupPathRemaps::new())?;
                            Ok(Outcome::Imported(manifest, preview))
                        });
                    } else {
                        self.status =
                            Some("Opening cancelled. Your settings were unchanged.".into());
                    }
                }
            });
        });
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Preparing setup summary…");
            });
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        if let Some(status) = &self.status {
            ui.label(status);
        }
        if let Some(manifest) = &self.export {
            ui.group(|ui| {
                ui.strong("Export preview");
                ui.label(export_summary(manifest));
                ui.label("The file contains your selected local folder locations. Review Advanced details before sharing it.");
                for notice in &manifest.notices {
                    if notice.coverage == SetupCoverage::RequiresAttention { ui.label(&notice.message); }
                }
                ui.collapsing("Not included in this export", |ui| {
                    for notice in &manifest.notices {
                        if notice.coverage == SetupCoverage::NotIncluded { ui.label(&notice.message); }
                    }
                });
                ui.collapsing("Advanced details — included preferences", |ui| {
                    preference_details(ui, manifest);
                });
                ui.collapsing("Advanced details — exported locations", |ui| {
                    for reference in manifest.paths() {
                        ui.label(format!("{}: {}", reference.label, reference.path.display()));
                    }
                    if let Some(romm) = &manifest.romm {
                        ui.label(format!("RomM server origin: {}", romm.server_origin.as_deref().unwrap_or("not available")));
                    }
                });
            });
            if ui
                .add_enabled(!busy, egui::Button::new("Save setup file…"))
                .clicked()
            {
                if let Some(path) = self
                    .dialog("Export EmuWiz setup")
                    .add_filter("EmuWiz setup", &["json"])
                    .set_file_name("emuwiz-setup.json")
                    .save_file()
                {
                    let manifest = manifest.clone();
                    self.start(ui.ctx(), move || {
                        export_setup_new(&path, &manifest).map(|_| Outcome::Exported)
                    });
                } else {
                    self.status = Some("Saving cancelled. No setup file was written.".into());
                }
            }
        }
        let mut remap = None;
        let location_dialog = self.dialog("Choose a location for this preview only");
        if let Some(preview) = &self.preview {
            ui.group(|ui| {
                ui.strong("Import preview — no changes applied");
                ui.heading("Reusable settings");
                if preview.reusable_settings.is_empty() { ui.label("No reusable preferences were recorded in this file."); }
                for setting in &preview.reusable_settings { ui.label(setting); }
                if let Some(manifest) = &self.imported {
                    ui.collapsing("Advanced details — included preferences", |ui| {
                        preference_details(ui, manifest);
                    });
                }
                ui.heading("Choose locations on this device");
                ui.label("Choose each local location, even if the original folder name looks familiar. These choices affect this preview only.");
                ui.add_enabled_ui(!busy, |ui| {
                    for review in &preview.paths {
                        ui.push_id(&review.reference.id, |ui| {
                            ui.group(|ui| {
                                ui.strong(&review.reference.label);
                                ui.label(path_summary(review.proposal.classification));
                                ui.horizontal_wrapped(|ui| {
                                    if ui.button("Choose local location…").clicked() {
                                        let dialog = location_dialog.clone();
                                        let path = if review.reference.kind == SetupPathKind::Directory { dialog.pick_folder() } else { dialog.pick_file() };
                                        if let Some(path) = path { remap = Some((review.reference.id.clone(), path)); }
                                        else { self.status = Some("Location choice cancelled. This preview and your settings were unchanged.".into()); }
                                    }
                                    if ui.button("Use original location").clicked() {
                                        remap = Some((review.reference.id.clone(), review.reference.path.clone()));
                                    }
                                });
                                ui.collapsing("Advanced details", |ui| {
                                    ui.label(format!("Original location: {}", review.reference.path.display()));
                                    if let Some(path) = &review.proposal.candidate_path { ui.label(format!("Chosen local location: {}", path.display())); }
                                    ui.label(&review.proposal.reason);
                                });
                            });
                        });
                    }
                });
                if !preview.missing_emulators.is_empty() {
                    ui.heading("Missing emulator or tool files");
                    for warning in &preview.missing_emulators { ui.label(warning); }
                    ui.label("Choose another executable location or install the emulator separately, then check it in Setup.");
                }
                ui.heading("Needs your attention");
                for warning in &preview.attention { ui.label(warning); }
                ui.collapsing("Saves, emulator configs and unsupported items", |ui| {
                    for warning in &preview.not_included { ui.label(warning); }
                });
                ui.label("Next: use Setup and Sources & Providers to configure this device explicitly. This preview cannot apply settings.");
            });
        }
        if let Some((id, path)) = remap
            && let Some(manifest) = self.imported.clone()
        {
            self.remaps.insert(id, path);
            let remaps = self.remaps.clone();
            // Hide the old result immediately: it is not evidence for the new
            // location while the read-only worker is still checking it.
            self.preview = None;
            self.start(ui.ctx(), move || {
                preview_setup_import(&manifest, &remaps).map(Outcome::Reviewed)
            });
        }
    }

    fn start(
        &mut self,
        context: &egui::Context,
        work: impl FnOnce() -> Result<Outcome, String> + Send + 'static,
    ) {
        let (sender, receiver) = mpsc::channel();
        let context = context.clone();
        self.worker = Some(receiver);
        self.status = None;
        std::thread::spawn(move || {
            let _ = sender.send(work());
            context.request_repaint();
        });
    }

    fn poll(&mut self) {
        let Some(worker) = &self.worker else {
            return;
        };
        match worker.try_recv() {
            Ok(result) => {
                self.worker = None;
                match result {
                    Ok(Outcome::Collected(manifest)) => {
                        self.imported = None;
                        self.preview = None;
                        self.remaps.clear();
                        self.export = Some(manifest);
                    }
                    Ok(Outcome::Imported(manifest, preview)) => {
                        self.remaps.clear();
                        self.export = None;
                        self.imported = Some(manifest);
                        self.preview = Some(preview);
                    }
                    Ok(Outcome::Reviewed(preview)) => self.preview = Some(preview),
                    Ok(Outcome::Exported) => {
                        self.status = Some(
                            "Setup file saved. Your settings and game files were unchanged.".into(),
                        )
                    }
                    Err(error) => self.status = Some(error),
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.worker = None;
                self.status = Some("Setup preview could not finish. Try again.".into());
            }
        }
    }
}

fn preference_details(ui: &mut egui::Ui, manifest: &SetupManifest) {
    for (index, source) in manifest.library.sources.iter().enumerate() {
        ui.label(format!(
            "Game source {}: {}",
            index + 1,
            if source.enabled {
                "enabled"
            } else {
                "disabled"
            }
        ));
    }
    for dat in &manifest.dat_sources {
        ui.label(format!(
            "DAT {}: {} · priority {}",
            dat.display_name,
            if dat.enabled.unwrap_or(true) {
                "enabled"
            } else {
                "disabled"
            },
            dat.priority
                .map_or_else(|| "default".into(), |value| value.to_string())
        ));
    }
    if let Some(policy) = &manifest.dat_policy {
        // Display only typed preference fields; arbitrary future TOML is never
        // projected. The original ordering of region/language choices matters.
        policy_details(
            ui,
            "All systems",
            &[
                (
                    "Regions",
                    policy
                        .region_preferences
                        .as_ref()
                        .map(|values| values.join(" → ")),
                ),
                (
                    "Languages",
                    policy
                        .language_preferences
                        .as_ref()
                        .map(|values| values.join(" → ")),
                ),
                ("Content", policy.content_selection.clone()),
                ("Revision choice", policy.revision_policy.clone()),
                ("Clone choice", policy.clone_policy.clone()),
            ],
        );
        if let Some(platforms) = &policy.platforms {
            for (platform, policy) in platforms {
                policy_details(
                    ui,
                    platform,
                    &[
                        (
                            "Regions",
                            policy
                                .region_preferences
                                .as_ref()
                                .map(|values| values.join(" → ")),
                        ),
                        (
                            "Languages",
                            policy
                                .language_preferences
                                .as_ref()
                                .map(|values| values.join(" → ")),
                        ),
                        ("Content", policy.content_selection.clone()),
                        ("Revision choice", policy.revision_policy.clone()),
                        ("Clone choice", policy.clone_policy.clone()),
                    ],
                );
            }
        }
    }
    if let Some(romm) = &manifest.romm {
        ui.label(format!(
            "RomM: {} · page size {} · import timeout {} seconds",
            if romm.enabled { "enabled" } else { "disabled" },
            romm.page_size
                .map_or_else(|| "default".into(), |value| value.to_string()),
            romm.import_timeout_seconds
                .map_or_else(|| "default".into(), |value| value.to_string())
        ));
    }
}

fn policy_details(ui: &mut egui::Ui, scope: &str, values: &[(&str, Option<String>)]) {
    for (label, value) in values {
        if let Some(value) = value {
            ui.label(format!("{scope} · {label}: {value}"));
        }
    }
}

fn export_summary(manifest: &SetupManifest) -> String {
    format!(
        "{} game sources · {} DAT registrations · {} manual emulator selections{}",
        manifest.library.sources.len(),
        manifest.dat_sources.len(),
        manifest.emulators.len(),
        if manifest.dat_policy.is_some() {
            " · DAT matching preferences"
        } else {
            ""
        }
    )
}

fn path_summary(classification: MigrationClassification) -> &'static str {
    match classification {
        MigrationClassification::AlreadyCurrent => {
            "Location exists. Contents and compatibility still need checking."
        }
        MigrationClassification::TargetMissing => "Location missing on this device.",
        _ => "Location needs your review.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_copy_describes_summary_without_claiming_full_backup() {
        let manifest = SetupManifest::default();
        assert_eq!(
            export_summary(&manifest),
            "0 game sources · 0 DAT registrations · 0 manual emulator selections"
        );
        assert!(!export_summary(&manifest).contains("backup"));
    }

    #[test]
    fn existing_location_copy_does_not_claim_verified_installation() {
        assert_eq!(
            path_summary(MigrationClassification::AlreadyCurrent),
            "Location exists. Contents and compatibility still need checking."
        );
        assert_eq!(
            path_summary(MigrationClassification::TargetMissing),
            "Location missing on this device."
        );
    }

    #[test]
    fn replacing_import_resets_location_choices() {
        let manifest = SetupManifest::default();
        let preview = preview_setup_import(&manifest, &SetupPathRemaps::new()).unwrap();
        let (sender, receiver) = mpsc::channel();
        let mut state = SetupPortabilityState {
            worker: Some(receiver),
            export: Some(SetupManifest::default()),
            ..Default::default()
        };
        state
            .remaps
            .insert("old.file.location".into(), "/old/location".into());
        sender
            .send(Ok(Outcome::Imported(manifest.clone(), preview)))
            .unwrap();
        state.poll();
        assert_eq!(state.imported, Some(manifest));
        assert!(state.export.is_none());
        assert!(state.remaps.is_empty());
        assert!(state.preview.as_ref().unwrap().read_only);
        assert!(state.worker.is_none());
    }

    #[test]
    fn preparing_export_replaces_import_preview_and_transient_choices() {
        let manifest = SetupManifest::default();
        let preview = preview_setup_import(&manifest, &SetupPathRemaps::new()).unwrap();
        let (sender, receiver) = mpsc::channel();
        let mut state = SetupPortabilityState {
            worker: Some(receiver),
            imported: Some(manifest.clone()),
            preview: Some(preview),
            remaps: [("old.location".into(), "/old/path".into())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        sender
            .send(Ok(Outcome::Collected(manifest.clone())))
            .unwrap();
        state.poll();
        assert_eq!(state.export, Some(manifest));
        assert!(state.imported.is_none());
        assert!(state.preview.is_none());
        assert!(state.remaps.is_empty());
        let strings = rendered_text(&mut state, [1280.0, 900.0]);
        assert!(strings.iter().any(|text| text == "Export preview"));
        assert!(!strings.iter().any(|text| text.contains("Import preview —")));
    }

    #[test]
    fn worker_failure_remains_visible() {
        let (sender, receiver) = mpsc::channel();
        let mut state = SetupPortabilityState {
            worker: Some(receiver),
            ..Default::default()
        };
        sender.send(Err("Setup file invalid.".into())).unwrap();
        state.poll();
        assert_eq!(state.status.as_deref(), Some("Setup file invalid."));
        assert!(state.worker.is_none());
    }

    fn rendered_text(state: &mut SetupPortabilityState, size: [f32; 2]) -> Vec<String> {
        fn append(shape: &egui::Shape, output: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => output.push(text.galley.text().to_string()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        append(shape, output);
                    }
                }
                _ => {}
            }
        }
        let context = egui::Context::default();
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size.into())),
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    state.render(ui);
                });
            },
        );
        let mut strings = Vec::new();
        for shape in output.shapes {
            append(&shape.shape, &mut strings);
        }
        strings
    }

    #[test]
    fn import_render_is_plain_preview_and_keeps_original_paths_under_advanced_details() {
        let manifest = SetupManifest {
            library: archivefs_core::setup_portability::SetupLibrary {
                sources: vec![archivefs_core::setup_portability::SetupSource {
                    path: "/private/home/user/games".into(),
                    enabled: false,
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let preview = preview_setup_import(&manifest, &SetupPathRemaps::new()).unwrap();
        let mut state = SetupPortabilityState {
            imported: Some(manifest),
            preview: Some(preview),
            ..Default::default()
        };
        for size in [[1280.0, 900.0], [620.0, 900.0]] {
            let strings = rendered_text(&mut state, size);
            for expected in [
                "Import preview — no changes applied",
                "Choose locations on this device",
                "Game source 1 (disabled)",
                "Use original location",
            ] {
                assert!(
                    strings.iter().any(|text| text.contains(expected)),
                    "missing {expected} at {size:?}"
                );
            }
            assert!(
                !strings
                    .iter()
                    .any(|text| text.contains("/private/home/user"))
            );
            assert!(
                !strings
                    .iter()
                    .any(|text| text == "Apply" || text == "Import settings")
            );
        }
    }

    #[test]
    fn export_render_requires_a_prepared_summary_before_saving() {
        let mut state = SetupPortabilityState::default();
        let strings = rendered_text(&mut state, [1280.0, 900.0]);
        assert!(!strings.iter().any(|text| text.contains("Save setup file")));
        state.export = Some(SetupManifest::default());
        let strings = rendered_text(&mut state, [1280.0, 900.0]);
        assert!(strings.iter().any(|text| text.contains("Export preview")));
        assert!(strings.iter().any(|text| text.contains("Save setup file")));
    }
}
