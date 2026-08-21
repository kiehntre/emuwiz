//! Emulator Adapter Batch B: a read-only "RPCS3" section - environment
//! health plus, only when the caller already has an authoritative PS3
//! title ID, a mapping of local RPCS3 game/update/DLC/config/patch state
//! for the selected title.
//!
//! # RPCS3 is not an identity authority
//!
//! This module never resolves or guesses a PS3 title ID itself. It only
//! ever accepts one, already-verified, from its caller (see
//! [`gather_rpcs3_status`]'s `verified_title_id` parameter) and passes it
//! straight through to
//! [`archivefs_core::patch_manager::Rpcs3GameRequest::verified_ps3_title_id`],
//! the exact same identity-safety boundary
//! [`archivefs_core::patch_manager::rpcs3_local`] itself enforces core-side
//! (see that module's own doc comment). When no verified title ID is
//! available this panel still shows RPCS3's own local environment health
//! (detected, config readable, firmware, dev_hdd0 readable), but never
//! claims a game mapping.
//!
//! # No mutation
//!
//! This module never starts RPCS3, edits a config file, enables a patch,
//! installs anything, or touches a save/trophy/RAP. Loading is always an
//! explicit action, off the UI thread, following the same generation-
//! guarded background-load convention this crate already uses elsewhere
//! (`thread::spawn` + `mpsc::channel` + a stale-result generation check).

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use archivefs_core::patch_manager::{
    Rpcs3FirmwareStatus, Rpcs3GameInspection, Rpcs3GameRequest, Rpcs3InstallationType,
    Rpcs3ProfileDiscoveryRoots, discover_rpcs3_profiles, inspect_rpcs3_game,
};
use eframe::egui;

use crate::ui::components as widgets;

// ---------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rpcs3FoundProfile {
    pub(crate) configuration_path: PathBuf,
    pub(crate) dev_hdd0_path: PathBuf,
    pub(crate) installation_type: Rpcs3InstallationType,
    pub(crate) executable_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Rpcs3StatusOutcome {
    /// No eligible RPCS3 profile was found in any documented location.
    NotFound,
    Found {
        profile: Rpcs3FoundProfile,
        inspection: Box<Rpcs3GameInspection>,
    },
}

/// Real, read-only environment probing: resolves discovery roots from the
/// process environment, discovers documented RPCS3 profiles (no home
/// directory recursion), and - when one is eligible - inspects it for
/// `verified_title_id` (never re-derived here; see this module's own doc
/// comment). Must not be called on every frame; callers gate it behind an
/// explicit action, the same convention every other gather in this crate
/// uses.
pub(crate) fn gather_rpcs3_status(verified_title_id: Option<String>) -> Rpcs3StatusOutcome {
    let Ok(roots) = Rpcs3ProfileDiscoveryRoots::from_environment() else {
        return Rpcs3StatusOutcome::NotFound;
    };
    let discovery = discover_rpcs3_profiles(&roots);
    let Some(profile) = discovery
        .profiles
        .into_iter()
        .find(|profile| profile.eligible)
    else {
        return Rpcs3StatusOutcome::NotFound;
    };
    let request = Rpcs3GameRequest {
        verified_ps3_title_id: verified_title_id,
        emulator_game_id: None,
    };
    let inspection = inspect_rpcs3_game(&profile, &request);
    Rpcs3StatusOutcome::Found {
        profile: Rpcs3FoundProfile {
            configuration_path: profile.configuration_path,
            dev_hdd0_path: profile.dev_hdd0_path,
            installation_type: profile.installation_type,
            executable_version: profile
                .executable_candidates
                .first()
                .and_then(|executable| executable.version.clone()),
        },
        inspection: Box::new(inspection),
    }
}

// ---------------------------------------------------------------------
// State
// ---------------------------------------------------------------------

#[derive(Default)]
pub(crate) enum Rpcs3State {
    #[default]
    Idle,
    Loading {
        generation: u64,
        receiver: Receiver<(u64, Rpcs3StatusOutcome)>,
    },
    Ready {
        #[allow(dead_code)]
        generation: u64,
        outcome: Rpcs3StatusOutcome,
    },
}

pub(crate) enum Rpcs3Action {
    /// Load (or reload) RPCS3 status. The only action this panel ever
    /// asks for - there is no mutating action in this vocabulary.
    Load,
}

// ---------------------------------------------------------------------
// Label helpers
// ---------------------------------------------------------------------

fn installation_type_label(kind: Rpcs3InstallationType) -> &'static str {
    match kind {
        Rpcs3InstallationType::Native => "Native",
        Rpcs3InstallationType::FlatpakUser => "Flatpak",
        Rpcs3InstallationType::Portable => "Portable/AppImage",
        Rpcs3InstallationType::Explicit => "Custom configured path",
    }
}

