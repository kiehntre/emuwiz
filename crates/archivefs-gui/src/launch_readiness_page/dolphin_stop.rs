//! Explicit launcher-only shutdown over the existing owned-process API.
//! No PID signalling, process-tree authority or lifecycle replacement lives here.
use archivefs_core::launch::{
    dolphin_execution::LaunchedDolphinProcess,
    process_spawn::{GracefulStopOutcome, GracefulStopScope},
};
use eframe::egui;
use std::time::Duration;

#[derive(Default)]
pub(super) struct StopControl {
    confirmed: bool,
    outcome: Option<GracefulStopOutcome>,
}

impl StopControl {
    fn request(&mut self, process: &mut LaunchedDolphinProcess) {
        if !self.confirmed || self.outcome.is_some() {
            return;
        }
        self.confirmed = false;
        // ZERO sends the existing single scoped request and observes without
        // sleeping in the render loop. Normal polling still owns exit/cleanup.
        self.outcome =
            Some(process.request_graceful_stop(GracefulStopScope::LaunchedChild, Duration::ZERO));
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui, process: &mut LaunchedDolphinProcess) {
        ui.label("Normal stop targets only the owned launcher. Dolphin may ask to save or confirm exit; a Flatpak launcher exiting does not prove Dolphin and its descendants have closed.");
        if let Some(outcome) = self.outcome {
            ui.label(message(outcome));
            if let GracefulStopOutcome::Refused(reason) = outcome {
                ui.label(format!("Refusal detail: {reason:?}"));
            }
            return;
        }
        ui.checkbox(
            &mut self.confirmed,
            "I want to request normal shutdown of this launcher",
        );
        if ui
            .add_enabled(self.confirmed, egui::Button::new("Request normal stop"))
            .clicked()
        {
            self.request(process);
        }
    }
}

fn message(outcome: GracefulStopOutcome) -> &'static str {
    match outcome {
        GracefulStopOutcome::AlreadyExited => {
            "The owned launcher already exited. Waiting for its final exit report."
        }
        GracefulStopOutcome::ExitedAfterRequest => {
            "The owned launcher exited after the request. Waiting for its final exit report."
        }
        GracefulStopOutcome::StillRunning { .. } => {
            "Shutdown requested; launcher exit has not been observed. Close Dolphin normally if needed. EmuWiz will keep watching for the exit report."
        }
        GracefulStopOutcome::Refused(_) => {
            "Normal stop was refused. Close Dolphin normally; EmuWiz will keep watching for the exit report."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_outcomes_describe_the_launcher_and_preserve_uncertainty() {
        use archivefs_core::launch::process_spawn::GracefulStopRefusal;
        assert!(message(GracefulStopOutcome::AlreadyExited).contains("launcher"));
        assert!(message(GracefulStopOutcome::ExitedAfterRequest).contains("final exit report"));
        assert!(
            message(GracefulStopOutcome::StillRunning {
                request_already_pending: false
            })
            .contains("exit has not been observed")
        );
        assert!(
            message(GracefulStopOutcome::Refused(
                GracefulStopRefusal::ProcessTreeNotSupervised
            ))
            .contains("refused")
        );
    }
}

#[cfg(all(test, target_os = "linux"))]
mod process_tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn confirmation_is_required_and_one_request_keeps_the_existing_exit_watcher() {
        let fixture = tempfile::tempdir().unwrap();
        let content = fixture.path().join("source.iso");
        std::fs::write(&content, b"preserved game bytes").unwrap();
        let mut process = super::super::tests::spawn_test_dolphin_process(
            "/bin/sleep",
            &["30"],
            &content,
            "private-test-profile",
        );
        let mut control = StopControl::default();
        control.request(&mut process);
        assert!(control.outcome.is_none());
        assert!(process.poll().is_none());
        control.confirmed = true;
        control.request(&mut process);
        assert!(!control.confirmed);
        assert!(matches!(
            control.outcome,
            Some(
                GracefulStopOutcome::StillRunning { .. } | GracefulStopOutcome::ExitedAfterRequest
            )
        ));
        let first = control.outcome;
        control.confirmed = true;
        control.request(&mut process);
        assert_eq!(control.outcome, first);
        let deadline = Instant::now() + Duration::from_secs(5);
        while process.poll().is_none() {
            assert!(
                Instant::now() < deadline,
                "normal watcher did not report launcher exit"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            process.poll().unwrap().status.as_ref().unwrap().signal(),
            Some(15)
        );
        assert_eq!(std::fs::read(&content).unwrap(), b"preserved game bytes");
    }
}
