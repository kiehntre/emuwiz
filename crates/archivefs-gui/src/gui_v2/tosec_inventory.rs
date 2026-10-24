//! Read-only view over the existing applied TOSEC catalogue and persisted
//! Core DAT coverage. It deliberately does not scan files or claim release
//! completeness from canonical DAT-entry coverage.

use archivefs_core::dat::catalogue_selection::{
    CatalogueAvailability, CatalogueProvenance, CatalogueRef, EvidenceValue,
};
use archivefs_core::dat::coverage::{CompleteSetVerdict, CoverageSourceScope};
use eframe::egui;

use crate::dat_catalogue_picker::{self, DatCataloguePickerState, DatCatalogueWorkflow};
use crate::gui_v2::routes::{Route, Section};

const MISSING_PAGE_SIZE: u32 = 50;

#[derive(Default)]
pub(super) struct State {
    picker: DatCataloguePickerState,
    selected: Option<CatalogueRef>,
    coverage: Option<Result<Coverage, String>>,
}

struct Coverage {
    counts: archivefs_core::dat::coverage::PlatformDatCoverage,
    missing: Vec<String>,
}

impl State {
    pub(super) fn show(&mut self, ui: &mut egui::Ui) -> Option<Route> {
        self.picker.ensure_loaded();
        self.picker.poll();
        let mut route = None;
        ui.horizontal_wrapped(|ui| {
            if ui.button("‹ DAT Sources").clicked() {
                route = Some(Route::Section(Section::Dat));
            }
        });
        if route.is_some() {
            return route;
        }
        ui.heading("TOSEC Collection Inventory");
        ui.label("Read-only catalogue evidence from applied TOSEC release packs and existing DAT verification records.");
        let prior = self.selected.clone();
        let _ = self
            .picker
            .show(ui, DatCatalogueWorkflow::TosecInventory, &mut self.selected);
        if self.selected != prior {
            self.coverage = None;
        }
        if ui.button("Refresh catalogue and coverage").clicked() {
            self.picker.refresh();
            self.coverage = None;
        }

        let Some(reference) = self.selected.as_ref() else {
            ui.label("Select an applied TOSEC release-pack catalogue. Manually registered DATs are not treated as trusted TOSEC authority here.");
            return None;
        };
        let Some(summary) = self
            .picker
            .summaries()
            .iter()
            .find(|summary| &summary.reference == reference)
        else {
            ui.label("Catalogue details are unavailable. Refresh the catalogue list.");
            return None;
        };

        ui.separator();
        ui.strong(&summary.display_name);
        ui.label(format!("System: {}", evidence_text(&summary.platform)));
        ui.label(format!(
            "Version: {}",
            summary
                .revision
                .as_deref()
                .unwrap_or("Unknown (not exposed by the imported pack projection)")
        ));
        ui.label(format!(
            "Provenance: {}",
            dat_catalogue_picker::provenance_label(&summary.provenance)
        ));
        if !matches!(summary.availability, CatalogueAvailability::Ready) {
            ui.colored_label(
                egui::Color32::YELLOW,
                format!("Catalogue status: {}", summary.availability.reason()),
            );
            ui.label("Coverage is unavailable until the catalogue is current and ready.");
            return show_workflow_links(ui);
        }
        if self.coverage.is_none() {
            self.coverage = Some(match self.picker.snapshot() {
                Some(snapshot) => read_coverage(summary, &snapshot.local_registry),
                None => Err("Catalogue configuration is not available.".into()),
            });
        }
        match self.coverage.as_ref().expect("coverage populated") {
            Err(error) => {
                ui.label(format!("Coverage: Unknown · {error}"));
            }
            Ok(coverage) => {
                let c = &coverage.counts;
                ui.separator();
                ui.label(format!(
                    "Verified media files: {} current · {} stale",
                    c.verified_current, c.verified_stale
                ));
                ui.label(format!(
                    "Unmatched files: {} · Ambiguous files: {} · Other/unknown: {}",
                    c.unmatched, c.ambiguous, c.unknown
                ));
                match (c.expected_unique_count, c.missing_count) {
                    (Some(expected), Some(missing)) => ui.label(format!(
                        "Catalogue entries: {expected} · Missing entries: {missing}"
                    )),
                    _ => ui.label(format!(
                        "Catalogue entries: Unknown · Missing entries: Unknown ({})",
                        c.expected_inventory.unavailable_reason()
                    )),
                };
                match &c.complete_set {
                    CompleteSetVerdict::Complete { .. } => {
                        ui.label("DAT entry coverage: complete for this catalogue snapshot.")
                    }
                    CompleteSetVerdict::Incomplete { missing_count } => ui.label(format!(
                        "DAT entry coverage: incomplete · {missing_count} entries missing."
                    )),
                    CompleteSetVerdict::NotProvable { reason } => {
                        ui.label(format!("DAT entry coverage: Unknown · {reason}"))
                    }
                };
                ui.strong(format!(
                    "Release completeness: {}",
                    release_completeness_label()
                ));
                ui.label("Multi-disk relationships: Unknown; this persisted TOSEC coverage view does not expose release-level media relationships.");
                ui.label("A verified hash confirms an individual media file. The available TOSEC inventory does not model required media across each release, so this view does not claim a game is complete or infer single-disk releases.");
                if !coverage.missing.is_empty() {
                    ui.collapsing(
                        format!("First {} missing catalogue entries", coverage.missing.len()),
                        |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(220.0)
                                .show(ui, |ui| {
                                    for name in &coverage.missing {
                                        ui.label(name);
                                    }
                                });
                        },
                    );
                }
            }
        }
        show_workflow_links(ui)
    }
}

