//! Read-only MAME collection health presentation.

use eframe::egui;

pub(super) fn show(ui: &mut egui::Ui) {
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
        });
}
