//! Native Blue "Providers and code previews" (boards `D11-LG-CheatExtras`,
//! `-CheatExtrasCompact`).
//!
//! Read-only, like the legacy Cheat sources extras it replaces:
//! - the open and local cheat providers and their licences
//!   (`open_cheat_provider_catalogue`);
//! - a Classic Game Genie preview (`decode_classic_game_genie`) that only
//!   decodes pasted codes: no ROM is patched and nothing is applied;
//! - which emulators can use classic POKEs at runtime
//!   (`poke_runtime_capability`); no emulator is launched.

use archivefs_core::patch_manager::{
    GameGeniePlatform, GameGenieRevisionEvidence, OpenCheatProviderMode, PokeRuntimeEmulator,
    decode_classic_game_genie, open_cheat_provider_catalogue, poke_runtime_capability,
};
use eframe::egui::{self, RichText};

use super::simple_shell::palette;
use crate::ui::components::{self as widgets, StatusTone};
use crate::ui::theme::{self, blue};

pub(super) const NO_APPLY: &str = "No ROM patch or hidden apply is performed.";

pub(super) struct CheatExtrasState {
    pub codes: String,
    pub platform: GameGeniePlatform,
}

impl Default for CheatExtrasState {
    fn default() -> Self {
        Self {
            codes: String::new(),
            platform: GameGeniePlatform::Nes,
        }
    }
}

const PLATFORMS: [GameGeniePlatform; 6] = [
    GameGeniePlatform::Nes,
    GameGeniePlatform::Snes,
    GameGeniePlatform::GameBoy,
    GameGeniePlatform::Genesis,
    GameGeniePlatform::MasterSystem,
    GameGeniePlatform::GameGear,
];

const POKE_EMULATORS: [PokeRuntimeEmulator; 7] = [
    PokeRuntimeEmulator::Fuse,
    PokeRuntimeEmulator::Vice,
    PokeRuntimeEmulator::Hatari,
    PokeRuntimeEmulator::Caprice32,
    PokeRuntimeEmulator::OpenMsx,
    PokeRuntimeEmulator::BeebEm,
    PokeRuntimeEmulator::BEm,
];

fn platform_label(platform: GameGeniePlatform) -> &'static str {
    match platform {
        GameGeniePlatform::Nes => "NES",
        GameGeniePlatform::Snes => "SNES",
        GameGeniePlatform::GameBoy => "Game Boy",
        GameGeniePlatform::Genesis => "Mega Drive / Genesis",
        GameGeniePlatform::MasterSystem => "Master System",
        GameGeniePlatform::GameGear => "Game Gear",
    }
}

/// How a provider's material reaches EmuWiz, in plain words.
pub(super) fn mode_badge(mode: OpenCheatProviderMode) -> (&'static str, StatusTone) {
    match mode {
        OpenCheatProviderMode::Bundled => ("Bundled", StatusTone::Success),
        OpenCheatProviderMode::Downloadable => ("Download on request", StatusTone::Info),
        OpenCheatProviderMode::MetadataIndex => ("Index only", StatusTone::Pending),
        OpenCheatProviderMode::UserImport => ("Your own files", StatusTone::Info),
    }
}

fn emulator_label(emulator: PokeRuntimeEmulator) -> &'static str {
    match emulator {
        PokeRuntimeEmulator::Fuse => "Fuse",
        PokeRuntimeEmulator::Vice => "VICE",
        PokeRuntimeEmulator::Hatari => "Hatari",
        PokeRuntimeEmulator::Caprice32 => "Caprice32",
        PokeRuntimeEmulator::OpenMsx => "openMSX",
        PokeRuntimeEmulator::BeebEm => "BeebEm",
        PokeRuntimeEmulator::BEm => "B-em",
    }
}

