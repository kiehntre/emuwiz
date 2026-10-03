//! Read-only Wii U WUD/WUX container presentation for selected media.

use eframe::egui;

use super::library::Game;

mod queue;
#[cfg(test)]
mod queue_tests;

fn is_wii_u(game: &Game) -> bool {
    game.platform.eq_ignore_ascii_case("Wii U") || game.platform.eq_ignore_ascii_case("WiiU")
}

fn is_container(game: &Game) -> bool {
    matches!(
        game.archive.archive_kind.to_ascii_lowercase().as_str(),
        "wud" | "wux" | "wua"
    ) || matches!(
        game.archive
            .absolute_path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("wud" | "wux" | "wua")
    )
}

pub(super) fn applies(game: &Game) -> bool {
    is_wii_u(game) && is_container(game)
}

pub(super) fn show(ui: &mut egui::Ui, game: &Game) {
    if !applies(game) {
        return;
    }

    let report = archivefs_core::wiiu_disc::inspect_wii_u_disc(&game.archive.absolute_path);
    egui::CollapsingHeader::new("Wii U disc container")
        .default_open(true)
        .show(ui, |ui| {
            ui.label("Read-only bounded WUD/WUX inspection. No keys are exposed.");
            egui::Grid::new("wiiu_disc_summary")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    ui.label("Format");
                    ui.label(format!("{:?}", report.format));
                    ui.end_row();
                    ui.label("Physical size");
                    ui.label(
                        report
                            .structure
                            .as_ref()
                            .map(|structure| format!("{} bytes", structure.physical_container_size_bytes))
                            .unwrap_or_else(|| "Unavailable".to_string()),
                    );
                    ui.end_row();
                    ui.label("Logical size");
                    ui.label(
                        report
                            .structure
                            .as_ref()
                            .and_then(|structure| structure.logical_disc_size_bytes)
                            .map(|size| format!("{} bytes", size))
                            .unwrap_or_else(|| "Not established".to_string()),
                    );
                    ui.end_row();
                    ui.label("Structural status");
                    ui.label(if report.structural_complete {
                        "Structurally valid"
                    } else {
                        "Incomplete or malformed"
                    });
                    ui.end_row();
                    ui.label("Key state");
                    ui.label(format!("{:?}", report.key_state));
                    ui.end_row();
                    ui.label("Future readiness");
                    ui.label(format!("{:?}", report.readiness));
                    ui.end_row();
                });
            if let Some(structure) = &report.structure {
                ui.label(format!("Split parts: {}", structure.parts.len()));
                if let Some(sector) = structure.sector_size_bytes {
                    ui.label(format!("WUX sector size: {sector} bytes"));
                }
                if let Some(blocks) = structure.block_count {
                    ui.label(format!("WUX block-table entries: {blocks}"));
                }
            }
            for issue in &report.issues {
                ui.colored_label(ui.visuals().warn_fg_color, format!("Issue: {issue:?}"));
            }
            ui.label("Your files are not changed by this inspection. Decryption and extraction are never performed.");
            show_conversion_readiness(ui, game, report.format);
        });
}

fn show_conversion_readiness(
    ui: &mut egui::Ui,
    game: &Game,
    format: archivefs_core::wiiu_disc::WiiUDiscFormat,
) {
    let direction = match format {
        archivefs_core::wiiu_disc::WiiUDiscFormat::Wud => {
            archivefs_core::wiiu_conversion::WiiUConversionDirection::WudToWux
        }
        archivefs_core::wiiu_disc::WiiUDiscFormat::Wux => {
            archivefs_core::wiiu_conversion::WiiUConversionDirection::WuxToWud
        }
        _ => return,
    };
    egui::CollapsingHeader::new("Convert this disc image")
        .default_open(true)
        .show(ui, |ui| queue::show(ui, game, direction));
}