fn firmware_label(status: &Rpcs3FirmwareStatus) -> (&'static str, widgets::StatusTone) {
    match status {
        Rpcs3FirmwareStatus::Present(_) => ("Ready", widgets::StatusTone::Success),
        Rpcs3FirmwareStatus::Missing => ("Missing", widgets::StatusTone::Warning),
        Rpcs3FirmwareStatus::Unknown => ("Unknown", widgets::StatusTone::Pending),
    }
}

// ---------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------

/// Draws the "RPCS3" section. `verified_title_id` must already be an
/// authoritative PS3 title ID (or `None`) - this panel never derives one.
/// Returns an action the caller should perform; drawing itself never
/// mutates anything.
pub(crate) fn show_rpcs3_panel(
    ui: &mut egui::Ui,
    advanced_mode: bool,
    verified_title_id: Option<&str>,
    state: &Rpcs3State,
) -> Option<Rpcs3Action> {
    let mut action = None;
    widgets::section_header(
        ui,
        "RPCS3",
        Some("Local PS3 emulator environment and, for the selected title, its RPCS3 assets."),
    );

    match state {
        Rpcs3State::Idle => {
            widgets::card(ui, |ui| {
                ui.label("RPCS3 status has not been checked yet.");
                if widgets::action_button(ui, "Check RPCS3", widgets::ActionStyle::Secondary, true)
                    .clicked()
                {
                    action = Some(Rpcs3Action::Load);
                }
            });
        }
        Rpcs3State::Loading { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Checking local RPCS3 installation…");
            });
        }
        Rpcs3State::Ready { outcome, .. } => {
            if widgets::action_button(ui, "Refresh", widgets::ActionStyle::Quiet, true).clicked() {
                action = Some(Rpcs3Action::Load);
            }
            show_outcome(ui, advanced_mode, verified_title_id, outcome);
        }
    }

    action
}

fn show_outcome(
    ui: &mut egui::Ui,
    advanced_mode: bool,
    verified_title_id: Option<&str>,
    outcome: &Rpcs3StatusOutcome,
) {
    let Rpcs3StatusOutcome::Found {
        profile,
        inspection,
    } = outcome
    else {
        widgets::empty_state(
            ui,
            "RPCS3 not found",
            "No RPCS3 installation was found in any documented location (native, Flatpak, or a \
             configured custom path).",
            None,
        );
        return;
    };

    widgets::card(ui, |ui| {
        widgets::status_badge(ui, "Emulator detected", widgets::StatusTone::Success);
        let (firmware_headline, firmware_tone) = firmware_label(&inspection.health.firmware);
        ui.label(format!("Firmware: {firmware_headline}"));
        let _ = firmware_tone;

        if verified_title_id.is_none() {
            ui.label(
                "No verified PS3 title ID is available for the current selection - showing \
                 RPCS3's own environment only.",
            );
        } else if inspection.base_game.is_some() || inspection.disc_game.is_some() {
            ui.label("✓ PS3 title ID matched");
            ui.label(format!(
                "Game install: {}",
                if inspection.base_game.is_some() {
                    "Found"
                } else {
                    "Found (disc-style)"
                }
            ));
            if inspection.update.detected {
                ui.label(format!(
                    "Update: v{}",
                    inspection.update.version.as_deref().unwrap_or("?")
                ));
            }
            if inspection.dlc.count > 0 {
                ui.label(format!("DLC: {} item(s)", inspection.dlc.count));
            }
            if let Some(patches) = &inspection.patches
                && !patches.entries.is_empty()
            {
                ui.label(format!(
                    "Patches: {} enabled of {} available",
                    patches.enabled_count,
                    patches.entries.len()
                ));
            }
            if inspection
                .per_game_config
                .as_ref()
                .is_some_and(|config| config.exists)
            {
                ui.label("Per-game config: Found");
            }
        } else {
            ui.label("PS3 title ID matched, but no installed game was found for it locally.");
        }

        if !advanced_mode {
            return;
        }
        widgets::technical_details(
            ui,
            ("rpcs3_technical_detail", &profile.configuration_path),
            |ui| {
                ui.label(format!(
                    "Installation kind: {}",
                    installation_type_label(profile.installation_type)
                ));
                widgets::path_value(ui, "Configuration root", &profile.configuration_path);
                widgets::path_value(ui, "dev_hdd0", &profile.dev_hdd0_path);
                if let Some(version) = &profile.executable_version {
                    widgets::copyable_value(ui, "Executable version", version);
                }
                if let Some(id) = verified_title_id {
                    widgets::copyable_value(ui, "Title ID", id);
                }
                if let Some(base) = &inspection.base_game {
                    widgets::copyable_value(
                        ui,
                        "Game path",
                        &base.install_path.display().to_string(),
                    );
                    if let Some(title) = &base.display_title {
                        widgets::copyable_value(ui, "PARAM.SFO title (display only)", title);
                    }
                }
                if let Some(disc) = &inspection.disc_game {
                    widgets::copyable_value(
                        ui,
                        "Disc game path",
                        &disc.install_path.display().to_string(),
                    );
                }
                for entry in &inspection.dlc.entries {
                    if let Some(content_id) = &entry.content_id {
                        widgets::copyable_value(ui, "DLC content ID", content_id);
                    }
                }
                if let Some(config) = &inspection.per_game_config
                    && config.exists
                {
                    widgets::path_value(ui, "Per-game config path", &config.path);
                }
                if let Some(patches) = &inspection.patches
                    && patches.exists
                {
                    widgets::path_value(ui, "Patch source", &patches.path);
                }
                if !inspection.health.warnings.is_empty() {
                    ui.label("Warnings:");
                    for warning in &inspection.health.warnings {
                        ui.add(egui::Label::new(warning.as_str()).selectable(true).wrap());
                    }
                }
            },
        );
    });
}

