//! Homebrew: browse a built-in offline catalogue (and optionally another local
//! catalogue file) and look up GitHub Releases for a known repository on
//! explicit request. Discovery only: nothing is downloaded, installed or run.
use archivefs_core::{
    homebrew_artifact::ProjectDiscoveryProvider, homebrew_catalogue::LocalCatalogueProvider,
};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
};

mod github;
mod view;

use view::Entry;

const BUILTIN_SOURCE: &str = "Built-in starter catalogue (offline)";
const LIMIT: &str = "Titles, platforms, rights and links are recorded claims with a stated basis, not verified game identity or permission to acquire or redistribute. Nothing here is downloaded, installed or run.";
const NETWORK_NOTE: &str = "Looking up GitHub releases contacts api.github.com once, when you press the button. It reads release metadata only and downloads no files.";

type LocalResult = Result<Vec<Entry>, String>;

#[derive(Default)]
pub(super) struct State {
    builtin: Option<Result<Vec<Entry>, String>>,
    path: Option<PathBuf>,
    local: Vec<Entry>,
    loading: Option<Receiver<LocalResult>>,
    github: Vec<Entry>,
    github_pending: Option<github::Pending>,
    github_input: String,
    github_error: Option<String>,
    query: String,
    platform: Option<String>,
    feedback: Option<String>,
}

fn builtin_entries() -> Result<Vec<Entry>, String> {
    entries_of(&LocalCatalogueProvider::bundled_starter(), BUILTIN_SOURCE)
}

fn entries_of(
    provider: &Result<
        LocalCatalogueProvider,
        archivefs_core::homebrew_catalogue::LocalCatalogueError,
    >,
    source: &str,
) -> Result<Vec<Entry>, String> {
    let provider = provider.as_ref().map_err(|e| e.to_string())?;
    let projects = provider.discover_projects("").map_err(|e| e.to_string())?;
    projects
        .iter()
        .map(|p| {
            provider
                .record(&p.id)
                .map(|record| view::from_catalogue(record, source))
                .map_err(|e| e.to_string())
        })
        .collect()
}

impl State {
    fn load_local(&mut self, path: PathBuf, context: &egui::Context) {
        // Dropping the old receiver prevents a late result replacing this selection.
        self.local.clear();
        self.feedback = None;
        self.path = Some(path.clone());
        let (tx, rx) = mpsc::channel();
        self.loading = Some(rx);
        let context = context.clone();
        std::thread::spawn(move || {
            let source = format!("Local catalogue file {}", path.display());
            let result = entries_of(&LocalCatalogueProvider::from_path(&path), &source);
            let _ = tx.send(result);
            context.request_repaint();
        });
    }

    fn start_github(&mut self, repository: String, context: &egui::Context) {
        if self.github_pending.is_some() {
            return;
        }
        self.github_error = None;
        self.github_pending = Some(github::start(repository, context));
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.loading {
            let result = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(
                    "Catalogue reader stopped. Choose the file again.".into(),
                )),
            };
            if let Some(result) = result {
                self.loading = None;
                match result {
                    Ok(entries) => self.local = entries,
                    Err(error) => self.feedback = Some(error),
                }
            }
        }
        if let Some(pending) = &self.github_pending {
            let result = match pending.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(Err("The lookup stopped unexpectedly.".into()))
                }
            };
            if let Some(result) = result {
                let name = pending.repository.clone();
                self.github_pending = None;
                self.apply_github(&name, result);
            }
        }
    }

    fn apply_github(&mut self, repository: &str, result: Result<Entry, String>) {
        match result {
            Ok(entry) => {
                self.github.retain(|e| e.key != entry.key);
                self.github.insert(0, entry);
                self.github_error = None;
            }
            Err(error) => self.github_error = Some(format!("{repository}: {error}")),
        }
    }

    fn all(&self) -> Vec<Entry> {
        let mut all = Vec::new();
        if let Some(Ok(builtin)) = &self.builtin {
            all.extend(builtin.iter().cloned());
        }
        all.extend(self.local.iter().cloned());
        all.extend(self.github.iter().cloned());
        all
    }
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut State) {
    state.poll();
    if state.builtin.is_none() {
        state.builtin = Some(builtin_entries());
    }
    ui.heading("Homebrew");
    ui.label(LIMIT);
    if let Some(Err(error)) = &state.builtin {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("Built-in catalogue unavailable: {error}"),
        );
    }

    ui.separator();
    ui.strong("Other sources");
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                state.loading.is_none(),
                egui::Button::new("Open a catalogue file…"),
            )
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Catalogue", &["json"])
                .pick_file()
        {
            state.load_local(path, ui.ctx());
        }
        if ui
            .add_enabled(
                state.loading.is_none() && state.path.is_some(),
                egui::Button::new("Reload file"),
            )
            .clicked()
        {
            state.load_local(state.path.clone().unwrap(), ui.ctx());
        }
        if state.loading.is_some() {
            ui.spinner();
        }
    });
    if let Some(error) = &state.feedback {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    ui.horizontal(|ui| {
        ui.label("GitHub repository (owner/name)");
        ui.text_edit_singleline(&mut state.github_input);
        let ready = state.github_pending.is_none() && !state.github_input.trim().is_empty();
        if ui
            .add_enabled(ready, egui::Button::new("Look up releases"))
            .clicked()
        {
            let repository = state.github_input.trim().to_string();
            state.start_github(repository, ui.ctx());
        }
        if state.github_pending.is_some() {
            ui.spinner();
            if ui.button("Cancel").clicked() {
                // Dropping the receiver discards the late result.
                state.github_pending = None;
            }
        }
    });
    ui.small(NETWORK_NOTE);
    if let Some(error) = &state.github_error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }

    ui.separator();
    let all = state.all();
    let mut platforms: Vec<String> = all.iter().flat_map(|e| e.platforms.clone()).collect();
    platforms.sort();
    platforms.dedup();
    ui.horizontal(|ui| {
        ui.label("Search");
        ui.text_edit_singleline(&mut state.query);
        egui::ComboBox::from_label("Platform claim")
            .selected_text(state.platform.as_deref().unwrap_or("Any"))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.platform, None, "Any");
                for platform in &platforms {
                    ui.selectable_value(&mut state.platform, Some(platform.clone()), platform);
                }
            });
    });
    let shown = view::filter(&all, &state.query, state.platform.as_deref());
    ui.label(format!("{} of {} titles", shown.len().min(200), all.len()));
    let mut lookup_request = None;
    egui::ScrollArea::vertical()
        .id_salt("homebrew_entries")
        .show(ui, |ui| {
            for entry in shown.iter().take(200) {
                ui.push_id(&entry.key, |ui| {
                    ui.collapsing(&entry.title, |ui| {
                        show_entry(ui, entry, &mut lookup_request)
                    });
                });
            }
        });
    if let Some(repository) = lookup_request {
        state.github_input = repository.clone();
        state.start_github(repository, ui.ctx());
    }
}

