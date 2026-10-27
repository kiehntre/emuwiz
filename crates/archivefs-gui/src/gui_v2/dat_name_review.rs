//! Blue's read-only canonical-name campaign. All naming stays in Core.
use crate::dat_catalogue_picker::{
    DatCataloguePickerState, DatCatalogueWorkflow, provenance_label,
};
use archivefs_core::dat::catalogue_selection::{CatalogueRef, resolve_catalogue};
use archivefs_core::dat::rename_plan::review::{CanonicalReview, ReviewDisposition, build_review};
use archivefs_core::dat::sources::{
    audit_cache::AuditCacheConfig, audit_run::run_dat_audit_with_cache,
};
use archivefs_core::identity_source::hashing::FileFingerprint;
use archivefs_core::safe_read::TrustedRoots;
use eframe::egui;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{
    Arc,
    mpsc::{self, Receiver},
};

#[derive(Default)]
pub(super) struct State {
    picker: DatCataloguePickerState,
    selected: Option<CatalogueRef>,
    root: String,
    result: Option<Result<CanonicalReview, String>>,
    worker: Option<Receiver<Result<CanonicalReview, String>>>,
    cancel: Option<Arc<AtomicBool>>,
    filter: usize,
    query: String,
    page: usize,
}
impl Drop for State {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}
impl State {
    pub(super) fn show(&mut self, ui: &mut egui::Ui) {
        ui.heading("DAT canonical name review");
        ui.label("Read-only preview. Originals, catalogue records, configurations and Undo journals are preserved. No Apply action is available here.");
        self.picker.ensure_loaded();
        self.picker.poll();
        if let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok(result) => {
                    self.result = Some(result);
                    self.worker = None;
                    self.cancel = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.result = Some(Err("Preview worker stopped; no files were changed".into()));
                    self.worker = None;
                    self.cancel = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let busy = self.worker.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            let prior = (self.selected.clone(), self.root.clone());
            let _ = self
                .picker
                .show(ui, DatCatalogueWorkflow::Repair, &mut self.selected);
            ui.horizontal(|ui| {
                ui.label("Library folder or file:");
                ui.text_edit_singleline(&mut self.root);
                if ui.button("Browse…").clicked()
                    && let Some(path) = rfd::FileDialog::new().pick_folder()
                {
                    self.root = path.display().to_string();
                }
            });
            if prior != (self.selected.clone(), self.root.clone()) {
                self.result = None;
                self.page = 0;
            }
            if ui.button("Refresh installed catalogues").clicked() {
                self.picker.refresh();
                self.result = None;
            }
            if ui
                .add_enabled(
                    self.selected.is_some() && !self.root.trim().is_empty(),
                    egui::Button::new("Build fresh read-only preview"),
                )
                .clicked()
            {
                self.start();
            }
        });
        if busy {
            ui.spinner();
            ui.label("Reading local catalogue and hashing files; no verification results or hash caches are saved.");
            if ui.button("Cancel preview").clicked()
                && let Some(cancel) = &self.cancel
            {
                cancel.store(true, Ordering::Relaxed);
            }
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        let Some(result) = &self.result else {
            return;
        };
        let report = match result {
            Ok(report) => report,
            Err(error) => {
                ui.colored_label(egui::Color32::YELLOW, error);
                return;
            }
        };
        ui.separator();
        ui.label(format!(
            "Catalogue: {} · version {}",
            report.source,
            report.version.as_deref().unwrap_or("not supplied")
        ));
        ui.label(format!("Source: {}", report.dat_path));
        ui.label(format!(
            "Audited catalogue SHA-256: {}",
            report.catalogue_sha256.as_deref().unwrap_or("unavailable")
        ));
        ui.label("DAT identity means matching local catalogue records. A source label or header does not authenticate its publisher. This review covers the selected catalogue and audited game candidates. Supporting files may be skipped; other unselected catalogues are not reconciled.");
        if let Some(problem) = &report.catalogue_problem {
            ui.colored_label(egui::Color32::YELLOW, problem);
        }
        ui.label("Snapshot checked at preview completion. Files and catalogues can change afterwards; build a fresh preview before reviewing again. This snapshot cannot authorize a rename.");
        let count = |d| report.rows.iter().filter(|r| r.disposition == d).count();
        ui.strong(format!(
            "{} items · {} eligible · {} held · {} refused · {} already canonical",
            report.rows.len(),
            count(ReviewDisposition::Eligible),
            count(ReviewDisposition::Held),
            count(ReviewDisposition::Refused),
            count(ReviewDisposition::AlreadyCanonical)
        ));
        if report.truncated {
            ui.label("Partial audit: every proposal is held until coverage is complete.");
        }
        ui.horizontal_wrapped(|ui| {
            for (index, label) in ["All", "Eligible", "Held", "Refused", "Already canonical"]
                .iter()
                .enumerate()
            {
                if ui
                    .selectable_value(&mut self.filter, index, *label)
                    .changed()
                {
                    self.page = 0;
                }
            }
            if ui.text_edit_singleline(&mut self.query).changed() {
                self.page = 0;
            }
        });
        let visible: Vec<_> = report
            .rows
            .iter()
            .filter(|r| {
                (self.filter == 0
                    || match self.filter {
                        1 => r.disposition == ReviewDisposition::Eligible,
                        2 => r.disposition == ReviewDisposition::Held,
                        3 => r.disposition == ReviewDisposition::Refused,
                        _ => r.disposition == ReviewDisposition::AlreadyCanonical,
                    })
                    && (self.query.is_empty()
                        || r.path.to_lowercase().contains(&self.query.to_lowercase())
                        || r.proposal
                            .as_ref()
                            .and_then(|p| p.proposed_basename.as_ref())
                            .is_some_and(|n| n.to_lowercase().contains(&self.query.to_lowercase())))
            })
            .collect();
        let pages = visible.len().div_ceil(50).max(1);
        self.page = self.page.min(pages - 1);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.page > 0, egui::Button::new("Previous"))
                .clicked()
            {
                self.page -= 1;
            }
            ui.label(format!(
                "Page {} of {} · {} matching",
                self.page + 1,
                pages,
                visible.len()
            ));
            if ui
                .add_enabled(self.page + 1 < pages, egui::Button::new("Next"))
                .clicked()
            {
                self.page += 1;
            }
        });
        for row in visible.into_iter().skip(self.page * 50).take(50) {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.strong(format!(
                    "{} · {}",
                    row.disposition.label(),
                    row.verdict.label()
                ));
                ui.label(format!("Current: {}", row.filename));
                ui.label(format!(
                    "Proposed: {}",
                    row.proposal
                        .as_ref()
                        .and_then(|p| p.proposed_basename.as_deref().or_else(|| (p.state
                            == archivefs_core::dat::rename_plan::ProposalState::AlreadyCanonical)
                            .then_some(p.current_basename.as_str())))
                        .unwrap_or("no supported proposal")
                ));
                ui.small(&row.path);
                if let Some(p) = &row.proposal {
                    ui.label(format!(
                        "DAT record: game {} · ROM {} · source {}",
                        p.game_name.as_deref().unwrap_or("ambiguous"),
                        p.rom_name.as_deref().unwrap_or("set-level"),
                        p.source_display_name
                    ));
                    ui.label(format!(
                        "Planner state: {} · confidence: {}",
                        p.state.label(),
                        if p.match_confident {
                            "cryptographic hash match in selected DAT"
                        } else {
                            "not verified"
                        }
                    ));
                }
                ui.collapsing("Identity and explanations", |ui| {
                    ui.label(format!("Audit evidence: {:?}", row.verdict));
                    for identity in &row.identity {
                        ui.monospace(identity);
                    }
                    for reason in &row.reasons {
                        ui.label(reason);
                    }
                });
            });
        }
    }
    fn start(&mut self) {
        let Some(reference) = self.selected.clone() else {
            return;
        };
        let Some(inventory) = self.picker.snapshot() else {
            self.result = Some(Err(
                "Installed catalogue inventory is unavailable; refresh it".into(),
            ));
            return;
        };
        let root = PathBuf::from(self.root.trim());
        if !root.is_absolute() {
            self.result = Some(Err("Choose an absolute local library path".into()));
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.worker = Some(receiver);
        self.result = None;
        self.page = 0;
        std::thread::spawn(move || {
            let result = inspect(&reference, &inventory, root, &cancel);
            let _ = sender.send(result);
        });
    }
}