#[cfg(test)]
mod tests {
    use archivefs_core::patch_manager::{
        Rpcs3DlcInventory, Rpcs3GameIdMapping, Rpcs3Health, Rpcs3SaveTrophyInventory,
        Rpcs3UpdateInfo,
    };

    use super::*;

    fn empty_inspection() -> Rpcs3GameInspection {
        Rpcs3GameInspection {
            title_id: None,
            title_id_mapping: Rpcs3GameIdMapping::Unavailable,
            base_game: None,
            disc_game: None,
            update: Rpcs3UpdateInfo::default(),
            dlc: Rpcs3DlcInventory::default(),
            per_game_config: None,
            overridden_setting_keys: Vec::new(),
            patches: None,
            save_trophy: Rpcs3SaveTrophyInventory::default(),
            health: Rpcs3Health {
                detected: true,
                config_readable: true,
                dev_hdd0_readable: true,
                firmware: Rpcs3FirmwareStatus::Unknown,
                patch_data_available: false,
                title_id_mapping: Rpcs3GameIdMapping::Unavailable,
                warnings: Vec::new(),
            },
        }
    }

    fn found_outcome(inspection: Rpcs3GameInspection) -> Rpcs3StatusOutcome {
        Rpcs3StatusOutcome::Found {
            profile: Rpcs3FoundProfile {
                configuration_path: PathBuf::from("/config/rpcs3"),
                dev_hdd0_path: PathBuf::from("/config/rpcs3/dev_hdd0"),
                installation_type: Rpcs3InstallationType::Native,
                executable_version: Some("0.0.31".to_string()),
            },
            inspection: Box::new(inspection),
        }
    }

    fn rendered_text_contains(output: &egui::FullOutput, needle: &str) -> bool {
        fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
            match shape {
                egui::Shape::Text(text_shape) => text_shape.galley.text().contains(needle),
                egui::Shape::Vec(nested) => nested.iter().any(|s| shape_contains(s, needle)),
                _ => false,
            }
        }
        output
            .shapes
            .iter()
            .any(|clipped| shape_contains(&clipped.shape, needle))
    }

    fn run_panel(
        advanced_mode: bool,
        verified_title_id: Option<&str>,
        state: &Rpcs3State,
    ) -> egui::FullOutput {
        let ctx = egui::Context::default();
        let draw = |ctx: &egui::Context| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_rpcs3_panel(ui, advanced_mode, verified_title_id, state);
            });
        };
        let _ = ctx.run(egui::RawInput::default(), draw);
        ctx.run(egui::RawInput::default(), draw)
    }

    #[test]
    fn not_found_renders_an_empty_state() {
        let state = Rpcs3State::Ready {
            generation: 1,
            outcome: Rpcs3StatusOutcome::NotFound,
        };
        let output = run_panel(false, None, &state);
        assert!(rendered_text_contains(&output, "RPCS3 not found"));
    }

    #[test]
    fn unresolved_identity_shows_environment_only_never_a_game_mapping() {
        let state = Rpcs3State::Ready {
            generation: 1,
            outcome: found_outcome(empty_inspection()),
        };
        let output = run_panel(false, None, &state);
        assert!(rendered_text_contains(&output, "Emulator detected"));
        assert!(!rendered_text_contains(&output, "PS3 title ID matched"));
    }

    #[test]
    fn a_verified_title_with_no_local_install_reports_that_honestly() {
        let state = Rpcs3State::Ready {
            generation: 1,
            outcome: found_outcome(empty_inspection()),
        };
        let output = run_panel(false, Some("BLUS30000"), &state);
        assert!(rendered_text_contains(
            &output,
            "PS3 title ID matched, but no installed game was found for it locally."
        ));
    }

    #[test]
    fn gamer_mode_hides_technical_details_advanced_mode_offers_it() {
        let state = Rpcs3State::Ready {
            generation: 1,
            outcome: found_outcome(empty_inspection()),
        };
        let gamer = run_panel(false, None, &state);
        let advanced = run_panel(true, None, &state);
        assert!(!rendered_text_contains(&gamer, "Technical details"));
        assert!(rendered_text_contains(&advanced, "Technical details"));
    }
}
