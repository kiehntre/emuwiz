//! Read-only MAME collection health presentation.

use archivefs_core::mame_playing_library::MamePlayingLibraryPlan;
use eframe::egui;

pub(super) fn show(ui: &mut egui::Ui) {
    show_with_playing_library_plan(ui, None);
}

/// Renders the read-only MAME playing-library projection when an inspection
/// workflow supplies one. `None` is the normal current state until a MAME
/// catalogue/inventory report has been loaded; it never invents metrics.
pub(super) fn show_with_playing_library_plan(
    ui: &mut egui::Ui,
    plan: Option<&MamePlayingLibraryPlan>,
) {
    egui::CollapsingHeader::new("MAME Collection Health")
        .default_open(false)
        .show(ui, |ui| {
            ui.label("Inspect a collection against a supplied current MAME -listxml catalogue.");
            ui.label("This page is analysis-only: EmuWiz does not rename, rebuild, download, or mutate ROMs.");
            egui::Grid::new("mame_collection_health_summary")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    ui.label("Emulator version"); ui.label("Not inspected"); ui.end_row();
                    ui.label("Collection root"); ui.label("Choose a root and current listxml in the inspection workflow"); ui.end_row();
                    ui.label("Health"); ui.label("Good / bad / unknown are reported separately"); ui.end_row();
                    ui.label("Set style"); ui.label("Merged, split, non-merged, exploded-self-contained, mixed, or unknown"); ui.end_row();
                    ui.label("Update readiness"); ui.label("Reusable, missing, obsolete, renamed, new, removed, and shared dependencies"); ui.end_row();
                });
            ui.strong("No update actions are available here");
            ui.label("The analyser prepares a read-only report only. ROM collection changes remain outside this feature.");
            ui.separator();
            ui.strong("Top shared problems");
            ui.label("Affected sets · Type · Preservation status · Potentially fixes N sets");
            ui.label("This is a shared device ROM used by many machines.");
            ui.label("This is a parent/shared dependency, not a game-specific file.");
            ui.label("MAME has no verified good dump for this chip.");
            ui.label("NO_DUMP items are preservation gaps, not ordinary missing collection files.");
            if ui.button("Export report").clicked() {
                ui.ctx().copy_text("MAME collection health export is available after an inspection report is loaded.".into());
            }
            ui.separator();
            ui.strong("MAME Playing Library preview");
            ui.label("Curates one practical representative per authoritative parent/clone family while retaining meaningful control-panel and regional variants.");
            if let Some(plan) = plan {
                egui::Grid::new("mame_playing_library_preview")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label("Archival collection"); ui.label(format!("{} sets", plan.archival_set_count)); ui.end_row();
                        ui.label("Proposed playing set"); ui.label(plan.projected_set_count.to_string()); ui.end_row();
                        ui.label("Estimated storage"); ui.label(format_bytes(plan.projected_storage_bytes)); ui.end_row();
                        ui.label("Estimated savings"); ui.label(format_bytes(plan.projected_savings_bytes)); ui.end_row();
                        ui.label("Ambiguities"); ui.label(plan.unresolved_cases.len().to_string()); ui.end_row();
                    });
                ui.label(format!("Preference rules are applied; {} sets excluded; {} BIOS/device support sets retained.", plan.excluded_sets.len(), plan.required_support_sets.len()));
                ui.label("Preview only: the archival collection is never mutated by this planner.");
            } else {
                ui.label("Load a MAME catalogue and complete collection report to preview selected sets, storage, savings, exclusions, dependencies, and ambiguities.");
                ui.label("No apply, copy, delete, rename, or source-update action is available here.");
            }
        });
}

fn format_bytes(value: Option<u64>) -> String {
    let Some(value) = value else {
        return "unknown".into();
    };
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut amount = value as f64;
    let mut unit = 0;
    while amount >= 1024.0 && unit + 1 < UNITS.len() {
        amount /= 1024.0;
        unit += 1;
    }
    format!("{amount:.1} {}", UNITS[unit])
}
