//! Blue "Texture packs" tab: a thin host for the existing Dolphin, PCSX2 and
//! PPSSPP texture-pack panels. Pack inspection, preview, the confirmed Apply,
//! the shared transaction journal, Undo and recovery all stay in those panels
//! and in archivefs-core; this module only decides *whether* a panel may be
//! shown for the selected game and keeps the panels' background jobs polled.

use std::path::Path;

use archivefs_core::game_identity::{GameIdentityReport, IdentityPlatform};
use eframe::egui;

use super::{Activity, NativeWorkflows, Route, Section};
use crate::emulator_setup::{DolphinProfilesState, Pcsx2ProfilesState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Engine {
    Dolphin,
    Pcsx2,
    Ppsspp,
}

/// Why the tab cannot offer texture packs for a selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Unavailable {
    /// No saved identity report for exactly this file.
    NoIdentity,
    /// A platform with no texture-pack engine in EmuWiz.
    Platform(&'static str),
    /// The platform is unknown; texture packs are never guessed from a name.
    UnknownPlatform,
}

/// The engine for a game, decided only by structural identity evidence for the
/// selected file - never by a filename or catalogue label.
pub(super) fn engine_for(
    report: Option<&GameIdentityReport>,
    path: &Path,
) -> Result<Engine, Unavailable> {
    let report = report
        .filter(|report| report.archive_path == path)
        .ok_or(Unavailable::NoIdentity)?;
    match report.platform {
        IdentityPlatform::GameCube | IdentityPlatform::Wii => Ok(Engine::Dolphin),
        IdentityPlatform::PlayStation2 => Ok(Engine::Pcsx2),
        IdentityPlatform::Psp => Ok(Engine::Ppsspp),
        IdentityPlatform::Other => Err(Unavailable::UnknownPlatform),
        other => Err(Unavailable::Platform(other.label())),
    }
}

impl NativeWorkflows {
    /// Whether any texture-pack job (picker, install or Undo) is running.
    pub(in crate::gui_v2) fn texture_busy(&self) -> bool {
        self.app.dolphin_texture_mod.is_busy()
            || self.app.dolphin_texture_mod.pcsx2_texture_mod.is_busy()
            || self.app.ppsspp_texture_mod.is_busy()
    }

    pub(in crate::gui_v2) fn show_texture_packs(
        &mut self,
        ui: &mut egui::Ui,
        selected: Option<&crate::gui_v2::library::Game>,
        activity: &mut Activity,
    ) -> Option<Route> {
        // These panels' workers are drained here: the legacy page that used to
        // poll them is not part of Blue.
        let changed = self.app.dolphin_texture_mod.poll()
            | self.app.dolphin_texture_mod.pcsx2_texture_mod.poll()
            | self.app.ppsspp_texture_mod.poll();
        if changed || self.texture_busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        ui.heading("Texture packs");
        ui.label("Dolphin (GameCube, Wii), PCSX2 (PlayStation 2) and PPSSPP (PSP). Choosing and previewing a pack changes nothing; installing writes only into the emulator's texture folder, keeps a journal and can be undone. Game files and disc images are never modified.");
        let Some(game) = selected else {
            ui.label("Choose a game before looking at texture packs.");
            return ui
                .button("Choose a game")
                .clicked()
                .then_some(Route::Section(Section::Games));
        };
        let path = game.archive.absolute_path.clone();
        let engine = match engine_for(game.archive.identity_report.as_ref(), &path) {
            Ok(engine) => engine,
            Err(Unavailable::NoIdentity | Unavailable::UnknownPlatform) => {
                ui.label("Texture packs are unavailable until this game's platform is identified from the file itself. Filenames and catalogue labels cannot authorise a write into an emulator folder.");
                return ui
                    .button("Review identity")
                    .clicked()
                    .then_some(Route::ReviewIdentity(game.archive.id));
            }
            Err(Unavailable::Platform(platform)) => {
                ui.label(format!(
                    "Texture packs are unavailable for {platform}: EmuWiz has no texture-pack engine for it. Nothing was changed."
                ));
                return None;
            }
        };
        if !self
            .app
            .cheat_workflow
            .as_ref()
            .is_some_and(|w| w.archive_path == path)
        {
            // Read-only: binds the game context and starts identity/profile scans.
            self.app.open_cheats_mods_workspace(ui.ctx(), path.clone());
        }
        match self.app.cheat_workflow.as_ref() {
            Some(workflow) if workflow.archive_path == path => {}
            Some(workflow) => {
                ui.label(format!("Previous game's workflow is still retained: {}. Return to that game to finish or review it before switching. No texture pack controls are available for this selection.", workflow.display_name));
                return None;
            }
            None => {
                ui.label("This selection could not be bound to a game workflow. Refresh the library and try again; no texture pack controls are available.");
                return None;
            }
        }
        match engine {
            Engine::Dolphin => self.show_dolphin_packs(ui),
            Engine::Pcsx2 => self.show_pcsx2_packs(ui),
            Engine::Ppsspp => self.show_ppsspp_packs(ui),
        }
        let _ = activity;
        None
    }

    fn show_dolphin_packs(&mut self, ui: &mut egui::Ui) {
        let Some(workflow) = self.app.cheat_workflow.as_mut() else {
            return;
        };
        match &self.app.emulator_readiness.dolphin_profiles {
            DolphinProfilesState::NotScanned | DolphinProfilesState::Error(_) => {
                if let DolphinProfilesState::Error(message) =
                    &self.app.emulator_readiness.dolphin_profiles
                {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        format!("Dolphin discovery failed: {message}"),
                    );
                }
                if ui.button("Scan for Dolphin").clicked() {
                    let context = ui.ctx().clone();
                    self.app.start_dolphin_profile_scan(context);
                }
            }
            DolphinProfilesState::Scanning { .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Scanning Dolphin profiles locally…");
                });
            }
            DolphinProfilesState::Ready(discovery) => {
                if discovery.profiles.is_empty() {
                    ui.label("No Dolphin profile was found, so texture packs are unavailable. Nothing was created.");
                    return;
                }
                let chosen = workflow.selected_dolphin_profile_id.clone();
                if chosen.as_deref().is_none_or(|id| {
                    !discovery
                        .profiles
                        .iter()
                        .any(|profile| profile.eligible && profile.profile_id == id)
                }) {
                    workflow.selected_dolphin_profile_id = None;
                    ui.label("Choose the Dolphin profile to install into.");
                }
                for profile in &discovery.profiles {
                    ui.push_id(("texture-dolphin-profile", &profile.profile_id), |ui| {
                        crate::cheats_mods::show_dolphin_profile_card(
                            ui,
                            workflow,
                            profile,
                            &mut self.app.clipboard,
                        );
                    });
                }
                let Some(profile) =
                    workflow
                        .selected_dolphin_profile_id
                        .as_deref()
                        .and_then(|id| {
                            discovery
                                .profiles
                                .iter()
                                .find(|profile| profile.eligible && profile.profile_id == id)
                        })
                else {
                    return;
                };
                crate::dolphin_texture_mod_page::show_dolphin_texture_mod_panel(
                    ui,
                    &mut self.app.dolphin_texture_mod,
                    &workflow.archive_path,
                    profile,
                    crate::ready_game_identity(workflow),
                );
            }
        }
    }

    fn show_pcsx2_packs(&mut self, ui: &mut egui::Ui) {
        let Some(workflow) = self.app.cheat_workflow.as_mut() else {
            return;
        };
        match &self.app.emulator_readiness.pcsx2_profiles {
            Pcsx2ProfilesState::NotScanned | Pcsx2ProfilesState::Error(_) => {
                if let Pcsx2ProfilesState::Error(message) =
                    &self.app.emulator_readiness.pcsx2_profiles
                {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        format!("PCSX2 discovery failed: {message}"),
                    );
                }
                if ui.button("Scan for PCSX2").clicked() {
                    let context = ui.ctx().clone();
                    self.app.start_pcsx2_profile_scan(context);
                }
            }
            Pcsx2ProfilesState::Scanning { .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Scanning PCSX2 profiles locally…");
                });
            }
            Pcsx2ProfilesState::Ready(discovery) => {
                if discovery.profiles.is_empty() {
                    ui.label("No PCSX2 profile was found, so texture packs are unavailable. Nothing was created.");
                    return;
                }
                let chosen = workflow.selected_pcsx2_profile_id.clone();
                if chosen.as_deref().is_none_or(|id| {
                    !discovery
                        .profiles
                        .iter()
                        .any(|profile| profile.eligible && profile.profile_id == id)
                }) {
                    workflow.selected_pcsx2_profile_id = None;
                    ui.label("Choose the PCSX2 profile to install into.");
                }
                for profile in &discovery.profiles {
                    ui.push_id(("texture-pcsx2-profile", &profile.profile_id), |ui| {
                        crate::cheats_mods::show_pcsx2_profile_card(
                            ui,
                            workflow,
                            profile,
                            &mut self.app.clipboard,
                        );
                    });
                }
                let Some(profile) = workflow
                    .selected_pcsx2_profile_id
                    .as_deref()
                    .and_then(|id| {
                        discovery
                            .profiles
                            .iter()
                            .find(|profile| profile.eligible && profile.profile_id == id)
                    })
                else {
                    return;
                };
                crate::pcsx2_texture_mod_page::show_pcsx2_texture_mod_panel(
                    ui,
                    &mut self.app.dolphin_texture_mod.pcsx2_texture_mod,
                    &workflow.archive_path,
                    profile,
                    crate::ready_game_identity(workflow),
                );
            }
        }
    }

    fn show_ppsspp_packs(&mut self, ui: &mut egui::Ui) {
        let Some(workflow) = self.app.cheat_workflow.as_ref() else {
            return;
        };
        let profile =
            self.app
                .ppsspp_texture_mod
                .eligible_profile(std::time::Instant::now(), || {
                    let roots =
                    archivefs_core::patch_manager::PpssppProfileDiscoveryRoots::from_environment()
                        .ok()?;
                    archivefs_core::patch_manager::discover_ppsspp_profiles(&roots)
                        .profiles
                        .into_iter()
                        .find(|profile| profile.eligible)
                });
        let Some(profile) = profile else {
            ui.label("No usable PPSSPP profile was found, so texture packs are unavailable. Nothing was created.");
            return;
        };
        crate::ppsspp_texture_mod_page::show_ppsspp_texture_mod_panel(
            ui,
            &mut self.app.ppsspp_texture_mod,
            &workflow.archive_path,
            &profile,
            crate::ready_game_identity(workflow),
        );
    }
}

#[cfg(test)]
#[path = "native_texture_packs/tests.rs"]
mod tests;
