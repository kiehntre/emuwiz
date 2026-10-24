//! Read-only controller settings evidence for the selected game. Reuses each
//! adapter's existing inspection result; this module cannot select a
//! controller profile, change a launch request or write emulator settings.
//!
//! Four facts stay separate: whether controller settings exist, which buttons
//! are bound, which hardware is connected, and whether the game is
//! compatible. Core only ever supplies the first, so nothing here claims a
//! controller is configured, lists missing buttons or offers a profile
//! picker (core has no typed launch field for one). Per-game controller
//! requirements are not modelled (docs/research/READY_TO_PLAY_ARCHITECTURE_AUDIT.md
//! section 12).
use super::*;
use archivefs_core::patch_manager::{
    HatariConfig, Pcsx2Config, Pcsx2GameRequest, inspect_pcsx2_game,
};
use std::sync::{Arc, Mutex};

pub(crate) const REQUIREMENTS_UNKNOWN: &str = "Controller requirements for this game are not known to EmuWiz. It cannot tell whether a specific controller is needed.";
pub(crate) const SCOPE_NOTE: &str = "Only shows whether controller settings exist. Button bindings, connected hardware and game compatibility are not checked, and this does not change launch readiness.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ControllerState {
    /// Settings exist; nothing says the bindings work.
    SettingsFound,
    /// Settings were read and every port EmuWiz could read is disabled or
    /// absent.
    NoActiveSettings,
    Unknown,
}

impl ControllerState {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::SettingsFound => "Controller settings found · bindings not verified",
            Self::NoActiveSettings => "No active controller settings found",
            Self::Unknown => "Unknown",
        }
    }
}

/// What one existing core inspection says about controller settings.
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
                "{} controllers: {}",
                self.emulator,
                self.state.label()
            ))
            .strong(),
        );
        ui.label(&self.detail);
        ui.small(&self.guidance);
    }
}

