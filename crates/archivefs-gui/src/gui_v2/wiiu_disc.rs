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
                    ui.label(format_label(report.format));
                    ui.end_row();
                    ui.label("Space used by this file");
                    ui.label(
                        report
                            .structure
                            .as_ref()
                            .map(|structure| format!("{} bytes", structure.physical_container_size_bytes))
                            .unwrap_or_else(|| "Unavailable".to_string()),
                    );
                    ui.end_row();
                    ui.label("Uncompressed disc size");
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
                    ui.label(key_label(report.key_state));
                    ui.end_row();
                    ui.label("What this check establishes");
                    ui.label(readiness_label(report.readiness));
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
            if !report.issues.is_empty() {
                ui.colored_label(ui.visuals().warn_fg_color, "This inspection has limitations or found a problem. Keep every split part together and check the original dump before attempting conversion.");
                crate::ui::components::technical_details(ui, "wiiu_inspection_issues", |ui| {
                    for issue in &report.issues { ui.label(format!("{issue:?}")); }
                });
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

fn format_label(format: archivefs_core::wiiu_disc::WiiUDiscFormat) -> &'static str {
    use archivefs_core::wiiu_disc::WiiUDiscFormat::*;
    match format {
        Wud => "WUD — uncompressed disc",
        Wux => "WUX — compressed disc",
        Wua => "WUA — separate archive format",
        Unknown => "Not recognised",
    }
}
fn key_label(state: archivefs_core::wiiu_disc::WiiUDiscKeyState) -> &'static str {
    use archivefs_core::wiiu_disc::WiiUDiscKeyState::*;
    match state {
        NotRequiredForContainerInspection => "No key needed to check the container",
        RequiredForDeeperInspection => "A key is needed to inspect encrypted contents",
        AvailableLocally => "Available on this computer",
        Missing => "Missing — encrypted contents cannot be checked",
        Invalid => "The supplied key was not accepted",
        Unknown => "Not checked",
    }
}
fn readiness_label(state: archivefs_core::wiiu_disc::WiiUDiscReadiness) -> &'static str {
    use archivefs_core::wiiu_disc::WiiUDiscReadiness::*;
    match state {
        ReadyForContainerInspection => "Container can be inspected; game contents are not verified",
        StructurallyComplete => {
            "Container structure is complete; this does not prove the game will run"
        }
        StructurallyIncomplete => {
            "Container is incomplete or damaged; check the source and split parts"
        }
        RequiresKeysForDeeperInspection => "Encrypted contents require a separate key check",
        UnsupportedRepresentation => "This type cannot be inspected here",
    }
}
#[cfg(test)]
mod presentation_tests {
    use super::*;
    use archivefs_core::wiiu_disc::{WiiUDiscKeyState as K, WiiUDiscReadiness as R};
    #[test]
    fn container_readiness_does_not_claim_game_or_key_verification() {
        assert!(readiness_label(R::StructurallyComplete).contains("does not prove"));
        assert!(readiness_label(R::ReadyForContainerInspection).contains("not verified"));
        assert!(key_label(K::Missing).contains("cannot be checked"));
        assert_eq!(key_label(K::Unknown), "Not checked");
    }
}
