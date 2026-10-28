//! Confirmed Stop for the Cemu child retained by the existing launch watcher.
use super::{StandaloneLaunchStage, StandaloneLaunchState, StandaloneProcess};
use archivefs_core::launch::{
    cemu_execution::LaunchedCemuProcess,
    process_spawn::{GracefulStopOutcome, GracefulStopScope, ProcessExitReport},
};
use eframe::egui;
use std::time::Duration;

pub(crate) struct Session {
    process: LaunchedCemuProcess,
    confirmed: bool,
    outcome: Option<GracefulStopOutcome>,
}

impl Session {
    pub(super) fn new(process: LaunchedCemuProcess) -> Self {
        Self {
            process,
            confirmed: false,
            outcome: None,
        }
    }

    pub(super) fn poll(&mut self) -> Option<&ProcessExitReport> {
        self.process.poll()
    }

    fn request(&mut self) {
        if !self.confirmed || self.outcome.is_some() {
            return;
        }
        self.confirmed = false;
        // No sleep in the render loop and no escalation. The existing watcher
        // continues to drain stderr and report the actual child exit.
        self.outcome = Some(
            self.process
                .request_graceful_stop(GracefulStopScope::LaunchedChild, Duration::ZERO),
        );
    }

    fn show(&mut self, ui: &mut egui::Ui) {
        ui.push_id("owned-cemu-lifecycle", |ui| {
            ui.strong("Cemu process");
            ui.label(format!("Launched content: {}", self.process.command_facts.content_path.display()));
            let outcome = self.outcome;
            if let Some(report) = self.process.poll() {
                ui.label(exit_message(report, outcome));
                if !report.stderr.is_empty() {
                    crate::ui::components::technical_details(ui, "cemu-exit-stderr", |ui| {
                        let text = String::from_utf8_lossy(&report.stderr);
                        ui.add(egui::Label::new(egui::RichText::new(text).monospace()).wrap());
                    });
                }
                ui.small("Process exit does not prove gameplay compatibility or that saves completed.");
                return;
            }
            match outcome {
                Some(GracefulStopOutcome::StillRunning { .. }) => {
                    ui.label("Stopping Cemu — exit has not been observed.");
                    ui.small("Close Cemu normally if needed. EmuWiz will not force-kill it or signal it again.");
                }
                Some(GracefulStopOutcome::AlreadyExited | GracefulStopOutcome::ExitedAfterRequest) => {
                    ui.label("Cemu exit observed; waiting for its final process report.");
                }
                Some(GracefulStopOutcome::Refused(reason)) => {
                    ui.label("Running · Stop failed: ownership or exit could not be safely established.");
                    ui.label(format!("Stop refused: {reason:?}"));
                    ui.small("Close Cemu normally. EmuWiz will keep watching this launch.");
                }
                None => {
                    ui.label("Running");
                    ui.small("Stop requests normal termination of only this EmuWiz-owned Cemu process. It does not stop other Cemu instances or descendants.");
                    ui.checkbox(&mut self.confirmed, "I confirm: request graceful Stop for this Cemu process");
                    if ui.add_enabled(self.confirmed, egui::Button::new("Stop Cemu")).clicked() {
                        self.request();
                    }
                }
            }
        });
    }
}

fn exit_message(report: &ProcessExitReport, outcome: Option<GracefulStopOutcome>) -> String {
    match &report.status {
        Ok(status) if status.success() => "Exited · Cemu process exited normally.".into(),
        #[cfg(unix)]
        Ok(status)
            if {
                use std::os::unix::process::ExitStatusExt;
                status.signal() == Some(15)
                    && matches!(
                        outcome,
                        Some(
                            GracefulStopOutcome::StillRunning { .. }
                                | GracefulStopOutcome::ExitedAfterRequest
                        )
                    )
            } =>
        {
            "Exited · Cemu process terminated after the graceful Stop request.".into()
        }
        Ok(status) => format!("Failed · Cemu process exited with {status}."),
        Err(error) => format!("Failed · Cemu process exit could not be observed: {error}"),
    }
}

impl StandaloneLaunchState {
    #[cfg(test)]
    pub(crate) fn track_test_cemu(
        &mut self,
        path: std::path::PathBuf,
        process: LaunchedCemuProcess,
    ) {
        self.tracked = Some((
            path,
            "Cemu".into(),
            StandaloneLaunchStage::Running(StandaloneProcess::Cemu(Session::new(process))),
        ));
    }

    /// Lifecycle ownership does not depend on a still-valid readiness preview
    /// or on the game currently selected in the library.
    pub(crate) fn show_cemu_lifecycle(&mut self, ui: &mut egui::Ui) {
        if let Some((
            _,
            _,
            StandaloneLaunchStage::Running(StandaloneProcess::Cemu(session))
            | StandaloneLaunchStage::Exited(StandaloneProcess::Cemu(session)),
        )) = self.tracked.as_mut()
        {
            session.show(ui);
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "cemu_stop/tests.rs"]
mod tests;
