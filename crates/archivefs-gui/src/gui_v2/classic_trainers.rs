//! Read-only "Classic trainers" section of Blue Game Details.
//!
//! Lists POKE trainers found in a `.pok` file next to the selected game, where
//! they came from and which emulators could use them. Nothing is applied,
//! launched or written: no launch path consumes a POKE projection yet, so
//! every state here is preview or preparation evidence only.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use archivefs_core::classic_trainer_evidence::{
    ClassicTrainerGame, ClassicTrainerReport, assess_sidecar_pok, poke_platform_for_label,
};
use archivefs_core::patch_manager::PokeRuntimeEmulator;
use eframe::egui::{self, RichText};

use super::library::Game;
use crate::ui::theme::blue;

/// What the cached report was built from. Any change rebuilds it, so a
/// different game, a changed file or a changed verification never shows an
/// older answer.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    game_id: i64,
    path: PathBuf,
    platform: String,
    verified: bool,
    sidecar: Option<(u64, Option<SystemTime>)>,
}

#[derive(Default)]
pub(super) struct State {
    cached: Option<(Key, Result<Option<ClassicTrainerReport>, String>)>,
}

fn sidecar_stamp(game_path: &Path) -> Option<(u64, Option<SystemTime>)> {
    ["pok", "POK"].into_iter().find_map(|extension| {
        std::fs::symlink_metadata(game_path.with_extension(extension))
            .ok()
            .map(|metadata| (metadata.len(), metadata.modified().ok()))
    })
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

/// The identity this game can offer a trainer. A DAT exact match proves the
/// game's file, but the sidecar carries no identity of its own, so it can
/// never be promoted to verified from here.
fn trainer_game(game: &Game) -> ClassicTrainerGame {
    ClassicTrainerGame {
        platform: game.platform.clone(),
        identity: None,
        identity_verified: game.dat_exact.is_some() && !game.inspection_incomplete,
    }
}

impl State {
    pub(super) fn show(&mut self, ui: &mut egui::Ui, game: &Game) {
        if poke_platform_for_label(&game.platform).is_none() {
            self.cached = None;
            return;
        }
        let path = game.archive.absolute_path.clone();
        let input = trainer_game(game);
        let key = Key {
            game_id: game.archive.id,
            sidecar: sidecar_stamp(&path),
            platform: input.platform.clone(),
            verified: input.identity_verified,
            path,
        };
        if self
            .cached
            .as_ref()
            .is_none_or(|(cached, _)| *cached != key)
        {
            let result = assess_sidecar_pok(&input, &key.path);
            self.cached = Some((key, result));
        }
        let Some((_, result)) = &self.cached else {
            return;
        };
        egui::CollapsingHeader::new(RichText::new("Classic trainers").strong())
            .id_salt(("v2_detail_classic_trainers", game.archive.id))
            .default_open(false)
            .show(ui, |ui| render(ui, result));
    }
}

fn line(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(egui::Label::new(RichText::new(text.into()).size(13.0).color(blue::BODY_TEXT)).wrap());
}

fn render(ui: &mut egui::Ui, result: &Result<Option<ClassicTrainerReport>, String>) {
    line(
        ui,
        "Nothing here is applied. A trainer matched by file name is a candidate, not verified compatibility with this release. EmuWiz also has no launch path that applies POKEs, so it never changes a game or an emulator from this list.",
    );
    match result {
        Err(error) => line(ui, format!("Trainer file could not be read: {error}")),
        Ok(None) => line(
            ui,
            "No .pok trainer file with the same name was found next to this game.",
        ),
        Ok(Some(report)) => {
            if let Some(problem) = &report.problem {
                line(ui, format!("No trainers listed — {problem}"));
            }
            for option in &report.options {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(&option.title)
                        .strong()
                        .color(blue::PRIMARY_TEXT),
                );
                line(
                    ui,
                    format!(
                        "{} · {} POKE{}",
                        option.platform.label(),
                        option.poke_count,
                        if option.poke_count == 1 { "" } else { "s" }
                    ),
                );
                line(ui, format!("Source: {}", option.source));
                line(ui, option.state_label());
                for emulator in &option.emulators {
                    let verdict = if emulator.exact_when_verified {
                        "could be exact once the release is verified".to_string()
                    } else {
                        emulator
                            .blocker
                            .as_ref()
                            .map_or("preview only".to_string(), |b| {
                                format!("preview only ({b:?})")
                            })
                    };
                    line(
                        ui,
                        format!(
                            "{} — {} · {}",
                            emulator_label(emulator.emulator),
                            emulator.support.plain_label(),
                            verdict
                        ),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::classic_trainer_evidence::ClassicTrainerMatch;

    fn texts(context: &egui::Context, run: impl FnOnce(&mut egui::Ui)) -> Vec<String> {
        let mut run = Some(run);
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 1600.0),
                )),
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    if let Some(run) = run.take() {
                        run(ui);
                    }
                });
            },
        );
        fn gather(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| gather(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in &output.shapes {
            gather(&shape.shape, &mut out);
        }
        out
    }

    fn report_for(dir: &Path, platform: &str) -> Result<Option<ClassicTrainerReport>, String> {
        assess_sidecar_pok(
            &ClassicTrainerGame {
                platform: platform.into(),
                identity: None,
                identity_verified: true,
            },
            &dir.join("game.tap"),
        )
    }

    #[test]
    fn card_says_nothing_is_applied_and_shows_source_and_states() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("game.pok"),
            "N Infinite lives\nZ 8 35899 0 53\nY\n",
        )
        .unwrap();
        let report = report_for(dir.path(), "ZX Spectrum");
        let context = egui::Context::default();
        let all = texts(&context, |ui| render(ui, &report)).join("\n");
        assert!(all.contains("Nothing here is applied"));
        assert!(all.contains("not verified compatibility"));
        assert!(all.contains("Infinite lives"));
        assert!(all.contains("game.pok"));
        assert!(all.contains("Candidate only: matched by file name"));
        assert!(all.contains("Fuse"));
        assert!(!all.to_lowercase().contains("apply now"));
    }

    #[test]
    fn missing_malformed_and_error_states_are_explained() {
        let dir = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let none = report_for(dir.path(), "ZX Spectrum");
        assert!(
            texts(&context, |ui| render(ui, &none))
                .join(" ")
                .contains("No .pok trainer file")
        );
        std::fs::write(dir.path().join("game.pok"), "nonsense").unwrap();
        let bad = report_for(dir.path(), "ZX Spectrum");
        assert!(
            texts(&context, |ui| render(ui, &bad))
                .join(" ")
                .contains("No trainers listed")
        );
        let err: Result<Option<ClassicTrainerReport>, String> = Err("boom".into());
        assert!(
            texts(&context, |ui| render(ui, &err))
                .join(" ")
                .contains("boom")
        );
    }

    #[test]
    fn unsupported_emulator_is_shown_as_preview_only() {
        let dir = tempfile::tempdir().unwrap();
        // A ZX trainer next to a Commodore 64 game is ignored, not offered to VICE.
        std::fs::write(dir.path().join("game.pok"), "N x\nZ 8 35899 0 53\nY\n").unwrap();
        let report = report_for(dir.path(), "Commodore 64").unwrap().unwrap();
        assert_eq!(
            report.options[0].association,
            ClassicTrainerMatch::WrongPlatform
        );
        assert!(report.options[0].emulators.is_empty());
        let context = egui::Context::default();
        let shown = texts(&context, |ui| render(ui, &Ok(Some(report)))).join(" ");
        assert!(shown.contains("Not for this platform"));
        assert!(!shown.contains("VICE"));
    }

    #[test]
    fn cache_key_changes_with_game_file_and_verification() {
        let dir = tempfile::tempdir().unwrap();
        let game_path = dir.path().join("game.tap");
        let base = Key {
            game_id: 1,
            path: game_path.clone(),
            platform: "ZX Spectrum".into(),
            verified: false,
            sidecar: sidecar_stamp(&game_path),
        };
        assert_eq!(base.sidecar, None);
        std::fs::write(dir.path().join("game.pok"), "N x\nZ 8 1 0 1\nY\n").unwrap();
        let after = Key {
            sidecar: sidecar_stamp(&game_path),
            ..base.clone()
        };
        assert_ne!(base, after);
        assert_ne!(
            after,
            Key {
                game_id: 2,
                ..after.clone()
            }
        );
        assert_ne!(
            after,
            Key {
                verified: true,
                ..after.clone()
            }
        );
    }
}