fn show_entry(ui: &mut egui::Ui, entry: &Entry, lookup_request: &mut Option<String>) {
    if let Some(summary) = &entry.summary {
        ui.label(summary);
    }
    ui.label(format!("Source: {}", entry.source));
    ui.label(format!("Page: {}", entry.source_page_url));
    ui.label(format!(
        "Platforms (claimed): {}",
        if entry.platforms.is_empty() {
            "none recorded".to_string()
        } else {
            entry.platforms.join(", ")
        }
    ));
    ui.strong("Versions and builds");
    if entry.releases.is_empty() {
        ui.label("No builds recorded.");
    }
    for release in &entry.releases {
        ui.label(format!("• {}", release.label));
        for build in &release.builds {
            ui.small(format!("    {build}"));
        }
    }
    ui.strong("Licence and rights");
    for line in &entry.licence {
        ui.label(line);
    }
    ui.strong("Compatibility notes");
    for line in &entry.compatibility {
        ui.label(line);
    }
    ui.strong("Provenance");
    for line in &entry.provenance {
        ui.small(line);
    }
    ui.small(view::LAUNCH_NOTE);
    if let Some(repository) = &entry.github_repository
        && ui
            .button(format!("Check GitHub releases for {repository}"))
            .on_hover_text(NETWORK_NOTE)
            .clicked()
    {
        *lookup_request = Some(repository.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(state: &mut State) {
        let context = egui::Context::default();
        let _ = context.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, state));
        });
    }

    #[test]
    fn builtin_catalogue_is_browsable_offline_with_no_file_or_network() {
        let mut state = State::default();
        render(&mut state);
        let entries = state.all();
        assert!(!entries.is_empty());
        assert!(entries.iter().all(|e| e.source == BUILTIN_SOURCE));
        assert!(state.github_pending.is_none() && state.loading.is_none());
        assert!(state.github.is_empty());
    }

    #[test]
    fn rendering_never_starts_a_network_lookup() {
        let mut state = State::default();
        state.github_input = "owner/demo".into();
        render(&mut state);
        render(&mut state);
        assert!(state.github_pending.is_none());
    }

    #[test]
    fn github_success_replaces_same_repository_and_error_is_kept_visible() {
        let mut state = State::default();
        let entry = |title: &str| {
            let mut e = builtin_entries().unwrap().remove(0);
            e.key = "github/1".into();
            e.title = title.into();
            e
        };
        state.apply_github("o/r", Ok(entry("first")));
        state.apply_github("o/r", Ok(entry("second")));
        assert_eq!(state.github.len(), 1);
        assert_eq!(state.github[0].title, "second");
        state.apply_github("o/r", Err("GitHub could not be reached: offline".into()));
        assert!(state.github_error.as_deref().unwrap().contains("offline"));
        assert_eq!(
            state.github.len(),
            1,
            "a failed lookup keeps earlier results"
        );
        render(&mut state);
    }

    #[test]
    fn replacement_load_clears_old_snapshot_and_ignores_old_receiver() {
        let (tx, rx) = mpsc::channel();
        let mut state = State {
            loading: Some(rx),
            local: builtin_entries().unwrap(),
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        state.load_local(dir.path().join("missing"), &egui::Context::default());
        assert!(tx.send(Err("old result".into())).is_err());
        assert!(state.local.is_empty());
    }

    #[test]
    fn local_file_is_read_only_and_leaves_its_directory_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalogue.json");
        let bytes = br#"{"schema_version":1,"provider":"local","records":[{"project":{"id":{"provider":"local","provider_id":"puzzle"},"title":"Moon Garden","summary":"A puzzle","source_page_url":"https://example.org/project"},"listings":[],"observations":[{"source_url":"https://example.org/project","observed_on":"2026-10-11","note":"Unverified fixture"}]}]}"#;
        std::fs::write(&path, bytes).unwrap();
        let entries = entries_of(&LocalCatalogueProvider::from_path(&path), "Local").unwrap();
        assert_eq!(entries[0].title, "Moon Garden");
        assert_eq!(
            entries[0].compatibility,
            ["No platform claim recorded; compatibility is unknown."]
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn invalid_local_catalogue_reports_an_error_instead_of_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, b"{not json").unwrap();
        assert!(entries_of(&LocalCatalogueProvider::from_path(&path), "Local").is_err());
    }
}
