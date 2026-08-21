//! PCSX2 GUI Integration Batch H2: a read-only "PCSX2" section - environment
//! health plus, only when the caller already has an authoritative verified
//! PS2 serial (and, separately, an authoritative verified executable CRC),
//! a mapping of local PCSX2 per-game config/patch/cheat/texture/memory-card/
//! save-state state for the selected title.
//!
//! # PCSX2 is not an identity authority
//!
//! This module never resolves or guesses a PS2 serial or executable CRC
//! itself. It only ever accepts them, already verified, from its caller
//! (see [`gather_pcsx2_status`]'s `verified_ps2_serial`/
//! `verified_executable_crc` parameters) and passes them straight through
//! to
//! [`archivefs_core::patch_manager::Pcsx2GameRequest`], the exact same
//! identity-safety boundary
//! [`archivefs_core::patch_manager::pcsx2_local`] itself enforces core-side
//! (see that module's own doc comment). When no verified serial is
//! available this panel still shows PCSX2's own local environment health
//! (detected, config readable, BIOS), but never claims a title-specific
//! mapping - no per-game config, patch/cheat state, texture pack, memory
//! card, or save-state ownership is ever shown for an unresolved,
//! ambiguous, or conflicting selection.
//!
//! # No mutation
//!
//! This module never starts PCSX2, edits a config file, enables a patch or
//! cheat, installs a texture pack, or touches a memory card/save state.
//! Loading is always an explicit action, off the UI thread, following the
//! same generation-guarded background-load convention this crate already
//! uses elsewhere (`thread::spawn` + `mpsc::channel` + a stale-result
//! generation check) - see the RPCS3 panel (`rpcs3_page.rs`) for the
//! identical pattern this module mirrors.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use archivefs_core::patch_manager::{
    Pcsx2BiosVerification, Pcsx2GameInspection, Pcsx2GameRequest, Pcsx2InstallationType,
    Pcsx2MemcardKind, Pcsx2ProfileDiscoveryRoots, Pcsx2SerialMapping, discover_pcsx2_profiles,
    inspect_pcsx2_game,
};
use eframe::egui;

use crate::ui::components as widgets;

// ---------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pcsx2FoundProfile {
    pub(crate) configuration_path: PathBuf,
    pub(crate) installation_type: Pcsx2InstallationType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Pcsx2StatusOutcome {
    /// No eligible PCSX2 profile was found in any documented location.
    NotFound,
    Found {
        profile: Pcsx2FoundProfile,
        inspection: Box<Pcsx2GameInspection>,
    },
}

/// Real, read-only environment probing: resolves discovery roots from the
/// process environment, discovers documented PCSX2 profiles (no home
/// directory recursion), and - when one is eligible - inspects it for the
/// supplied verified identity (never re-derived here; see this module's
/// own doc comment). Must not be called on every frame; callers gate it
/// behind an explicit action, the same convention every other gather in
/// this crate uses.
pub(crate) fn gather_pcsx2_status(
    verified_ps2_serial: Option<String>,
    verified_executable_crc: Option<String>,
) -> Pcsx2StatusOutcome {
    let Ok(roots) = Pcsx2ProfileDiscoveryRoots::from_environment() else {
        return Pcsx2StatusOutcome::NotFound;
    };
    let Ok(discovery) = discover_pcsx2_profiles(&roots) else {
        return Pcsx2StatusOutcome::NotFound;
    };
    let Some(profile) = discovery
        .profiles
        .into_iter()
        .find(|profile| profile.eligible)
    else {
        return Pcsx2StatusOutcome::NotFound;
    };
    let request = Pcsx2GameRequest {
        verified_ps2_serial,
        verified_executable_crc,
        emulator_serial: None,
    };
    let inspection = inspect_pcsx2_game(&profile, &request);
    Pcsx2StatusOutcome::Found {
        profile: Pcsx2FoundProfile {
            configuration_path: profile.configuration_path,
            installation_type: profile.installation_type,
        },
        inspection: Box::new(inspection),
    }
}

// ---------------------------------------------------------------------
// State
// ---------------------------------------------------------------------

