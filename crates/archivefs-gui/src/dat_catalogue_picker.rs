//! Shared, explicit DAT catalogue selection for task-oriented GUI workflows.
//!
//! The picker owns only the transient inventory/loading state.  A workflow
//! owns the selected [`CatalogueRef`], so leaving and returning to a page can
//! preserve that choice without creating a global active-DAT preference.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use archivefs_core::dat::catalogue_selection::{
    CatalogueAvailability, CatalogueInventoryInputs, CatalogueProvenance, CatalogueRef,
    EvidenceValue, InstalledCatalogueSummary, list_installed_catalogues,
};
use archivefs_core::dat::limits::DatLimits;
use archivefs_core::dat::managed_sources::{
    ManagedDatSources, default_managed_dat_sources_config_path, load_managed_dat_sources_from,
};
use archivefs_core::dat::sources::{
    DatSourceRegistry, default_dat_sources_config_path, load_dat_sources_config_from,
};
use archivefs_core::dat::updates::managed_dat_root;
use eframe::egui;

use crate::ui::{components as widgets, theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DatCatalogueWorkflow {
    Verify,
    Repair,
    PlayingLibrary,
}

impl DatCatalogueWorkflow {
    fn label(self) -> &'static str {
        match self {
            Self::Verify => "Verify Games",
            Self::Repair => "Repair Review",
            Self::PlayingLibrary => "Build Playing Library",
        }
    }

    fn capability(self, summary: &InstalledCatalogueSummary) -> bool {
        match self {
            Self::Verify => summary.capabilities.verify,
            Self::Repair => summary.capabilities.repair,
            Self::PlayingLibrary => summary.capabilities.single_catalogue_1g1r,
        }
    }
}

/// Owned inputs used to resolve a selected reference after the user has
/// explicitly chosen it.  Keeping these beside the inventory means pages do
/// not reconstruct a second DAT-selection model.
#[derive(Debug, Clone)]
pub(crate) struct CatalogueInventorySnapshot {
    pub(crate) local_registry: DatSourceRegistry,
    pub(crate) managed_sources: ManagedDatSources,
    pub(crate) managed_root: PathBuf,
}

impl CatalogueInventorySnapshot {
    pub(crate) fn inputs(&self) -> CatalogueInventoryInputs<'_> {
        CatalogueInventoryInputs {
            local_registry: &self.local_registry,
            managed_sources: &self.managed_sources,
            managed_root: &self.managed_root,
            limits: DatLimits::default(),
        }
    }
}

struct LoadedInventory {
    summaries: Vec<InstalledCatalogueSummary>,
    snapshot: CatalogueInventorySnapshot,
}

enum InventoryMessage {
    Loaded(u64, Result<LoadedInventory, String>),
}

/// Transient loading and rendering state for the shared picker.
#[derive(Default)]
pub(crate) struct DatCataloguePickerState {
    summaries: Vec<InstalledCatalogueSummary>,
    snapshot: Option<CatalogueInventorySnapshot>,
    receiver: Option<Receiver<InventoryMessage>>,
    generation: u64,
    loaded: bool,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
    query: String,
}

impl DatCataloguePickerState {
    pub(crate) fn ensure_loaded(&mut self) {
        if !self.loaded && !self.loading && self.error.is_none() {
            self.start_load();
        }
    }

    pub(crate) fn refresh(&mut self) {
        self.start_load();
    }

    fn start_load(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.loading = true;
        self.loaded = false;
        self.error = None;
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        std::thread::spawn(move || {
            let result = load_inventory().map_err(|error| error.to_string());
            let _ = sender.send(InventoryMessage::Loaded(generation, result));
        });
    }

    /// Drain the worker and return whether the visible state changed.
    pub(crate) fn poll(&mut self) -> bool {
        let Some(receiver) = self.receiver.as_ref() else {
            return false;
        };
        match receiver.try_recv() {
            Ok(InventoryMessage::Loaded(generation, result)) => {
                self.receiver = None;
                self.loading = false;
                if generation != self.generation {
                    return false;
                }
                match result {
                    Ok(loaded) => {
                        self.summaries = loaded.summaries;
                        self.snapshot = Some(loaded.snapshot);
                        self.error = None;
                        self.loaded = true;
                    }
                    Err(error) => self.error = Some(error),
                }
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.receiver = None;
                self.loading = false;
                self.error = Some("the catalogue worker stopped unexpectedly".to_string());
                true
            }
        }
    }

