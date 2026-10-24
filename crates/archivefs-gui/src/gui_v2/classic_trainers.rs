//! Classic trainer evidence and explicit pin creation in Blue Game Details.
//!
//! Lists POKE trainers found in a `.pok` file next to the selected game, where
//! they came from and which emulators could use them. A trainer is shown as a
//! verified association only when its target file pins it to a hash that
//! equals this game's current DAT-verified hash; everything else is a
//! candidate. Explicit pin creation is available after preview; applying
//! POKEs and launching with them remain unsupported.

mod pinning;

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use archivefs_core::classic_trainer_evidence::{
    ClassicTrainerGame, ClassicTrainerOption, ClassicTrainerReport, assess_sidecar_pok,
    exact_game_identities, poke_platform_for_label, trainer_target_path,
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
    identities: Vec<String>,
    sidecar: Option<(u64, Option<SystemTime>)>,
    target: Option<(u64, Option<SystemTime>)>,
}

#[derive(Default)]
pub(super) struct State {
    cached: Option<(Key, Result<Option<ClassicTrainerReport>, String>)>,
    pinning: pinning::Pinning,
}

fn stamp(path: &Path) -> Option<(u64, Option<SystemTime>)> {
    std::fs::symlink_metadata(path)
        .ok()
        .map(|metadata| (metadata.len(), metadata.modified().ok()))
}