#[derive(Default)]
pub(crate) enum Pcsx2StatusState {
    #[default]
    Idle,
    Loading {
        generation: u64,
        receiver: Receiver<(u64, Pcsx2StatusOutcome)>,
    },
    Ready {
        #[allow(dead_code)]
        generation: u64,
        outcome: Pcsx2StatusOutcome,
    },
}

pub(crate) enum Pcsx2StatusAction {
    /// Load (or reload) PCSX2 status. The only action this panel ever
    /// asks for - there is no mutating action in this vocabulary.
    Load,
}

// ---------------------------------------------------------------------
// Label helpers
// ---------------------------------------------------------------------

fn installation_type_label(kind: Pcsx2InstallationType) -> &'static str {
    match kind {
        Pcsx2InstallationType::Native => "Native",
        Pcsx2InstallationType::NativeAlternate => "Native (alternate data location)",
        Pcsx2InstallationType::FlatpakUser => "Flatpak (user)",
        Pcsx2InstallationType::FlatpakSystem => "Flatpak (system)",
        Pcsx2InstallationType::Portable => "Portable/AppImage",
    }
}

fn bios_label(status: Pcsx2BiosVerification) -> (&'static str, widgets::StatusTone) {
    match status {
        Pcsx2BiosVerification::Verified => ("Ready", widgets::StatusTone::Success),
        Pcsx2BiosVerification::PresentUnverified => {
            ("Present (unverified)", widgets::StatusTone::Pending)
        }
        Pcsx2BiosVerification::Missing => ("Missing", widgets::StatusTone::Warning),
        Pcsx2BiosVerification::Unreadable => ("Unreadable", widgets::StatusTone::Warning),
    }
}

// ---------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------

/// Draws the "PCSX2" section. `verified_ps2_serial`/`verified_executable_crc`
/// must already be authoritative (or `None`) - this panel never derives
/// either. Returns an action the caller should perform; drawing itself
/// never mutates anything.
pub(crate) fn show_pcsx2_panel(
    ui: &mut egui::Ui,
    advanced_mode: bool,
    verified_ps2_serial: Option<&str>,
    state: &Pcsx2StatusState,
) -> Option<Pcsx2StatusAction> {
    let mut action = None;
    widgets::section_header(
        ui,
        "PCSX2",
        Some("Local PS2 emulator environment and, for the selected title, its PCSX2 assets."),
    );

    match state {
        Pcsx2StatusState::Idle => {
            widgets::card(ui, |ui| {
                ui.label("PCSX2 status has not been checked yet.");
                if widgets::action_button(ui, "Check PCSX2", widgets::ActionStyle::Secondary, true)
                    .clicked()
                {
                    action = Some(Pcsx2StatusAction::Load);
                }
            });
        }
        Pcsx2StatusState::Loading { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Checking local PCSX2 installation…");
            });
        }
        Pcsx2StatusState::Ready { outcome, .. } => {
            if widgets::action_button(ui, "Refresh", widgets::ActionStyle::Quiet, true).clicked() {
                action = Some(Pcsx2StatusAction::Load);
            }
            show_outcome(ui, advanced_mode, verified_ps2_serial, outcome);
        }
    }

    action
}