    pub(crate) fn snapshot(&self) -> Option<CatalogueInventorySnapshot> {
        self.snapshot.clone()
    }

    pub(crate) fn summaries(&self) -> &[InstalledCatalogueSummary] {
        &self.summaries
    }

    pub(crate) fn is_usable(
        &self,
        workflow: DatCatalogueWorkflow,
        reference: &CatalogueRef,
    ) -> bool {
        let Some(summary) = self.summaries.iter().find(|row| &row.reference == reference) else {
            return false;
        };
        workflow.capability(summary)
            && (matches!(summary.availability, CatalogueAvailability::Ready)
                || workflow == DatCatalogueWorkflow::Verify
                    && matches!(
                        summary.availability,
                        CatalogueAvailability::NeedsValidation { .. }
                            | CatalogueAvailability::AggregateFolder { .. }
                    ))
    }

    /// Draw the same picker in any workflow.  The returned reference is only
    /// emitted by an explicit "Use selected catalogue" click; no row is ever
    /// selected implicitly.
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        workflow: DatCatalogueWorkflow,
        selected: &mut Option<CatalogueRef>,
    ) -> Option<CatalogueRef> {
        self.ensure_loaded();
        if self.loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Loading installed catalogues…");
            });
            return None;
        }
        if let Some(error) = &self.error {
            widgets::banner(
                ui,
                "Catalogue list could not be loaded",
                error,
                widgets::StatusTone::Blocked,
            );
            return None;
        }

        ui.horizontal(|ui| {
            ui.label(format!("Choose a catalogue for {}", workflow.label()));
            ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("Filter catalogues"));
            if ui.button("Refresh").clicked() {
                self.refresh();
            }
        });
        ui.label(
            egui::RichText::new("Select a row explicitly. EmuWiz will not guess between catalogues.")
                .color(theme::muted(ui)),
        );

        let query = self.query.trim().to_ascii_lowercase();
        let mut visible = 0usize;
        egui::ScrollArea::vertical()
            .id_salt(("dat-catalogue-picker", workflow.label()))
            .max_height(280.0)
            .show(ui, |ui| {
                for summary in &self.summaries {
                    if !query.is_empty() && !summary_matches(summary, &query) {
                        continue;
                    }
                    visible += 1;
                    let selected_row = selected.as_ref() == Some(&summary.reference);
                    let usable = self.is_usable(workflow, &summary.reference);
                    let response = egui::Frame::new()
                        .fill(if selected_row {
                            ui.visuals().selection.bg_fill.gamma_multiply(0.35)
                        } else {
                            theme::card_fill(ui)
                        })
                        .stroke(theme::border(ui))
                        .corner_radius(6)
                        .inner_margin(egui::Margin::symmetric(10, 7))
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(egui::RichText::new(&summary.display_name).strong());
                                ui.label(summary.store.label());
                                ui.label(availability_label(&summary.availability, usable));
                            });
                            ui.horizontal_wrapped(|ui| {
                                ui.label(evidence_label(&summary.platform));
                                ui.label(evidence_ecosystem_label(&summary.ecosystem));
                                if let Some(variant) = summary.variant.confirmed() {
                                    ui.label(variant.label());
                                }
                                if let Some(revision) = &summary.revision {
                                    ui.label(format!("revision {revision}"));
                                }
                                ui.label(provenance_label(&summary.provenance));
                            });
                            if !summary.availability.is_ready() {
                                ui.label(
                                    egui::RichText::new(summary.availability.reason())
                                        .color(theme::WARNING),
                                );
                            }
                            widgets::technical_details(
                                ui,
                                ("catalogue-technical-details", summary.reference.token()),
                                |ui| {
                                if let Some(path) = &summary.technical_path {
                                    ui.label(format!("Path: {}", path.display()));
                                }
                                ui.label(format!("Source: {}", summary.reference.token()));
                                if let Some(hash) = &summary.content_sha256 {
                                    ui.label(format!("Snapshot SHA-256: {hash}"));
                                }
                                },
                            );
                        })
                        .response
                        .interact(egui::Sense::click());
                    if response.clicked() {
                        *selected = Some(summary.reference.clone());
                    }
                }
            });
        if visible == 0 {
            ui.label(egui::RichText::new("No catalogues match this filter.").color(theme::muted(ui)));
        }
        if let Some(reference) = selected.as_ref() {
            ui.label(
                egui::RichText::new(format!("Selected: {}", reference.token()))
                    .color(theme::muted(ui)),
            );
            let matching = self.summaries.iter().find(|row| &row.reference == reference);
            if let Some(row) = matching
                && !workflow.capability(row)
            {
                ui.label(
                    egui::RichText::new("This catalogue is not available for this workflow.")
                        .color(theme::WARNING),
                );
            }
            if ui.button("Use selected catalogue").clicked() {
                return Some(reference.clone());
            }
        } else {
            ui.label(egui::RichText::new("No catalogue selected.").color(theme::muted(ui)));
        }
        None
    }
}

