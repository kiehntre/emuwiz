//! Retained watched-process feedback survives clearing a reviewed selection.
use super::{StandaloneLaunchStage, StandaloneLaunchState, StandaloneProcess};
use eframe::egui;
use std::path::Path;

/// Does not need a launch request, and cannot start or replace a process.
/// Adapter code supplies only its existing wrapper's PID/exit description.
pub(super) fn show(
    ui: &mut egui::Ui,
    state: &mut StandaloneLaunchState,
    path: Option<&Path>,
    adapter: &str,
    label: &str,
    describe: impl Fn(&mut StandaloneProcess) -> Option<(u32, Option<String>)>,
) {
    let same = state
        .tracked
        .as_ref()
        .is_some_and(|(p, a, _)| Some(p.as_path()) == path && a == adapter);
    if !same {
        if state.is_active() {
            ui.label("Another standalone launch is active; wait before starting this emulator.");
        }
        return;
    }
    match &mut state.tracked.as_mut().unwrap().2 {
        StandaloneLaunchStage::Starting(_) => {
            ui.label(format!("Launching {label}…"));
        }
        StandaloneLaunchStage::Running(process) => {
            if let Some((pid, _)) = describe(process) {
                ui.label(format!("{label} running · PID {pid}"));
            }
            ui.small("Starting the process does not prove the game ran.");
        }
        StandaloneLaunchStage::Exited(process) => {
            ui.label("Process exited");
            if let Some((_, Some(line))) = describe(process) {
                ui.label(line);
            }
            ui.small(
                "Historical result. Check the current profile and media before another launch.",
            );
        }
        StandaloneLaunchStage::Failed(detail) => {
            ui.label("Launch failed");
            ui.label(detail.as_str());
        }
    }
}
