//! Blue port of eba83521's EmuWiz checker adapter. Session-local, explicit
//! checks only. The existing HTTPS transport is constructed on the worker.
use std::sync::mpsc::{self, Receiver, TryRecvError};

use crate::ui::{components as widgets, theme};
use archivefs_core::emulator_download::HttpsEmulatorDownloadTransport;
use archivefs_core::self_update::{
    EMUWIZ_OFFICIAL_REPOSITORY, EmuWizUpdateCheck, EmuWizUpdateStatus, check_emuwiz_update,
    current_emuwiz_version, validated_emuwiz_release_url,
};
use eframe::egui;

#[derive(Default)]
pub(super) struct UpdateCheckState {
    task: Option<Receiver<EmuWizUpdateCheck>>,
    last_result: Option<EmuWizUpdateCheck>,
}

fn failed_check(detail: String) -> EmuWizUpdateCheck {
    EmuWizUpdateCheck {
        current_version: current_emuwiz_version().into(),
        latest_version: None,
        release_url: None,
        release_notes: None,
        status: EmuWizUpdateStatus::CheckFailed,
        detail: Some(detail),
        checked_at_unix_seconds: unix_seconds(),
    }
}

fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl UpdateCheckState {
    fn start_check_with<F>(&mut self, ctx: egui::Context, checker: F)
    where
        F: FnOnce() -> EmuWizUpdateCheck + Send + 'static,
    {
        if self.task.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        match std::thread::Builder::new()
            .name("emuwiz-update-check".into())
            .spawn(move || {
                let result = checker();
                let _ = sender.send(result);
                ctx.request_repaint();
            }) {
            Ok(_) => self.task = Some(receiver),
            Err(error) => {
                self.last_result = Some(failed_check(format!(
                    "Could not start update-check worker: {error}"
                )))
            }
        }
    }
    fn poll(&mut self, ctx: &egui::Context) {
        let Some(receiver) = &self.task else {
            return;
        };
        match receiver.try_recv() {
            Ok(result) => {
                self.last_result = Some(result);
                self.task = None;
            }
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(150))
            }
            Err(TryRecvError::Disconnected) => {
                self.task = None;
                self.last_result = Some(failed_check(
                    "The update-check worker ended without a result. You can retry.".into(),
                ));
            }
        }
    }
}

fn status_message(status: EmuWizUpdateStatus) -> &'static str {
    match status {
        EmuWizUpdateStatus::UpToDate => "You have the latest published version tag",
        EmuWizUpdateStatus::UpdateAvailable => "A newer EmuWiz release is available",
        EmuWizUpdateStatus::DevelopmentBuild => {
            "This build has a newer version tag than the published release"
        }
        EmuWizUpdateStatus::CurrentVersionUnknown => "Current version could not be compared",
        EmuWizUpdateStatus::LatestVersionUnknown => "Release version could not be compared",
        EmuWizUpdateStatus::NetworkFailure => "Connection or transfer failed (possibly offline)",
        EmuWizUpdateStatus::Unavailable => "The official release endpoint is unavailable",
        EmuWizUpdateStatus::MalformedResponse => "Release metadata is malformed or untrusted",
        EmuWizUpdateStatus::CheckFailed => "The update check did not complete",
    }
}

fn status_tone(status: EmuWizUpdateStatus) -> widgets::StatusTone {
    match status {
        EmuWizUpdateStatus::UpToDate => widgets::StatusTone::Success,
        EmuWizUpdateStatus::UpdateAvailable => widgets::StatusTone::Active,
        EmuWizUpdateStatus::DevelopmentBuild => widgets::StatusTone::Info,
        _ => widgets::StatusTone::Warning,
    }
}

fn release_link(check: &EmuWizUpdateCheck) -> Option<&str> {
    let url = check.release_url.as_deref()?;
    let tag = check.latest_version.as_deref()?;
    validated_emuwiz_release_url(url, tag).then_some(url)
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut UpdateCheckState) {
    show_with_checker(ui, state, || {
        check_emuwiz_update(&HttpsEmulatorDownloadTransport::new())
    });
}

fn show_with_checker<F>(
    ui: &mut egui::Ui,
    state: &mut UpdateCheckState,
    checker: F,
) -> egui::Response
where
    F: FnOnce() -> EmuWizUpdateCheck + Send + 'static,
{
    state.poll(ui.ctx());
    ui.separator();
    widgets::section_header(
        ui,
        "About EmuWiz",
        Some("Check only when you ask. Results stay in this session; no installation or restart."),
    );
    ui.label(format!("Current version: {}", current_emuwiz_version()));
    ui.label(
        "Compares published version tags; local Git commits and binary identity are not compared.",
    );
    let checking = state.task.is_some();
    if checking {
        ui.spinner();
        ui.label("Checking the official release metadata…");
    }
    if let Some(check) = &state.last_result {
        ui.label(if checking {
            "Previous completed check"
        } else {
            "Last check result"
        });
        widgets::status_badge(ui, status_message(check.status), status_tone(check.status));
        ui.label(format!(
            "Available version: {}",
            check.latest_version.as_deref().unwrap_or("Unknown")
        ));
        ui.label(format!(
            "Checked {} seconds ago",
            unix_seconds()
                .saturating_sub(check.checked_at_unix_seconds)
                .max(0)
        ));
        if let Some(detail) = &check.detail {
            ui.label(detail);
        }
        if let Some(url) = release_link(check) {
            ui.hyperlink_to("Open official release notes", url);
        }
        if let Some(notes) = &check.release_notes {
            ui.collapsing("Release notes (plain text)", |ui| {
                let excerpt = notes.chars().take(8000).collect::<String>();
                ui.label(excerpt);
                if notes.chars().count() > 8000 {
                    ui.label("Excerpt shown; open the official release page for all notes.");
                }
            });
        }
    } else {
        ui.label("Not checked in this session. Offline use remains available.");
    }
    let response = widgets::action_button(
        ui,
        if checking {
            "Checking…"
        } else {
            "Check for Updates"
        },
        widgets::ActionStyle::Primary,
        !checking,
    );
    if response.clicked() {
        state.start_check_with(ui.ctx().clone(), checker);
    }
    ui.label(format!("Official source: {EMUWIZ_OFFICIAL_REPOSITORY}"));
    ui.add_space(theme::SECTION_GAP);
    response
}

#[cfg(test)]
mod tests;
