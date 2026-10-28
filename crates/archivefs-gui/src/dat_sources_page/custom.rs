//! Explicit user-source lifecycle. Workers own I/O; UI approvals own one
//! source/candidate/active-state observation and are discarded on selection.
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use archivefs_core::dat::custom_sources::{
    CustomDatCandidate, CustomDatLifecycle, CustomDatSource, CustomDatSourceKind,
    CustomDatSourceRegistry,
};
use archivefs_core::identity_source::managed_snapshot::{ManagedSourceReference, UpdateCheck};
use eframe::egui;

type Outcome<T> = Result<T, String>;

#[derive(Default)]
pub(super) struct Controller {
    registry: CustomDatSourceRegistry,
    loaded: bool,
    selected: Option<CustomDatSource>,
    name: String,
    address: String,
    kind: usize,
    candidate: Option<CustomDatCandidate>,
    active: Option<String>,
    active_path: Option<PathBuf>,
    history: Vec<String>,
    rollback: Option<String>,
    approved: bool,
    message: String,
    job: Option<Receiver<Outcome<Response>>>,
}

struct Response {
    registry: CustomDatSourceRegistry,
    source: Option<CustomDatSource>,
    candidate: Option<CustomDatCandidate>,
    active: Option<String>,
    active_path: Option<PathBuf>,
    history: Vec<String>,
    message: String,
}

enum Action {
    Load,
    Save {
        old: Option<CustomDatSource>,
        new: CustomDatSource,
    },
    Remove(CustomDatSource),
    Inspect(CustomDatSource),
    Check(CustomDatSource),
    Stage(CustomDatSource),
    Activate(CustomDatSource, Box<CustomDatCandidate>),
    Rollback(CustomDatSource, String, Option<String>),
}

fn run(
    path: &Path,
    root: &Path,
    expected: &CustomDatSourceRegistry,
    action: Action,
) -> Outcome<Response> {
    let mut registry = CustomDatSourceRegistry::load(path).map_err(|e| e.to_string())?;
    if !matches!(action, Action::Load) && &registry != expected {
        return Err("Source registry changed. Reload and review again.".into());
    }
    let mut source = None;
    let mut candidate = None;
    let message = match action {
        Action::Load => "Source registry reloaded; approvals cleared.".into(),
        Action::Save { old, new } => {
            if let Some(old) = old {
                registry.remove(&old.id);
            }
            registry.add(new.clone()).map_err(|e| e.to_string())?;
            registry
                .save_if_current(path, expected)
                .map_err(|e| e.to_string())?;
            source = Some(new);
            "Registration saved. No DAT was activated.".into()
        }
        Action::Remove(old) => {
            registry.remove(&old.id);
            registry
                .save_if_current(path, expected)
                .map_err(|e| e.to_string())?;
            "Registration removed. Original files, snapshots and local catalogue registrations remain.".into()
        }
        action => {
            let selected = match &action {
                Action::Inspect(s)
                | Action::Check(s)
                | Action::Stage(s)
                | Action::Activate(s, _)
                | Action::Rollback(s, _, _) => s.clone(),
                _ => unreachable!(),
            };
            if registry.get(&selected.id) != Some(&selected) {
                return Err("Source changed; reload before continuing.".into());
            }
            let lifecycle = CustomDatLifecycle::new(&selected, root).map_err(|e| e.to_string())?;
            let message = match action {
                Action::Inspect(_) => "Current snapshot and history reloaded.".into(),
                Action::Check(_) => {
                    match lifecycle.check_update(false).map_err(|e| e.to_string())? {
                        UpdateCheck::Unchanged { .. } => {
                            "Server reports no update. No bytes activated.".into()
                        }
                        UpdateCheck::Available { .. } => {
                            "Server reports an available update. Stage it to validate.".into()
                        }
                        UpdateCheck::Offline { .. } => "Offline; no update checked.".into(),
                    }
                }
                Action::Stage(_) => {
                    let staged = lifecycle.download().map_err(|e| e.to_string())?;
                    lifecycle.preview(&staged).map_err(|e| e.to_string())?;
                    candidate = Some(staged);
                    "Staged and validated. Review and approve activation separately.".into()
                }
                Action::Activate(_, staged) => {
                    lifecycle.activate(&staged).map_err(|e| e.to_string())?;
                    "Snapshot activated. Game verification is unchanged until explicitly rechecked."
                        .into()
                }
                Action::Rollback(_, target, active) => {
                    lifecycle
                        .rollback_if_current(&target, active.as_deref())
                        .map_err(|e| e.to_string())?;
                    "Historical snapshot activated. Recheck games before relying on changed evidence.".into()
                }
                _ => unreachable!(),
            };
            source = Some(selected);
            message
        }
    };
    let (active, active_path, history) = if let Some(source) = &source {
        let lifecycle = CustomDatLifecycle::new(source, root).map_err(|e| e.to_string())?;
        let active = lifecycle
            .active()
            .map_err(|e| e.to_string())?
            .map(|s| s.sha256);
        let active_path = lifecycle.active_dat_path().map_err(|e| e.to_string())?;
        let history = lifecycle
            .history()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|s| s.sha256)
            .collect();
        (active, active_path, history)
    } else {
        (None, None, Vec::new())
    };
    Ok(Response {
        registry,
        source,
        candidate,
        active,
        active_path,
        history,
        message,
    })
}