/// `Pad1`..`PadN` / `USB1`..`USBN` are ports; the bare `[Pad]` section holds
/// global pad options and is not one.
fn is_pcsx2_port(section: &str) -> bool {
    let lower = section.to_ascii_lowercase();
    ["pad", "usb"].iter().any(|prefix| {
        lower
            .strip_prefix(prefix)
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// PCSX2 writes default port sections itself, so a section is never read as
/// a configured controller. A port's `Type` is used only when core retained
/// it (its unknown-setting map is bounded, so large files often drop it);
/// `Type = None` is a disabled port.
pub(super) fn pcsx2_report(config: &Pcsx2Config, sections: &[String]) -> ControllerReport {
    if !config.readable {
        return ControllerReport::new(
            "PCSX2",
            ControllerState::Unknown,
            "PCSX2's main settings could not be read.".into(),
        );
    }
    let ports: Vec<(&str, Option<&str>)> = sections
        .iter()
        .filter(|section| is_pcsx2_port(section))
        .map(|section| {
            let kind = config
                .settings
                .unknown
                .get(&format!("{section}/Type"))
                .map(String::as_str);
            (section.as_str(), kind)
        })
        .collect();
    if ports.is_empty() {
        return ControllerReport::new(
            "PCSX2",
            ControllerState::NoActiveSettings,
            "PCSX2's main settings contain no controller port sections.".into(),
        );
    }
    let describe = |(port, kind): &(&str, Option<&str>)| match kind {
        Some(kind) if kind.eq_ignore_ascii_case("none") => format!("{port}: disabled"),
        Some(kind) => format!("{port}: {kind}"),
        None => format!("{port}: type not read"),
    };
    let all_disabled = ports
        .iter()
        .all(|(_, kind)| kind.is_some_and(|kind| kind.eq_ignore_ascii_case("none")));
    let list = ports.iter().map(describe).collect::<Vec<_>>().join(" · ");
    if all_disabled {
        return ControllerReport::new(
            "PCSX2",
            ControllerState::NoActiveSettings,
            format!("Every controller port is disabled (Type = None). {list}."),
        );
    }
    ControllerReport::new(
        "PCSX2",
        ControllerState::SettingsFound,
        format!(
            "{list}. PCSX2 writes default port settings itself, so this does not mean a controller was set up or that its buttons work."
        ),
    )
}

/// Core's DuckStation and PPSSPP readers look for setting names that real
/// configuration files do not use (DuckStation keeps controls under
/// `[Pad1]`..; PPSSPP keeps them in a separate `controls.ini` core never
/// reads). Their flags are therefore never shown as evidence.
pub(super) fn unread_report(emulator: &'static str) -> ControllerReport {
    ControllerReport::new(
        emulator,
        ControllerState::Unknown,
        format!("EmuWiz does not read {emulator}'s controller settings yet."),
    )
}

/// Hatari's raw `nJoystickMode` for the two ports core reads. 0/1/2 are
/// Hatari's own disabled, real-joystick and keyboard modes; anything else
/// is shown raw.
pub(crate) fn hatari_report(config: &HatariConfig) -> ControllerReport {
    if !config.readable {
        return ControllerReport::new(
            "Hatari",
            ControllerState::Unknown,
            "Hatari's settings could not be read.".into(),
        );
    }
    let modes: Vec<(usize, &str)> = config
        .input
        .joystick_ports
        .iter()
        .enumerate()
        .filter_map(|(port, mode)| Some((port, mode.as_deref()?.trim())))
        .collect();
    if modes.is_empty() {
        return ControllerReport::new(
            "Hatari",
            ControllerState::Unknown,
            "No joystick mode is set for ports 0 and 1 in Hatari's settings.".into(),
        );
    }
    let list = modes
        .iter()
        .map(|(port, mode)| {
            let name = match *mode {
                "0" => "disabled".to_string(),
                "1" => "real joystick".to_string(),
                "2" => "keyboard".to_string(),
                other => format!("mode {other}"),
            };
            format!("Joystick {port}: {name}")
        })
        .collect::<Vec<_>>()
        .join(" · ");
    let state = if modes.iter().all(|(_, mode)| *mode == "0") {
        ControllerState::NoActiveSettings
    } else {
        ControllerState::SettingsFound
    };
    ControllerReport::new(
        "Hatari",
        state,
        format!("{list}. Only ports 0 and 1 are read; button assignments are not."),
    )
}

type Job = Box<dyn FnOnce() -> ControllerReport + Send>;

/// PCSX2's existing inspection for exactly this candidate, built from the
/// already-discovered profile and already-verified identity.
pub(super) fn pcsx2_job(
    candidate: &LaunchCandidate,
    pcsx2: Option<&Pcsx2LaunchContext>,
) -> Option<(String, Job)> {
    let LaunchTarget::Standalone {
        adapter_id: "pcsx2",
        profile_id,
        ..
    } = &candidate.target
    else {
        return None;
    };
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

/// Emulators whose controller settings EmuWiz cannot read honestly.
pub(super) fn unread_emulator(candidate: &LaunchCandidate) -> Option<&'static str> {
    match &candidate.target {
        LaunchTarget::Standalone {
            adapter_id: "duckstation",
            ..
        } => Some("DuckStation"),
        LaunchTarget::Standalone {
            adapter_id: "ppsspp",
            ..
        } => Some("PPSSPP"),
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
) {
    if let Some(emulator) = unread_emulator(candidate) {
        ui.push_id(("controller-readiness", emulator), |ui| {
            ui.collapsing("Controller setup", |ui| {
                ui.label(REQUIREMENTS_UNKNOWN);
                ui.small(SCOPE_NOTE);
                unread_report(emulator).show(ui);
            });
        });
        return;
    }
    let Some((job_key, job)) = pcsx2_job(candidate, pcsx2) else {
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
                "Check controller settings"
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
