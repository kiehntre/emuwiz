//! Read-only Dreamcast IP.BIN presentation for selected media.

use eframe::egui;

use super::library::Game;

pub(super) fn show(ui: &mut egui::Ui, game: &Game) {
    if !game.platform.eq_ignore_ascii_case("Dreamcast") {
        return;
    }
    let report = game.archive.identity_report.as_ref();
    let product = report.and_then(|report| {
        report.verified_value(archivefs_core::game_identity::IdentityKind::DreamcastProductCode)
    });
    egui::CollapsingHeader::new("Dreamcast boot metadata")
        .default_open(true)
        .show(ui, |ui| {
            ui.label("Read-only facts from the bounded Dreamcast IP.BIN inspection.");
            egui::Grid::new("dreamcast_ipbin_summary")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    ui.label("Product code");
                    ui.label(product.unwrap_or("Not established"));
                    ui.end_row();
                    ui.label("Format");
                    ui.label(
                        report.map_or_else(|| "Not inspected".to_string(), |report| format!("{:?}", report.format)),
                    );
                    ui.end_row();
                    ui.label("Version"); ui.label("Bounded IP.BIN metadata is retained by native inspection"); ui.end_row();
                    ui.label("Region"); ui.label("Declared area symbols are corroborating evidence only"); ui.end_row();
                    ui.label("VGA support"); ui.label("Declared / not declared / unknown — not a runtime guarantee"); ui.end_row();
                    ui.label("Boot program"); ui.label("Checked against the selected filesystem when available"); ui.end_row();
                    ui.label("Release date"); ui.label("Informational IP.BIN metadata"); ui.end_row();
                    ui.label("Title"); ui.label("Informational IP.BIN metadata"); ui.end_row();
                    ui.label("Peripheral flags"); ui.label("Known VGA meaning decoded; unknown bits remain raw"); ui.end_row();
                    ui.label("Validation"); ui.label(if report.is_some() { "Native Dreamcast inspection completed; warnings remain visible" } else { "Native Dreamcast inspection is not available" }); ui.end_row();
                });
            ui.collapsing("Advanced IP.BIN details", |ui| {
                ui.label(format!("Source image/container: {}", game.archive.archive_kind));
                ui.label("Raw field bytes are preserved by the core inspection record.");
                ui.label("Provenance: selected Dreamcast data track at logical offset zero.");
                ui.label("Identity contribution: recognised boot signature and product code only; stronger native/disc evidence keeps precedence.");
                if let Some(report) = report {
                    for warning in &report.warnings {
                        ui.colored_label(ui.visuals().warn_fg_color, warning);
                    }
                    ui.label(format!("Native inspection complete: {}", report.complete));
                } else {
                    ui.label("Warnings/conflicts: native inspection has not produced a report.");
                }
                ui.label("Warnings/conflicts: no automatic correction or mutation is performed.");
            });
        });
}
