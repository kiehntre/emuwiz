//! Offline catalogue browsing over the existing validated Core provider.
use archivefs_core::{
    homebrew_artifact::ProjectDiscoveryProvider, homebrew_catalogue::LocalCatalogueProvider,
};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
};

const LIMIT: &str = "Local metadata snapshot only. Titles, platform, rights and download links are unverified catalogue claims, not game identity or permission to acquire or redistribute. No download or installation is offered.";

#[derive(Default)]
pub(super) struct State {
    path: Option<PathBuf>,
    provider: Option<LocalCatalogueProvider>,
    loading: Option<Receiver<Result<LocalCatalogueProvider, String>>>,
    query: String,
    platform: String,
    feedback: Option<String>,
}

impl State {
    fn load(&mut self, path: PathBuf, context: &egui::Context) {
        // Dropping the old receiver prevents a late result replacing this selection.
        self.provider = None;
        self.feedback = None;
        self.path = Some(path.clone());
        let (tx, rx) = mpsc::channel();
        self.loading = Some(rx);
        let context = context.clone();
        std::thread::spawn(move || {
            let result = LocalCatalogueProvider::from_path(path).map_err(|e| e.to_string());
            let _ = tx.send(result);
            context.request_repaint();
        });
    }

    fn poll(&mut self) {
        let Some(rx) = &self.loading else { return };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                Err("Catalogue reader stopped. Choose the file again.".into())
            }
        };
        self.loading = None;
        match result {
            Ok(provider) => self.provider = Some(provider),
            Err(error) => self.feedback = Some(error),
        }
    }
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut State) {
    state.poll();
    ui.heading("Offline homebrew catalogue");
    ui.label(LIMIT);
    ui.label("Choose an EmuWiz local catalogue (schema 1 JSON, at most 4 MiB). Reload explicitly to see file changes.");
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                state.loading.is_none(),
                egui::Button::new("Choose catalogue…"),
            )
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Catalogue", &["json"])
                .pick_file()
        {
            state.load(path, ui.ctx());
        }
        if ui
            .add_enabled(
                state.loading.is_none() && state.path.is_some(),
                egui::Button::new("Reload"),
            )
            .clicked()
        {
            state.load(state.path.clone().unwrap(), ui.ctx());
        }
        if ui.button("Clear").clicked() {
            *state = State::default();
        }
    });
    if let Some(path) = &state.path {
        ui.label(format!("Snapshot from {}", path.display()));
    }
    if state.loading.is_some() {
        ui.spinner();
    }
    if let Some(error) = &state.feedback {
        ui.label(error);
    }
    ui.horizontal(|ui| {
        ui.label("Search title or description");
        ui.text_edit_singleline(&mut state.query);
    });
    ui.horizontal(|ui| {
        ui.label("Platform claim (exact registered name, or leave empty)");
        ui.text_edit_singleline(&mut state.platform);
    });
    let Some(provider) = &state.provider else {
        return;
    };
    let projects = if state.platform.trim().is_empty() {
        provider.discover_projects(&state.query)
    } else {
        provider.discover_for_platform(&state.query, state.platform.trim())
    };
    let projects = match projects {
        Ok(projects) => projects,
        Err(error) => {
            ui.label(error.to_string());
            return;
        }
    };
    ui.label(format!(
        "{} matches; showing at most 100. Narrow your search for more.",
        projects.len()
    ));
    for claim in provider.unregistered_platform_claims().iter().take(20) {
        ui.label(format!(
            "Unregistered platform claim: {} ({})",
            claim.platform, claim.project.provider_id
        ));
    }
    for project in projects.iter().take(100) {
        ui.push_id(&project.id, |ui| {
            ui.collapsing(&project.title, |ui| {
                ui.label(format!(
                    "Provider: {} · Project: {}",
                    project.id.provider, project.id.provider_id
                ));
                if let Some(summary) = &project.summary {
                    ui.label(summary);
                }
                ui.label(format!("Source: {}", project.source_page_url));
                if let Ok(record) = provider.record(&project.id) {
                    for observation in &record.observations {
                        ui.label(format!(
                            "Observed {} · {} · {}",
                            observation.observed_on, observation.source_url, observation.note
                        ));
                    }
                    for artifact in record.listings.iter().flat_map(|l| &l.artifacts) {
                        ui.label(format!(
                            "Artifact claim: {} · {:?}",
                            artifact.filename, artifact.platform_claims
                        ));
                    }
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browse_real_catalogue_and_filter_without_touching_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalogue.json");
        let bytes = br#"{"schema_version":1,"provider":"local","records":[{"project":{"id":{"provider":"local","provider_id":"puzzle"},"title":"Moon Garden","summary":"A puzzle","source_page_url":"https://example.org/project"},"listings":[],"observations":[{"source_url":"https://example.org/project","observed_on":"2026-10-11","note":"Unverified fixture"}]}]}"#;
        std::fs::write(&path, bytes).unwrap();
        let provider = LocalCatalogueProvider::from_path(&path).unwrap();
        assert_eq!(provider.discover_projects("puzzle").unwrap().len(), 1);
        assert!(
            provider
                .discover_for_platform("", "NES")
                .unwrap()
                .is_empty()
        );
        assert!(provider.discover_for_platform("", "invented").is_err());
        let mut state = State {
            provider: Some(provider),
            path: Some(path.clone()),
            ..Default::default()
        };
        let context = egui::Context::default();
        let _ = context.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, &mut state));
        });
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[test]
    fn replacement_load_clears_old_snapshot_and_ignores_old_receiver() {
        let (tx, rx) = mpsc::channel();
        let mut state = State {
            loading: Some(rx),
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        state.load(dir.path().join("missing"), &egui::Context::default());
        assert!(tx.send(Err("old result".into())).is_err());
        assert!(state.provider.is_none());
    }
}