fn inspect(
    reference: &CatalogueRef,
    inventory: &crate::dat_catalogue_picker::CatalogueInventorySnapshot,
    root: PathBuf,
    cancel: &AtomicBool,
) -> Result<CanonicalReview, String> {
    let resolved = resolve_catalogue(reference, inventory.inputs()).map_err(|e| e.to_string())?;
    let provenance = provenance_label(&resolved.provenance);
    let before = FileFingerprint::observe(&resolved.backing_path)
        .ok_or("Selected catalogue is unavailable")?;
    let request = resolved.to_dat_audit_request(root, Default::default(), None);
    let outcome = run_dat_audit_with_cache(
        &request,
        &TrustedRoots::none(),
        cancel,
        &|_| {},
        AuditCacheConfig::Disabled,
    )
    .map_err(|e| e.to_string())?;
    if !before.still_current() {
        return Err("Selected catalogue changed during preview; refresh and try again".into());
    }
    if let CatalogueRef::ManagedCurrent {
        snapshot_sha256, ..
    } = reference
        && outcome.catalogue_sha256.as_ref() != Some(snapshot_sha256)
    {
        return Err("Managed catalogue snapshot changed; no naming authority granted".into());
    }
    let mut review = build_review(&outcome, cancel)?;
    review.source = format!("{} · {}", review.source, provenance);
    Ok(review)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat_catalogue_picker::CatalogueInventorySnapshot;
    use archivefs_core::dat::sources::{DatSourceEntry, DatSourceKind, DatSourceRegistry};
    #[test]
    fn selected_installed_catalogue_to_blue_batch_is_read_only() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("roms");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("old.nes"), b"abc").unwrap();
        let dat = temp.path().join("test.dat");
        std::fs::write(&dat, r#"<datafile><header><name>Test</name><version>1</version></header><game name="Example"><rom name="Canonical.nes" size="3" sha1="a9993e364706816aba3e25717850c26c9cd0d89d"/></game></datafile>"#).unwrap();
        let mut registry = DatSourceRegistry::new();
        let mut source =
            DatSourceEntry::new("selected".into(), "My DAT".into(), dat, DatSourceKind::File);
        source.platform = Some("nes".into());
        registry.add(source).unwrap();
        let inventory = CatalogueInventorySnapshot {
            local_registry: registry,
            managed_sources: Default::default(),
            managed_root: temp.path().join("managed"),
        };
        let result = inspect(
            &CatalogueRef::local("selected"),
            &inventory,
            root.clone(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].disposition,
            ReviewDisposition::Eligible,
            "{:?}",
            result.rows[0]
        );
        assert_eq!(
            result.rows[0]
                .proposal
                .as_ref()
                .unwrap()
                .proposed_basename
                .as_deref(),
            Some("Canonical.nes")
        );
        assert_eq!(std::fs::read(root.join("old.nes")).unwrap(), b"abc");
        assert!(!root.join("Canonical.nes").exists());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 2);
        assert!(
            inspect(
                &CatalogueRef::local("missing"),
                &inventory,
                root,
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }
}