fn load_inventory() -> archivefs_core::Result<LoadedInventory> {
    let local_path = default_dat_sources_config_path()?;
    let local_config = load_dat_sources_config_from(&local_path)?;
    let (local_registry, _problems) = DatSourceRegistry::from_config(&local_config);
    let managed_config_path = default_managed_dat_sources_config_path()?;
    let managed_sources = load_managed_dat_sources_from(&managed_config_path)?;
    let managed_root = managed_dat_root()?;
    let snapshot = CatalogueInventorySnapshot {
        local_registry,
        managed_sources,
        managed_root,
    };
    let summaries = list_installed_catalogues(snapshot.inputs());
    Ok(LoadedInventory { summaries, snapshot })
}

fn summary_matches(summary: &InstalledCatalogueSummary, query: &str) -> bool {
    summary.display_name.to_ascii_lowercase().contains(query)
        || summary.store.label().to_ascii_lowercase().contains(query)
        || evidence_label(&summary.platform).to_ascii_lowercase().contains(query)
}

fn evidence_label(value: &EvidenceValue<String>) -> String {
    match value {
        EvidenceValue::Assigned(value) | EvidenceValue::Confirmed(value) => value.clone(),
        EvidenceValue::Ambiguous(values) => format!("Ambiguous: {}", values.join(", ")),
        EvidenceValue::Unknown => "Platform unknown".to_string(),
        EvidenceValue::Unavailable => "Platform unavailable".to_string(),
    }
}

fn evidence_ecosystem_label(
    value: &EvidenceValue<archivefs_core::dat::model::DatEcosystem>,
) -> String {
    match value {
        EvidenceValue::Assigned(value) | EvidenceValue::Confirmed(value) => value.label().to_string(),
        EvidenceValue::Ambiguous(_) => "Ecosystem ambiguous".to_string(),
        EvidenceValue::Unknown => "Ecosystem unknown".to_string(),
        EvidenceValue::Unavailable => "Ecosystem unavailable".to_string(),
    }
}

fn provenance_label(provenance: &CatalogueProvenance) -> String {
    match provenance {
        CatalogueProvenance::UserRegistered => "User registered".to_string(),
        CatalogueProvenance::NoIntroPackProjection => "No-Intro projection".to_string(),
        CatalogueProvenance::TosecReleasePackProjection { pack_id } => {
            format!("TOSEC release pack {pack_id}")
        }
        CatalogueProvenance::EmuWizManaged { provider } => {
            format!("Managed · {provider:?}")
        }
    }
}

fn availability_label(availability: &CatalogueAvailability, usable: bool) -> &'static str {
    if usable && matches!(availability, CatalogueAvailability::Ready) {
        return "Ready";
    }
    match availability {
        CatalogueAvailability::Ready => "Unavailable for this workflow",
        CatalogueAvailability::Missing { .. } => "Missing",
        CatalogueAvailability::NeedsValidation { .. } => "Needs validation",
        CatalogueAvailability::Corrupt { .. } => "Corrupt",
        CatalogueAvailability::StaleManagedState { .. } => "Stale",
        CatalogueAvailability::AggregateFolder { .. } => "Folder containing multiple catalogues",
        CatalogueAvailability::Unregistered { .. } => "Unregistered",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_starts_without_an_implicit_selection() {
        let picker = DatCataloguePickerState::default();
        assert!(picker.summaries.is_empty());
    }

    #[test]
    fn selection_is_a_typed_reference_not_a_row_index() {
        let reference = CatalogueRef::local("source");
        let mut selected = None;
        selected = Some(reference.clone());
        assert_eq!(selected, Some(reference));
    }

    #[test]
    fn unavailable_state_is_rendered_as_a_reason() {
        let state = CatalogueAvailability::StaleManagedState {
            reason: "snapshot is no longer current".to_string(),
        };
        assert_eq!(availability_label(&state, false), "Stale");
        assert_eq!(state.reason(), "snapshot is no longer current");
    }
}
