//! Read-only controller configuration evidence for the selected game. Reuses
//! each adapter's existing `inspect_*_game` result; this module cannot select
//! a controller profile, change a launch request or write emulator settings.
//!
//! Core never reads individual button mappings, so nothing here lists
//! configured or missing buttons. Per-game controller requirements are not
//! modelled either (docs/research/READY_TO_PLAY_ARCHITECTURE_AUDIT.md
//! section 12), so the panel says so rather than inventing one. Core has no
//! typed launch field for a controller profile, so no picker is offered.
use super::*;
use archivefs_core::patch_manager::{
    HatariConfig, Pcsx2Config, Pcsx2GameRequest, inspect_duckstation_game_for_verified,
    inspect_pcsx2_game, inspect_ppsspp_game_for_verified,
};
use std::sync::{Arc, Mutex};

pub(crate) const REQUIREMENTS_UNKNOWN: &str = "Controller requirements for this game are not known to EmuWiz. It cannot tell whether a specific controller is needed.";
pub(crate) const SCOPE_NOTE: &str = "This is controller configuration only. Individual buttons are not checked, and it does not change launch readiness or say whether the game is compatible.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ControllerState {
    Configured,
    NotConfigured,
    Unknown,
}

impl ControllerState {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Configured => "Configured",
            Self::NotConfigured => "Not configured",
            Self::Unknown => "Unknown",
        }
    }
}

/// What one existing core inspection says about controller configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ControllerReport {
    pub(crate) emulator: &'static str,
    pub(crate) state: ControllerState,
    pub(crate) detail: String,
    pub(crate) guidance: String,
}

impl ControllerReport {
    fn new(emulator: &'static str, state: ControllerState, detail: String) -> Self {
        Self {
            emulator,
            state,
            detail,
            guidance: format!(
                "To change controls, open {emulator} and use its own controller settings, then check again."
            ),
        }
    }

    pub(crate) fn show(&self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new(format!(
                "{} controller configuration: {}",
                self.emulator,
                self.state.label()
            ))
            .strong(),
        );
        ui.label(&self.detail);
        ui.small(&self.guidance);
    }
}

/// PCSX2 records which `[Pad*]`/`[USB*]` sections exist in its main
/// settings; device and button detail is not read.
pub(super) fn pcsx2_report(config: &Pcsx2Config, sections: &[String]) -> ControllerReport {
    let (state, detail) = if !config.readable {
        (
            ControllerState::Unknown,
            "PCSX2's main settings could not be read.".to_string(),
        )
    } else if sections.is_empty() {
        (
            ControllerState::NotConfigured,
            "PCSX2's main settings contain no controller port sections.".to_string(),
        )
    } else {
        (
            ControllerState::Configured,
            format!(
                "Ports with settings: {}. Which device or buttons each port uses is not read.",
                sections.join(", ")
            ),
        )
    };
    ControllerReport::new("PCSX2", state, detail)
}

/// `global`/`per_game` are the adapter's own tri-state profile-entry flags;
/// the outer `None` on `per_game` means no per-game settings were found.
pub(super) fn profile_entry_report(
    emulator: &'static str,
    global: Option<bool>,
    per_game: Option<Option<bool>>,
) -> ControllerReport {
    let (state, scope) = match (per_game.flatten(), global) {
        (Some(set), _) => (set, "this game's own settings"),
        (None, Some(set)) => (set, "the main settings"),
        (None, None) => {
            return ControllerReport::new(
                emulator,
                ControllerState::Unknown,
                "No controller profile entry EmuWiz recognises was found. This does not mean controllers are unconfigured.".into(),
            );
        }
    };
    let (state, detail) = if state {
        (
            ControllerState::Configured,
            format!("A controller profile is named in {scope}."),
        )
    } else {
        (
            ControllerState::NotConfigured,
            format!("The controller profile entry in {scope} is empty."),
        )
    };
    ControllerReport::new(emulator, state, detail)
}

/// Hatari's raw `nJoystickMode` per port. 0/1/2 are Hatari's own disabled,
/// real-joystick and keyboard modes; anything else is shown raw.
pub(crate) fn hatari_report(config: &HatariConfig) -> ControllerReport {
    let ports: Vec<String> = config
        .input
        .joystick_ports
        .iter()
        .enumerate()
        .filter_map(|(port, mode)| {
            let mode = mode.as_deref()?;
            let name = match mode.trim() {
                "0" => "disabled".to_string(),
                "1" => "real joystick".to_string(),
                "2" => "keyboard".to_string(),
                other => format!("mode {other}"),
            };
            Some(format!("Joystick {port}: {name}"))
        })
        .collect();
    let (state, detail) = if !config.readable {
        (
            ControllerState::Unknown,
            "Hatari's settings could not be read.".to_string(),
        )
    } else if ports.is_empty() {
        (
            ControllerState::Unknown,
            "No joystick mode is set in Hatari's settings, so Hatari's defaults apply.".to_string(),
        )
    } else {
        (
            ControllerState::Configured,
            format!("{}. Button assignments are not read.", ports.join(" · ")),
        )
    };
    ControllerReport::new("Hatari", state, detail)
}