impl Controller {
    fn start(&mut self, path: PathBuf, root: PathBuf, action: Action) {
        if self.job.is_some() {
            return;
        }
        self.approved = false;
        self.candidate = None;
        self.rollback = None;
        self.active_path = None;
        let registry = self.registry.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        self.job = Some(rx);
        std::thread::spawn(move || {
            // Core remote errors are redacted. Do not debug-print metadata,
            // candidate records or source URLs (queries can contain secrets).
            let _ = tx.send(run(&path, &root, &registry, action));
        });
    }

    fn poll(&mut self) {
        let Some(rx) = &self.job else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(_) => Err("Source worker ended without a result. Reload before retrying.".into()),
        };
        self.job = None;
        match result {
            Ok(result) => {
                self.loaded = true;
                self.registry = result.registry;
                self.selected = result.source;
                self.candidate = result.candidate;
                self.active = result.active;
                self.active_path = result.active_path;
                self.history = result.history;
                self.message = result.message;
            }
            Err(error) => {
                self.message = error;
                self.active = None;
                self.history.clear();
            }
        }
    }

    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        path: PathBuf,
        root: PathBuf,
    ) -> Option<PathBuf> {
        self.poll();
        if !self.loaded && self.job.is_none() && self.message.is_empty() {
            self.start(path.clone(), root.clone(), Action::Load);
        }
        let mut action = None;
        let mut import = None;
        ui.collapsing("Custom user DAT sources", |ui| {
            ui.label("User-provided catalogues, never official. Registration, staging, activation and game verification are separate steps. Originals and historical snapshots are preserved.");
            if !self.message.is_empty() { ui.label(&self.message); }
            if self.job.is_some() { ui.spinner(); ui.label("Working on the selected source…"); }
            ui.add_enabled_ui(self.job.is_none(), |ui| {
                if ui.button("Reload sources and clear review").clicked() { action = Some(Action::Load); }
                for source in self.registry.sources() {
                    if ui.selectable_label(self.selected.as_ref() == Some(source), format!("{} — {} — UserProvided", source.display_name, source.display_address())).clicked() {
                        self.name = source.display_name.clone();
                        // Never redisplay raw secret-bearing remote URLs in the editor.
                        self.address = match &source.reference { ManagedSourceReference::LocalPath(p) => p.display().to_string(), _ => String::new() };
                        self.kind = match source.kind { CustomDatSourceKind::Local => 0, CustomDatSourceKind::Https => 1, CustomDatSourceKind::Github => 2 };
                        action = Some(Action::Inspect(source.clone()));
                    }
                }
                if ui.button("New registration").clicked() {
                    self.selected = None; self.name.clear(); self.address.clear(); self.candidate = None; self.rollback = None; self.active = None; self.active_path = None; self.history.clear(); self.approved = false;
                }
                ui.label("Display name");
                let mut edited = ui.text_edit_singleline(&mut self.name).changed();
                ui.horizontal(|ui| {
                    for (index, label) in ["Local", "HTTPS", "GitHub"].iter().enumerate() {
                        edited |= ui.radio_value(&mut self.kind, index, *label).changed();
                    }
                });
                ui.label("Path or address (re-enter a remote address to edit; query values stay hidden)");
                edited |= ui.add(egui::TextEdit::singleline(&mut self.address).password(self.kind != 0)).changed();
                if edited { self.candidate = None; self.rollback = None; self.approved = false; }
                if ui.button("Save registration only").clicked() {
                    let source = match self.kind {
                        0 => CustomDatSource::local(Path::new(&self.address), self.name.clone()),
                        1 => CustomDatSource::https(&self.address, self.name.clone()),
                        _ => CustomDatSource::github(&self.address, self.name.clone()),
                    };
                    match source {
                        Ok(new) => { action = Some(Action::Save { old: self.selected.clone(), new }); self.address.clear(); }
                        Err(error) => self.message = error.to_string(),
                    }
                }
                if let Some(source) = self.selected.clone() {
                    ui.label(format!("Actions below use saved source: {} ({})", source.display_name, source.display_address()));
                    if ui.button("Remove registration only").clicked() { action = Some(Action::Remove(source.clone())); }
                    if source.kind != CustomDatSourceKind::Local && ui.button("Check for update (network)").clicked() { action = Some(Action::Check(source.clone())); }
                    if ui.button("Stage and validate saved source").clicked() { action = Some(Action::Stage(source.clone())); }
                    if let Some(candidate) = &self.candidate {
                        ui.label(format!("Candidate {} · {} entries · {:?} · UserProvided", candidate.candidate.snapshot.sha256, candidate.validation.entry_count, candidate.validation.ecosystem));
                        for warning in &candidate.validation.warnings { ui.label(warning); }
                        ui.checkbox(&mut self.approved, "I approve activating this exact staged catalogue");
                        if ui.add_enabled(self.approved, egui::Button::new("Activate reviewed candidate")).clicked() {
                            action = Some(Action::Activate(source.clone(), Box::new(candidate.clone())));
                        }
                    }
                    if let Some(active) = &self.active { ui.label(format!("Observed active snapshot: {active}")); }
                    ui.label("History (retained versions)");
                    for hash in &self.history {
                        if ui.selectable_label(self.rollback.as_ref() == Some(hash), hash).clicked() {
                            self.rollback = Some(hash.clone()); self.approved = false; self.candidate = None;
                        }
                    }
                    if let Some(target) = &self.rollback {
                        ui.checkbox(&mut self.approved, "I approve replacing the observed active snapshot with this historical version");
                        if ui.add_enabled(self.approved, egui::Button::new("Activate reviewed historical snapshot")).clicked() {
                            action = Some(Action::Rollback(source, target.clone(), self.active.clone()));
                        }
                    }
                    if let Some(active_path) = &self.active_path {
                        ui.label("Local catalogue registrations pin a version. Updating this feed does not silently change an existing game's evidence. Add the new version and explicitly save/check it in the local catalogue controls.");
                        if ui.button("Add observed active version to local DAT list").clicked() { import = Some(active_path.clone()); }
                    }
                }
            });
        });
        if let Some(action) = action {
            self.start(path, root, action);
        }
        if self.job.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        import
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const DAT: &str = r#"<?xml version="1.0"?><datafile><header><name>Custom</name></header><game name="Example"><rom name="example.bin" size="1" sha1="86f7e437fa060d3f29a2f2c2e1f7b8f7d3b2e0d4"/></game></datafile>"#;

    #[test]
    fn local_lifecycle_uses_persisted_registry_and_separate_activation() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("original.dat");
        std::fs::write(&input, DAT).unwrap();
        let registry = dir.path().join("sources.json");
        let root = dir.path().join("snapshots");
        let source = CustomDatSource::local(&input, "My DAT").unwrap();
        let initial = CustomDatSourceRegistry::default();
        let saved = run(
            &registry,
            &root,
            &initial,
            Action::Save {
                old: None,
                new: source.clone(),
            },
        )
        .unwrap();
        let staged = run(
            &registry,
            &root,
            &saved.registry,
            Action::Stage(source.clone()),
        )
        .unwrap();
        assert!(staged.active.is_none());
        let candidate = staged.candidate.unwrap();
        let activated = run(
            &registry,
            &root,
            &saved.registry,
            Action::Activate(source.clone(), Box::new(candidate)),
        )
        .unwrap();
        let active_path = activated.active_path.unwrap();
        assert_eq!(std::fs::read_to_string(&active_path).unwrap(), DAT);
        // Existing catalogue parser accepts the store's extensionless objects.
        let entry = archivefs_core::dat::sources::DatSourceEntry::new(
            "custom".into(),
            "User DAT".into(),
            active_path.clone(),
            archivefs_core::dat::sources::DatSourceKind::File,
        );
        let report = archivefs_core::dat::sources::validate_dat_source(
            &entry,
            archivefs_core::dat::limits::DatLimits::default(),
        );
        assert_eq!(report.0.entry_count, 1);
        assert!(matches!(
            report.0.state,
            archivefs_core::dat::sources::DatHealthState::Valid
                | archivefs_core::dat::sources::DatHealthState::ValidWithWarnings
        ));
        assert!(entry.is_user_local());
        run(&registry, &root, &saved.registry, Action::Remove(source)).unwrap();
        assert!(
            CustomDatSourceRegistry::load(&registry)
                .unwrap()
                .sources()
                .is_empty()
        );
        assert_eq!(std::fs::read_to_string(input).unwrap(), DAT);
        assert!(active_path.is_file());
    }

    #[test]
    fn source_change_refuses_old_actions_and_rendering_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sources.json");
        let root = dir.path().join("snapshots");
        let source =
            CustomDatSource::https("https://example.com/feed.dat?token=secret", "Feed").unwrap();
        let old = CustomDatSourceRegistry::default();
        let saved = run(
            &path,
            &root,
            &old,
            Action::Save {
                old: None,
                new: source.clone(),
            },
        )
        .unwrap();
        assert!(run(&path, &root, &old, Action::Stage(source.clone())).is_err());
        let bytes = std::fs::read(&path).unwrap();
        let mut controller = Controller {
            loaded: true,
            registry: saved.registry,
            selected: Some(source),
            ..Default::default()
        };
        let context = egui::Context::default();
        let _ = context.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                controller.show(ui, path.clone(), root.clone());
            });
        });
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert!(!root.exists());
        assert!(!controller.approved);
    }
}