fn show_workflow_links(ui: &mut egui::Ui) -> Option<Route> {
    let mut route = None;
    ui.horizontal_wrapped(|ui| {
        if ui.button("DAT Sources").clicked() {
            route = Some(Route::Section(Section::Dat));
        }
        if ui.button("Check Games").clicked() {
            route = Some(Route::Section(Section::Check));
        }
        if ui.button("Review problems").clicked() {
            route = Some(Route::Section(Section::ProblemsRepair));
        }
    });
    route
}

fn evidence_text(value: &EvidenceValue<String>) -> &str {
    match value {
        EvidenceValue::Assigned(value) | EvidenceValue::Confirmed(value) => value,
        EvidenceValue::Ambiguous(_) => "Ambiguous",
        EvidenceValue::Unknown => "Unknown",
        EvidenceValue::Unavailable => "Unavailable",
    }
}

fn release_completeness_label() -> &'static str {
    "Unknown"
}

fn read_coverage(
    summary: &archivefs_core::dat::catalogue_selection::InstalledCatalogueSummary,
    registry: &archivefs_core::dat::sources::DatSourceRegistry,
) -> Result<Coverage, String> {
    let source_id = match &summary.reference {
        CatalogueRef::Local {
            source_id,
            member: None,
        } => source_id.as_str(),
        _ => return Err("This catalogue has no direct local source identity.".into()),
    };
    if !matches!(
        summary.provenance,
        CatalogueProvenance::TosecReleasePackProjection { .. }
    ) {
        return Err("Only applied TOSEC release-pack catalogues are supported.".into());
    }
    let platform = registry
        .get(source_id)
        .and_then(|source| source.platform.as_deref())
        .ok_or_else(|| "The catalogue has no explicit system assignment.".to_string())?;
    let path = archivefs_core::default_database_path().map_err(|error| error.to_string())?;
    let database = archivefs_core::Database::open_read_only(&path)
        .map_err(|error| format!("Library evidence could not be opened read-only: {error}"))?;
    let current_revision = summary.revision.as_deref();
    let scope = CoverageSourceScope {
        source_platform: Some(platform),
        current_source_revision: current_revision,
        configured: summary.enabled,
    };
    let counts = database
        .platform_dat_coverage(platform, source_id, scope)
        .map_err(|error| error.to_string())?;
    let missing = if counts.missing_count.unwrap_or(0) > 0 {
        database
            .missing_dat_canonical_identities(platform, source_id, scope, MISSING_PAGE_SIZE, 0)
            .map_err(|error| error.to_string())?
    } else {
        Vec::new()
    };
    Ok(Coverage { counts, missing })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_ambiguity_is_never_collapsed_to_a_system() {
        let value = EvidenceValue::Ambiguous(vec!["Amiga".to_string(), "Atari ST".to_string()]);
        assert_eq!(evidence_text(&value), "Ambiguous");
    }

    #[test]
    fn absent_platform_evidence_is_unknown() {
        assert_eq!(evidence_text(&EvidenceValue::Unknown), "Unknown");
    }

    #[test]
    fn canonical_entry_coverage_never_claims_a_complete_release() {
        assert_eq!(release_completeness_label(), "Unknown");
    }

    #[test]
    fn missing_entry_sample_is_bounded() {
        assert_eq!(MISSING_PAGE_SIZE, 50);
    }
}
