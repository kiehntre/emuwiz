//! Advisory source relocation preview. The Core adapter owns all path decisions.
use archivefs_core::{
    Database, SourceFolderRecord,
    source_folder_migration::{
        SourceFolderMigrationInput, SourceFolderMigrationReport, plan_source_folder_migration,
    },
};
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

const LIMIT: &str = "Impact preview only. No folders, database paths, archive keys or history are changed. This is an observation, not approval to relocate anything. Reload and preview again after changes.";
enum Loaded {
    Sources(Vec<SourceFolderRecord>),
    Report(Box<SourceFolderMigrationReport>),
}
#[derive(Default)]
pub(super) struct State {
    database: Option<PathBuf>,
    sources: Vec<SourceFolderRecord>,
    selected: Option<i64>,
    destination: String,
    loading: Option<Receiver<Result<Loaded, String>>>,
    report: Option<SourceFolderMigrationReport>,
    error: Option<String>,
}

fn preview(
    database: &Path,
    source: i64,
    new_root: &Path,
) -> Result<SourceFolderMigrationReport, String> {
    if !new_root.is_absolute() {
        return Err("Choose an absolute destination folder.".into());
    }
    let database = Database::open_read_only(database).map_err(|e| e.to_string())?;
    let row = database
        .list_source_folders()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|row| row.id == source)
        .ok_or("Source no longer exists. Reload sources.")?;
    let mut input = SourceFolderMigrationInput::from_database(&database, row.path, new_root)
        .map_err(|e| e.to_string())?;
    input.target_source_id = Some(source);
    Ok(plan_source_folder_migration(&input))
}
impl State {
    pub(super) fn live() -> Self {
        Self {
            database: archivefs_core::default_database_path().ok(),
            ..Default::default()
        }
    }
    fn invalidate(&mut self) {
        self.report = None;
        self.error = None;
        self.loading = None;
    }
    fn start(&mut self, context: &egui::Context, source: Option<i64>) {
        self.invalidate();
        let Some(database) = self.database.clone() else {
            self.error = Some("Library database location is unavailable.".into());
            return;
        };
        let destination = PathBuf::from(&self.destination);
        if source.is_none() {
            self.sources.clear();
            self.selected = None;
        }
        let (tx, rx) = mpsc::channel();
        self.loading = Some(rx);
        let context = context.clone();
        std::thread::spawn(move || {
            let result = match source {
                Some(source) => preview(&database, source, &destination)
                    .map(Box::new)
                    .map(Loaded::Report),
                None => Database::open_read_only(database)
                    .and_then(|db| db.list_source_folders())
                    .map(Loaded::Sources)
                    .map_err(|e| e.to_string()),
            };
            let _ = tx.send(result);
            context.request_repaint();
        });
    }
    fn poll(&mut self) {
        let Some(rx) = &self.loading else { return };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("Preview worker stopped. Try again.".into()),
        };
        self.loading = None;
        match result {
            Ok(Loaded::Sources(rows)) => self.sources = rows,
            Ok(Loaded::Report(report)) => self.report = Some(*report),
            Err(error) => self.error = Some(error),
        }
    }
}
pub(super) fn show(ui: &mut egui::Ui, state: &mut State) {
    state.poll();
    ui.heading("Source-location impact preview");
    ui.label(LIMIT);
    if ui
        .add_enabled(
            state.loading.is_none(),
            egui::Button::new("Reload library sources"),
        )
        .clicked()
    {
        state.start(ui.ctx(), None);
    }
    if state.loading.is_some() {
        ui.spinner();
    }
    let previous = state.selected;
    egui::ComboBox::from_label("Existing library source")
        .selected_text(
            state
                .sources
                .iter()
                .find(|s| Some(s.id) == state.selected)
                .map(|s| s.path.display().to_string())
                .unwrap_or_else(|| "Select a source".into()),
        )
        .show_ui(ui, |ui| {
            for source in &state.sources {
                ui.selectable_value(
                    &mut state.selected,
                    Some(source.id),
                    format!("{} (source {})", source.path.display(), source.id),
                );
            }
        });
    ui.label("Proposed new folder (absolute path)");
    let edited = ui.text_edit_singleline(&mut state.destination).changed();
    if previous != state.selected || edited {
        state.invalidate();
    }
    if ui
        .add_enabled(
            state.loading.is_none()
                && state.selected.is_some()
                && !state.destination.trim().is_empty(),
            egui::Button::new("Preview impact"),
        )
        .clicked()
    {
        state.start(ui.ctx(), state.selected);
    }
    if state.sources.is_empty() && state.loading.is_none() {
        ui.label("No sources loaded. Reload to read the existing library; this does not create or upgrade a database.");
    }
    if let Some(error) = &state.error {
        ui.label(error);
    }
    if let Some(report) = &state.report {
        let t = &report.totals;
        ui.label(format!("Database snapshot: {} roots could rebase · {} already current · {} need review · {} unsupported · {} conflicts", t.source_roots_rebase, t.source_roots_already_current, t.source_roots_manual_review, t.source_roots_unsupported, t.conflicts));
        ui.label(format!("{} archive observations would need recomputing. Stable relative keys and history remain unchanged.", t.archive_observations_to_recompute));
        for proposal in report.source_roots.migration.proposals.iter().filter(|p| {
            p.reference_id == format!("source-folder:{}", state.selected.unwrap_or_default())
        }) {
            ui.label(format!(
                "{} → {} · {:?}: {}",
                proposal.old_path.display(),
                proposal
                    .candidate_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "no candidate".into()),
                proposal.classification,
                proposal.reason
            ));
        }
        ui.label("Other persisted source rows are not selected for relocation. No automatic relocation is available here.");
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_reads_real_database_without_modifying_it_and_binds_one_source() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("library.db");
        let old = dir.path().join("old");
        let new = dir.path().join("new");
        std::fs::create_dir(&new).unwrap();
        let mut db = Database::open_or_create(&database).unwrap();
        db.register_source_folders(&[old, dir.path().join("other")])
            .unwrap();
        let source = db.list_source_folders().unwrap()[0].id;
        drop(db);
        let before = std::fs::read(&database).unwrap();
        let report = preview(&database, source, &new).unwrap();
        assert_eq!(report.totals.migratable_source_roots, 1);
        assert_eq!(report.totals.source_folder_records, 2);
        assert_eq!(std::fs::read(&database).unwrap(), before);
        assert!(preview(&database, i64::MAX, &new).is_err());
        assert!(preview(&database, source, Path::new("relative")).is_err());
        let missing = dir.path().join("missing.db");
        assert!(preview(&missing, source, &new).is_err());
        assert!(!missing.exists());
    }
    #[test]
    fn changing_selection_drops_pending_and_displayed_results() {
        let (tx, rx) = mpsc::channel();
        let mut state = State {
            loading: Some(rx),
            error: Some("old".into()),
            ..Default::default()
        };
        state.invalidate();
        assert!(tx.send(Err("late".into())).is_err());
        assert!(state.error.is_none());
        assert!(state.report.is_none());
        let context = egui::Context::default();
        let _ = context.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, &mut state));
        });
        assert!(state.database.is_none());
    }
}