fn show_outcome(
    ui: &mut egui::Ui,
    advanced_mode: bool,
    verified_ps2_serial: Option<&str>,
    outcome: &Pcsx2StatusOutcome,
) {
    let Pcsx2StatusOutcome::Found {
        profile,
        inspection,
    } = outcome
    else {
        widgets::empty_state(
            ui,
            "PCSX2 not found",
            "No PCSX2 installation was found in any documented location (native, Flatpak, or a \
             configured custom path).",
            None,
        );
        return;
    };

    widgets::card(ui, |ui| {
        widgets::status_badge(ui, "Emulator detected", widgets::StatusTone::Success);
        let (bios_headline, bios_tone) = bios_label(inspection.bios.verification);
        ui.label(format!("BIOS: {bios_headline}"));
        let _ = bios_tone;

        if verified_ps2_serial.is_none() {
            ui.label(
                "No verified PS2 serial is available for the current selection - showing \
                 PCSX2's own environment only.",
            );
        } else if inspection.serial_mapping == Pcsx2SerialMapping::VerifiedPs2Serial {
            ui.label("✓ PS2 serial matched");
            if let Some(config) = &inspection.per_game_config {
                ui.label(format!(
                    "Per-game config: {}",
                    if config.exists { "Found" } else { "Not found" }
                ));
            }
            if let Some(patches) = &inspection.patches
                && !patches.files.is_empty()
            {
                ui.label(format!(
                    "Patches/cheats: {} file(s) found",
                    patches.files.len()
                ));
            }
            if let Some(textures) = &inspection.textures
                && textures.present
            {
                ui.label(format!("Texture pack: {} file(s)", textures.file_count));
            }
            if inspection
                .memcards
                .iter()
                .any(|card| card.kind == Pcsx2MemcardKind::PerGameFolder && card.present)
            {
                ui.label("Memory card: per-game folder found");
            } else if inspection
                .memcards
                .iter()
                .any(|card| card.kind == Pcsx2MemcardKind::Shared && card.present)
            {
                ui.label("Memory card: shared card present");
            }
            if inspection.savestates.matched_count > 0 {
                ui.label(format!(
                    "Save states: {} found",
                    inspection.savestates.matched_count
                ));
            }
            if inspection.controllers.profile_configured {
                ui.label("Controller config: configured");
            }
        } else {
            ui.label("PS2 serial matched, but no title-specific PCSX2 data was found locally.");
        }

        if !advanced_mode {
            return;
        }
        widgets::technical_details(
            ui,
            ("pcsx2_technical_detail", &profile.configuration_path),
            |ui| {
                ui.label(format!(
                    "Installation kind: {}",
                    installation_type_label(profile.installation_type)
                ));
                widgets::path_value(ui, "Configuration root", &profile.configuration_path);
                if let Some(id) = verified_ps2_serial {
                    widgets::copyable_value(ui, "Verified PS2 serial", id);
                }
                if let Some(serial) = &inspection.serial {
                    widgets::copyable_value(ui, "PCSX2 serial mapping input", serial);
                }
                ui.label(format!("Serial mapping: {:?}", inspection.serial_mapping));
                if let Some(bios_path) = &inspection.bios.path {
                    widgets::path_value(ui, "BIOS path", bios_path);
                }
                if let Some(config) = &inspection.per_game_config
                    && config.exists
                {
                    widgets::path_value(ui, "Per-game config path", &config.path);
                }
                if let Some(patches) = &inspection.patches {
                    ui.label(format!("Patch files inspected: {}", patches.files.len()));
                }
                if let Some(patch_match) = &inspection.patch_match {
                    ui.label(format!("Patch match state: {:?}", patch_match.state));
                }
                if let Some(textures) = &inspection.textures
                    && textures.present
                {
                    widgets::path_value(ui, "Texture pack path", &textures.path);
                }
                for card in &inspection.memcards {
                    if card.present {
                        widgets::path_value(ui, "Memory card path", &card.path);
                    }
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
        Pcsx2BiosInfo, Pcsx2Config, Pcsx2ControllerInfo, Pcsx2Health, Pcsx2SaveStateInventory,
    };

    use super::*;

    fn empty_inspection() -> Pcsx2GameInspection {
        Pcsx2GameInspection {
            serial: None,
            serial_mapping: Pcsx2SerialMapping::Unavailable,
            global_config: Pcsx2Config {
                path: PathBuf::from("/config/pcsx2/inis/PCSX2.ini"),
                exists: true,
                readable: true,
                settings: Default::default(),
                warnings: Vec::new(),
            },
            per_game_config: None,
            overridden_setting_keys: Vec::new(),
            patches: None,
            patch_match: None,
            textures: None,
            memcards: Vec::new(),
            savestates: Pcsx2SaveStateInventory::default(),
            controllers: Pcsx2ControllerInfo::default(),
            bios: Pcsx2BiosInfo {
                path: None,
                verification: Pcsx2BiosVerification::Missing,
                filename_hint: None,
                warnings: Vec::new(),
            },
            health: Pcsx2Health {
                detected: true,
                config_readable: true,
                bios: Pcsx2BiosVerification::Missing,
                patch_data_available: false,
                serial_mapping: Pcsx2SerialMapping::Unavailable,
                warnings: Vec::new(),
            },
        }
    }

    fn found_outcome(inspection: Pcsx2GameInspection) -> Pcsx2StatusOutcome {
        Pcsx2StatusOutcome::Found {
            profile: Pcsx2FoundProfile {
                configuration_path: PathBuf::from("/config/pcsx2"),
                installation_type: Pcsx2InstallationType::Native,
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
        verified_ps2_serial: Option<&str>,
        state: &Pcsx2StatusState,
    ) -> egui::FullOutput {
        let ctx = egui::Context::default();
        let draw = |ctx: &egui::Context| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_pcsx2_panel(ui, advanced_mode, verified_ps2_serial, state);
            });
        };
        let _ = ctx.run(egui::RawInput::default(), draw);
        ctx.run(egui::RawInput::default(), draw)
    }

    #[test]
    fn not_found_renders_an_empty_state() {
        let state = Pcsx2StatusState::Ready {
            generation: 1,
            outcome: Pcsx2StatusOutcome::NotFound,
        };
        let output = run_panel(false, None, &state);
        assert!(rendered_text_contains(&output, "PCSX2 not found"));
    }

    #[test]
    fn unresolved_identity_shows_environment_only_never_a_game_mapping() {
        let state = Pcsx2StatusState::Ready {
            generation: 1,
            outcome: found_outcome(empty_inspection()),
        };
        let output = run_panel(false, None, &state);
        assert!(rendered_text_contains(&output, "Emulator detected"));
        assert!(!rendered_text_contains(&output, "PS2 serial matched"));
    }

    #[test]
    fn ambiguous_or_conflicting_identity_never_produces_title_specific_mapping() {
        // Even with an inspection that *would* show title-specific data, a
        // caller must never pass a verified serial derived from anything
        // but a genuinely verified identity - this test represents the
        // Ambiguous/Conflict/Unknown case, where the caller correctly
        // passes `None` regardless of what PCSX2 itself has on disk.
        let mut inspection = empty_inspection();
        inspection.serial = Some("SLUS-20312".to_string());
        inspection.serial_mapping = Pcsx2SerialMapping::EmulatorMetadataOnly;
        let state = Pcsx2StatusState::Ready {
            generation: 1,
            outcome: found_outcome(inspection),
        };
        let output = run_panel(false, None, &state);
        assert!(!rendered_text_contains(&output, "PS2 serial matched"));
    }

    #[test]
    fn a_verified_serial_with_no_local_data_reports_that_honestly() {
        let state = Pcsx2StatusState::Ready {
            generation: 1,
            outcome: found_outcome(empty_inspection()),
        };
        let output = run_panel(false, Some("SLUS-20312"), &state);
        assert!(rendered_text_contains(
            &output,
            "PS2 serial matched, but no title-specific PCSX2 data was found locally."
        ));
    }

    #[test]
    fn bios_missing_state_is_shown() {
        let state = Pcsx2StatusState::Ready {
            generation: 1,
            outcome: found_outcome(empty_inspection()),
        };
        let output = run_panel(false, None, &state);
        assert!(rendered_text_contains(&output, "BIOS: Missing"));
    }

    #[test]
    fn per_game_config_found_is_shown_for_a_verified_match() {
        let mut inspection = empty_inspection();
        inspection.serial = Some("SLUS-20312".to_string());
        inspection.serial_mapping = Pcsx2SerialMapping::VerifiedPs2Serial;
        inspection.per_game_config = Some(Pcsx2Config {
            path: PathBuf::from("/config/pcsx2/inis/gamesettings/SLUS-20312.ini"),
            exists: true,
            readable: true,
            settings: Default::default(),
            warnings: Vec::new(),
        });
        let state = Pcsx2StatusState::Ready {
            generation: 1,
            outcome: found_outcome(inspection),
        };
        let output = run_panel(false, Some("SLUS-20312"), &state);
        assert!(rendered_text_contains(&output, "Per-game config: Found"));
    }

    #[test]
    fn gamer_mode_hides_technical_details_advanced_mode_offers_it() {
        let state = Pcsx2StatusState::Ready {
            generation: 1,
            outcome: found_outcome(empty_inspection()),
        };
        let gamer = run_panel(false, None, &state);
        let advanced = run_panel(true, None, &state);
        assert!(!rendered_text_contains(&gamer, "Technical details"));
        assert!(rendered_text_contains(&advanced, "Technical details"));
    }
}