fn sidecar_stamp(game_path: &Path) -> Option<(u64, Option<SystemTime>)> {
    ["pok", "POK"]
        .into_iter()
        .find_map(|extension| stamp(&game_path.with_extension(extension)))
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

/// The identity this game can offer a trainer: the audited hashes behind a
/// current DAT exact match, and nothing when the match is absent or the last
/// scan did not finish. A trainer's own pin is only compared against these.
fn trainer_game(game: &Game) -> ClassicTrainerGame {
    let verified = game.dat_exact.is_some() && !game.inspection_incomplete;
    let identities = if verified {
        game.dat_exact_hashes
            .as_ref()
            .map(exact_game_identities)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    ClassicTrainerGame {
        platform: game.platform.clone(),
        identity_verified: verified && !identities.is_empty(),
        identities,
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
            target: stamp(&trainer_target_path(&path)),
            platform: input.platform.clone(),
            verified: input.identity_verified,
            identities: input.identities.clone(),
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
        let mut refresh = false;
        egui::CollapsingHeader::new(RichText::new("Classic trainers").strong())
            .id_salt(("v2_detail_classic_trainers", game.archive.id))
            .default_open(false)
            .show(ui, |ui| {
                render(ui, result);
                let candidate = result
                    .as_ref()
                    .ok()
                    .and_then(|r| r.as_ref())
                    .is_some_and(|r| r.candidates().next().is_some());
                refresh = self.pinning.show(ui, game, candidate);
            });
        if refresh {
            self.cached = None;
            ui.ctx().request_repaint();
        }
    }
}

fn line(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(egui::Label::new(RichText::new(text.into()).size(13.0).color(blue::BODY_TEXT)).wrap());
}

/// The plain refusal shown on every card: no state here applies anything.
const APPLY_UNSUPPORTED: &str = "Applying POKEs is not supported yet. EmuWiz never applies a trainer, changes a game or launches an emulator with POKEs from this list, even for a verified association; that stays off until each emulator's support and launch checks are proven.";

fn render(ui: &mut egui::Ui, result: &Result<Option<ClassicTrainerReport>, String>) {
    line(ui, APPLY_UNSUPPORTED);
    line(
        ui,
        "A trainer matched by file name is a candidate, not verified compatibility with this release. It is a verified association only when its .pok.target file pins it to the same hash as this game's current DAT-verified copy.",
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
            if let Some(note) = report.pin.note() {
                line(ui, note);
            }
            let verified: Vec<_> = report.verified().collect();
            let candidates: Vec<_> = report.candidates().collect();
            if !verified.is_empty() {
                section(ui, "Verified for this release");
                verified
                    .into_iter()
                    .for_each(|option| render_option(ui, option));
            }
            if !candidates.is_empty() {
                section(ui, "Unverified candidates");
                candidates
                    .into_iter()
                    .for_each(|option| render_option(ui, option));
            }
        }
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(title).strong().color(blue::PRIMARY_TEXT));
}

fn render_option(ui: &mut egui::Ui, option: &ClassicTrainerOption) {
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
    let verified = option.association.is_verified();
    for emulator in &option.emulators {
        let verdict = match (emulator.exact_when_verified, verified) {
            (true, true) => "exact projection possible · not applied".to_string(),
            (true, false) => "could be exact once the release is verified".to_string(),
            (false, _) => emulator
                .blocker
                .as_ref()
                .map_or("preview only".to_string(), |b| {
                    format!("preview only ({b:?})")
                }),
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

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::classic_trainer_evidence::ClassicTrainerMatch;

    pub(super) fn texts(context: &egui::Context, run: impl FnOnce(&mut egui::Ui)) -> Vec<String> {
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
                identities: vec![SHA1.into()],
                identity_verified: true,
            },
            &dir.join("game.tap"),
        )
    }

    const SHA1: &str = "sha1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const POK: &str = "N Infinite lives\nZ 8 35899 0 53\nY\n";

    fn pin(dir: &Path, game_hash: &str) {
        use sha2::Digest;
        let trainer: String = sha2::Sha256::digest(POK.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        std::fs::write(dir.join("game.pok"), POK).unwrap();
        std::fs::write(
            dir.join("game.pok.target"),
            format!("trainer-sha256 {trainer}\ngame-sha1 {game_hash}\n"),
        )
        .unwrap();
    }

    #[test]
    fn verified_association_is_separated_from_candidates_and_never_applied() {
        let dir = tempfile::tempdir().unwrap();
        pin(dir.path(), &SHA1[5..]);
        let report = report_for(dir.path(), "ZX Spectrum");
        let context = egui::Context::default();
        let all = texts(&context, |ui| render(ui, &report)).join("\n");
        assert!(all.contains("Verified for this release"));
        assert!(!all.contains("Unverified candidates"));
        assert!(all.contains("pinned by its target file"));
        assert!(all.contains("Applying POKEs is not supported yet"));
        assert!(all.contains("exact projection possible · not applied"));
        assert!(!all.to_lowercase().contains("apply now"));

        // Pinned to another release: listed as a candidate, not verified.
        pin(dir.path(), &"b".repeat(40));
        let report = report_for(dir.path(), "ZX Spectrum");
        let all = texts(&context, |ui| render(ui, &report)).join("\n");
        assert!(!all.contains("Verified for this release"));
        assert!(all.contains("Unverified candidates"));
        assert!(all.contains("different release"));
    }

    #[test]
    fn trainer_game_offers_hashes_only_for_a_current_dat_exact_match() {
        use archivefs_core::dat::library_identity_summary::LibraryItemHashes;
        let mut game =
            super::super::library::Game::from_archive(archivefs_core::PersistedArchive {
                id: 1,
                source_folder_id: 1,
                relative_path: "g.tap".into(),
                absolute_path: "/x/g.tap".into(),
                archive_kind: "loose".into(),
                display_name: "g".into(),
                normalized_name: "g".into(),
                size_bytes: Some(3),
                modified_time_unix_seconds: None,
                platform: Some("ZX Spectrum".into()),
                platform_source: None,
                last_known_health: "ok".into(),
                last_seen_at: String::new(),
                last_verified_missing_at: None,
                identity_report: None,
            });
        game.dat_exact_hashes = Some(LibraryItemHashes {
            sha1: Some(SHA1[5..].into()),
            ..Default::default()
        });
        // Hashes without a current exact match are not offered.
        assert!(!trainer_game(&game).identity_verified);
        assert!(trainer_game(&game).identities.is_empty());
        game.mark_dat_exact(Some("TOSEC"));
        assert_eq!(trainer_game(&game).identities, vec![SHA1.to_string()]);
        assert!(trainer_game(&game).identity_verified);
        game.inspection_incomplete = true;
        assert!(!trainer_game(&game).identity_verified);
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
        assert!(all.contains("Applying POKEs is not supported yet"));
        assert!(all.contains("Unverified candidates"));
        assert!(!all.contains("Verified for this release"));
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
            identities: Vec::new(),
            sidecar: sidecar_stamp(&game_path),
            target: stamp(&trainer_target_path(&game_path)),
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
        assert_ne!(
            after,
            Key {
                identities: vec![SHA1.into()],
                ..after.clone()
            }
        );
        std::fs::write(dir.path().join("game.pok.target"), "x").unwrap();
        assert_ne!(
            after,
            Key {
                target: stamp(&trainer_target_path(&game_path)),
                ..after.clone()
            }
        );
    }
}