type Job = Box<dyn FnOnce() -> ControllerReport + Send>;

/// The supported emulator's inspection for exactly this candidate, built
/// from the already-discovered profile and already-verified identity.
pub(super) fn job(
    candidate: &LaunchCandidate,
    pcsx2: Option<&Pcsx2LaunchContext>,
    duckstation: Option<&DuckStationLaunchContext>,
    ppsspp: Option<&PpssppLaunchContext>,
) -> Option<(String, Job)> {
    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        return None;
    };
    match *adapter_id {
        "pcsx2" => {
            let context = pcsx2?;
            let profile = context
                .discovery
                .profiles
                .iter()
                .find(|p| &p.profile_id == profile_id)?
                .clone();
            let request = Pcsx2GameRequest {
                verified_ps2_serial: context.verified_ps2_serial.clone(),
                verified_executable_crc: None,
                emulator_serial: None,
            };
            let key = format!("{profile:?}/{request:?}");
            Some((
                key,
                Box::new(move || {
                    let inspection = inspect_pcsx2_game(&profile, &request);
                    pcsx2_report(
                        &inspection.global_config,
                        &inspection.controllers.configured_sections,
                    )
                }),
            ))
        }
        "duckstation" => {
            let context = duckstation?;
            let profile = context
                .discovery
                .profiles
                .iter()
                .find(|p| &p.profile_id == profile_id)?
                .clone();
            let serial = context.verified_ps1_serial.clone();
            let key = format!("{profile:?}/{serial:?}");
            Some((
                key,
                Box::new(move || {
                    let inspection = inspect_duckstation_game_for_verified(&profile, serial);
                    profile_entry_report(
                        "DuckStation",
                        inspection.global_config.settings.controller_profile_present,
                        inspection
                            .per_game_config
                            .map(|config| config.settings.controller_profile_present),
                    )
                }),
            ))
        }
        "ppsspp" => {
            let context = ppsspp?;
            let profile = context
                .discovery
                .profiles
                .iter()
                .find(|p| &p.profile_id == profile_id)?
                .clone();
            let disc_id = context.verified_psp_disc_id.clone();
            let key = format!("{profile:?}/{disc_id:?}");
            Some((
                key,
                Box::new(move || {
                    let inspection = inspect_ppsspp_game_for_verified(&profile, disc_id);
                    profile_entry_report(
                        "PPSSPP",
                        inspection.global_config.settings.controller_config_present,
                        inspection
                            .per_game_config
                            .map(|config| config.settings.controller_config_present),
                    )
                }),
            ))
        }
        _ => None,
    }
}

#[derive(Default)]
struct State {
    pending: Option<Receiver<ControllerReport>>,
    result: Option<Result<ControllerReport, String>>,
}

pub(super) fn show(
    ui: &mut egui::Ui,
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
    pcsx2: Option<&Pcsx2LaunchContext>,
    duckstation: Option<&DuckStationLaunchContext>,
    ppsspp: Option<&PpssppLaunchContext>,
) {
    let Some((job_key, job)) = job(candidate, pcsx2, duckstation, ppsspp) else {
        return;
    };
    // Scoped to the selected game, candidate, profile and verified identity:
    // a new selection never shows another selection's evidence.
    let key = egui::Id::new((
        "controller-readiness",
        format!(
            "{:?}/{:?}/{candidate:?}/{job_key}",
            plan.platform_id, plan.game_key
        ),
    ));
    let state = ui.ctx().data_mut(|data| {
        data.get_temp_mut_or_default::<Arc<Mutex<State>>>(key)
            .clone()
    });
    let Ok(mut state) = state.try_lock() else {
        return;
    };
    if let Some(receiver) = &state.pending {
        match receiver.try_recv() {
            Ok(report) => {
                state.result = Some(Ok(report));
                state.pending = None;
            }
            Err(TryRecvError::Disconnected) => {
                state.result = Some(Err("The controller check stopped. Check again.".into()));
                state.pending = None;
            }
            Err(TryRecvError::Empty) => {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
    }
    ui.push_id(key, |ui| {
        ui.collapsing("Controller setup", |ui| {
            ui.label(REQUIREMENTS_UNKNOWN);
            ui.small(SCOPE_NOTE);
            let busy = state.pending.is_some();
            let label = if state.result.is_some() {
                "Check again"
            } else {
                "Check controller setup"
            };
            if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                let (sender, receiver) = mpsc::channel();
                let repaint = ui.ctx().clone();
                state.result = None;
                state.pending = Some(receiver);
                thread::spawn(move || {
                    let _ = sender.send(job());
                    repaint.request_repaint();
                });
            }
            if busy {
                ui.label("Reading local settings…");
            }
            match &state.result {
                Some(Ok(report)) => report.show(ui),
                Some(Err(error)) => {
                    ui.colored_label(theme::warning(ui), error);
                }
                None => {}
            }
        });
    });
}
