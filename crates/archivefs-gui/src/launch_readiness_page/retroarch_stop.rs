//! Confirmed Stop for the RetroArch child retained by the existing launch
//! watcher. The only authority is the tracked launch's own pidfd-backed
//! handle: no PID lookup, process-name match, process-tree signal, second
//! signal or escalation lives here. Core refuses unless that handle is
//! RetroArch itself. Normal polling still owns exit, stderr, the SRAM
//! observation and cheat workspace cleanup.
use super::{RetroArchLaunchStage, RetroArchLaunchState};
use crate::ui::components::StatusTone;
use archivefs_core::launch::{
    LaunchedRetroArchProcess,
    process_spawn::{GracefulStopOutcome, GracefulStopRefusal, GracefulStopScope, OwnedExecutable},
};
use eframe::egui;
use std::time::Duration;

const EXPLANATION: &str = "RetroArch is asked to close normally, so save in the game first if it needs it. Only the RetroArch that EmuWiz started is affected.";
const NOT_THE_EMULATOR: &str = "Stop is not available for this launch: EmuWiz cannot confirm that the program it started is RetroArch itself, so stopping it might not close the game or let it save. Close RetroArch from its own window. EmuWiz will keep watching this launch.";
const STILL_CLOSING: &str =
    "Close RetroArch yourself if it stays open. EmuWiz will not force it to quit or ask again.";

#[derive(Default)]
pub(super) struct StopControl {
    confirmed: bool,
    outcome: Option<GracefulStopOutcome>,
    /// The launch's settled target, so it is not asked every frame. Core
    /// asks again at the moment of the request.
    target: Option<OwnedExecutable>,
}

impl StopControl {
    pub(super) fn outcome(&self) -> Option<GracefulStopOutcome> {
        self.outcome
    }

    fn request(&mut self, process: &mut LaunchedRetroArchProcess) {
        if !self.confirmed || self.outcome.is_some() {
            return;
        }
        self.confirmed = false;
        // ZERO sends the single scoped request and observes without sleeping
        // in the render loop.
        self.outcome =
            Some(process.request_graceful_stop(GracefulStopScope::LaunchedChild, Duration::ZERO));
    }

    fn show(&mut self, ui: &mut egui::Ui, process: &mut LaunchedRetroArchProcess) {
        let target = match self.target {
            Some(target @ (OwnedExecutable::Matches | OwnedExecutable::Differs)) => target,
            _ => *self.target.insert(process.stop_target()),
        };
        ui.push_id("owned-retroarch-stop", |ui| match self.outcome {
            None if matches!(target, OwnedExecutable::Exited) => {}
            None if !matches!(target, OwnedExecutable::Matches) => {
                ui.label(NOT_THE_EMULATOR);
            }
            None => {
                let game = game_name(process);
                ui.small(EXPLANATION);
                ui.checkbox(
                    &mut self.confirmed,
                    format!("I confirm: ask RetroArch to close {game}"),
                );
                if ui
                    .add_enabled(self.confirmed, egui::Button::new("Stop RetroArch"))
                    .clicked()
                {
                    self.request(process);
                }
            }
            Some(GracefulStopOutcome::StillRunning { .. }) => {
                ui.label("Stopping RetroArch: it has not closed yet.");
                ui.small(STILL_CLOSING);
            }
            Some(GracefulStopOutcome::AlreadyExited | GracefulStopOutcome::ExitedAfterRequest) => {
                ui.label("RetroArch has closed; waiting for its final exit report.");
            }
            Some(GracefulStopOutcome::Refused(GracefulStopRefusal::TargetNotEmulator)) => {
                ui.label(NOT_THE_EMULATOR);
            }
            Some(GracefulStopOutcome::Refused(reason)) => {
                ui.label("Stop did not reach RetroArch. Close RetroArch yourself; EmuWiz will keep watching this launch.");
                ui.small(format!("Refused: {reason:?}"));
            }
        });
    }
}

impl RetroArchLaunchState {
    /// Stop is shown for whichever launch is tracked as running, whatever
    /// game is on screen: the request can only reach that launch's own child.
    /// Afterwards this keeps the result in view until the next launch, because
    /// the game's own card may no longer be on screen. A starting, failed or
    /// unstopped launch has nothing to show.
    pub(crate) fn show_stop_control(&mut self, ui: &mut egui::Ui) {
        let (process, stop, success) = match self.tracked.as_mut() {
            Some((_, RetroArchLaunchStage::Running { process, stop })) => {
                return stop.show(ui, process);
            }
            Some((_, RetroArchLaunchStage::RunningCheats { session, stop })) => {
                if let Some(process) = session.process_mut() {
                    stop.show(ui, process);
                }
                return;
            }
            Some((_, RetroArchLaunchStage::Exited { process, stop })) => {
                let success = process
                    .poll()
                    .is_some_and(|report| matches!(&report.status, Ok(status) if status.success()));
                (&*process, &*stop, success)
            }
            Some((_, RetroArchLaunchStage::ExitedCheats { session, stop })) => {
                let success = session
                    .receipt()
                    .process
                    .as_ref()
                    .and_then(|process| process.exit.as_ref())
                    .is_some_and(|exit| exit.success);
                match session.process() {
                    Some(process) => (process, &*stop, success),
                    None => return,
                }
            }
            _ => return,
        };
        if let Some((summary, _)) = requested_exit(stop.outcome(), success) {
            ui.small(format!("{}: {summary}", game_name(process)));
        }
    }
}

fn game_name(process: &LaunchedRetroArchProcess) -> String {
    let path = &process.command_facts.content_path;
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
pub(crate) fn retroarch_test_request(path: &str) -> archivefs_core::launch::RetroArchLaunchRequest {
    super::tests::request_for(path)
}

#[cfg(test)]
impl RetroArchLaunchState {
    /// Tracks a real fixture child as `request`'s running launch.
    pub(crate) fn track_running_for_test(
        &mut self,
        request: &archivefs_core::launch::RetroArchLaunchRequest,
    ) {
        self.tracked = Some((
            super::RetroArchLaunchKey::from_request(request),
            RetroArchLaunchStage::running(super::tests::spawn_test_process("/bin/sleep", &["30"])),
        ));
    }

    /// Confirms and requests Stop for the tracked launch, without a click.
    pub(crate) fn stop_for_test(&mut self) -> Option<GracefulStopOutcome> {
        let (process, stop) = match self.tracked.as_mut()? {
            (_, RetroArchLaunchStage::Running { process, stop }) => (process, stop),
            (_, RetroArchLaunchStage::RunningCheats { session, stop }) => {
                (session.process_mut()?, stop)
            }
            _ => return None,
        };
        stop.confirmed = true;
        stop.request(process);
        stop.outcome()
    }
}

/// The result banner for a launch that ended after the player's own request,
/// or `None` when no request reached it and the ordinary wording applies.
pub(super) fn requested_exit(
    outcome: Option<GracefulStopOutcome>,
    success: bool,
) -> Option<(&'static str, StatusTone)> {
    if !matches!(
        outcome,
        Some(GracefulStopOutcome::StillRunning { .. } | GracefulStopOutcome::ExitedAfterRequest)
    ) {
        return None;
    }
    Some(if success {
        (
            "RetroArch closed normally after you asked it to stop.",
            StatusTone::Success,
        )
    } else {
        (
            "RetroArch ended after you asked it to stop but did not close normally. Recent in-game progress may not have been saved. If its window is still open, close it yourself.",
            StatusTone::Warning,
        )
    })
}

#[cfg(all(test, target_os = "linux"))]
#[path = "retroarch_stop/tests.rs"]
mod tests;