/// One decoded line of the preview, in plain words.
pub(super) fn preview_lines(
    platform: GameGeniePlatform,
    codes: &str,
) -> Vec<(String, Vec<String>)> {
    codes
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|code| {
            let result =
                decode_classic_game_genie(platform, code, &GameGenieRevisionEvidence::unverified());
            let mut lines = vec![format!("Status: {:?}", result.status)];
            if let Some(instruction) = result.instruction {
                lines.push(format!(
                    "Address 0x{:X} · value 0x{:02X} · {} bits",
                    instruction.address, instruction.value, instruction.width_bits
                ));
                if let Some(compare) = instruction.compare {
                    lines.push(format!(
                        "Compare 0x{compare:02X}: only applies when the original byte matches."
                    ));
                }
                lines.push("ROM revision unverified: preview only.".into());
            }
            for issue in &result.issues {
                lines.push(format!("Issue: {issue:?}"));
            }
            (code.to_string(), lines)
        })
        .collect()
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut CheatExtrasState) {
    super::game_detail_blue::side_card(ui, "OPEN AND LOCAL CHEAT PROVIDERS", |ui| {
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Reference, download and import options. EmuWiz does not bundle unclear or community payloads automatically.",
                )
                .color(palette::SECONDARY_TEXT),
            )
            .wrap(),
        );
        ui.add_space(4.0);
        for provider in open_cheat_provider_catalogue() {
            ui.push_id(("v2_cheat_provider", &provider.id), |ui| {
                egui::Frame::new()
                    .fill(blue::INSET)
                    .stroke(egui::Stroke::new(1.0_f32, palette::BORDER_SUBTLE))
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(&provider.name)
                                    .strong()
                                    .color(blue::PRIMARY_TEXT),
                            );
                            let (label, tone) = mode_badge(provider.mode);
                            widgets::status_badge(ui, label, tone);
                        });
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!(
                                    "{} · Licence: {} · {}",
                                    provider.platform_coverage.join(", "),
                                    provider.licence,
                                    provider.redistribution_status
                                ))
                                .size(13.0)
                                .color(palette::SECONDARY_TEXT),
                            )
                            .wrap(),
                        );
                    });
            });
            ui.add_space(4.0);
        }
    });
    super::game_detail_blue::side_card(ui, "CLASSIC GAME GENIE PREVIEW", |ui| {
        ui.label(
            RichText::new(format!(
                "Paste codes and pick a platform to see what they mean. {NO_APPLY}"
            ))
            .color(blue::BODY_TEXT),
        );
        ui.horizontal_wrapped(|ui| {
            ui.label("Platform");
            egui::ComboBox::from_id_salt("v2_game_genie_platform")
                .selected_text(platform_label(state.platform))
                .width(200.0)
                .show_ui(ui, |ui| {
                    for platform in PLATFORMS {
                        ui.selectable_value(
                            &mut state.platform,
                            platform,
                            platform_label(platform),
                        );
                    }
                });
            ui.label(
                RichText::new("Choose explicitly when platforms share a code shape.")
                    .size(13.0)
                    .color(palette::SECONDARY_TEXT),
            );
        });
        ui.add(
            egui::TextEdit::multiline(&mut state.codes)
                .hint_text("Paste Game Genie codes, one per line")
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        let previews = preview_lines(state.platform, &state.codes);
        if previews.is_empty() {
            ui.label(
                RichText::new("Nothing to preview yet. This preview is read-only and local.")
                    .color(palette::SECONDARY_TEXT),
            );
        }
        for (index, (code, lines)) in previews.iter().enumerate() {
            ui.push_id(("v2_game_genie_line", index), |ui| {
                ui.label(
                    RichText::new(code)
                        .monospace()
                        .strong()
                        .color(blue::PRIMARY_TEXT),
                );
                for line in lines {
                    ui.add(
                        egui::Label::new(RichText::new(line).size(13.0).color(blue::BODY_TEXT))
                            .wrap(),
                    );
                }
                ui.add_space(4.0);
            });
        }
    });
    super::game_detail_blue::side_card(ui, "CLASSIC POKES AT RUNTIME", |ui| {
        ui.label(
            RichText::new(
                "POKE codes: which emulators can use them at runtime. No emulator is launched from this page.",
            )
            .color(blue::BODY_TEXT),
        );
        for emulator in POKE_EMULATORS {
            let capability = poke_runtime_capability(emulator);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(110.0, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(110.0);
                        ui.label(RichText::new(emulator_label(emulator)).color(blue::DIM_TEXT));
                    },
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(format!(
                            "{} — {}",
                            capability.support.plain_label(),
                            capability.detail
                        ))
                        .size(13.0)
                        .color(blue::BODY_TEXT),
                    )
                    .wrap(),
                );
            });
        }
    });
    ui.add_space(theme::SPACE_XS);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_preview_decodes_only_and_says_so() {
        // A well-known NES code shape; whatever the decoder says, the
        // preview never claims to apply it.
        let lines = preview_lines(GameGeniePlatform::Nes, "  SXIOPO \n\n");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].0, "SXIOPO");
        assert!(lines[0].1[0].starts_with("Status: "));
        assert!(preview_lines(GameGeniePlatform::Snes, "   ").is_empty());
        assert!(NO_APPLY.contains("No ROM patch"));
    }

    #[test]
    fn every_provider_mode_and_poke_emulator_is_described() {
        for provider in open_cheat_provider_catalogue() {
            let (label, _) = mode_badge(provider.mode);
            assert!(!label.is_empty());
        }
        for emulator in POKE_EMULATORS {
            assert!(
                !poke_runtime_capability(emulator)
                    .support
                    .plain_label()
                    .is_empty()
            );
        }
    }

    #[test]
    fn the_page_fits_and_renders_its_three_sections() {
        let context = egui::Context::default();
        crate::gui_v2::readable_style(&context);
        let mut state = CheatExtrasState {
            codes: "SXIOPO\nNOT A CODE".into(),
            ..Default::default()
        };
        for width in [600.0, 1500.0] {
            let mut used = 0.0_f32;
            let output = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 2400.0),
                    )),
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            show(ui, &mut state);
                            used = ui.min_rect().right();
                        });
                    });
                },
            );
            assert!(used <= width, "used {used} of {width}");
            let mut texts = Vec::new();
            fn gather(shape: &egui::Shape, out: &mut Vec<String>) {
                match shape {
                    egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
                    egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| gather(s, out)),
                    _ => {}
                }
            }
            for shape in &output.shapes {
                gather(&shape.shape, &mut texts);
            }
            for wanted in [
                "OPEN AND LOCAL CHEAT PROVIDERS",
                "CLASSIC GAME GENIE PREVIEW",
                "CLASSIC POKES AT RUNTIME",
                "NOT A CODE",
            ] {
                assert!(texts.iter().any(|text| text.contains(wanted)), "{wanted}");
            }
        }
    }
}
