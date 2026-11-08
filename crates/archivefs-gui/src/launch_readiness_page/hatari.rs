//! Atari ST launch through Core's isolated Hatari session with kept saves.
//!
//! Hatari runs a private per-game copy of the disk (saves kept) with a fresh `HOME`, no
//! user configuration, no drive B and no hard disks (see
//! `archivefs_core::launch::hatari_session`). Nothing the game writes is kept.

use super::*;
use archivefs_core::launch::process_spawn::WatchedProcess;
use archivefs_core::launch::{HatariLaunchRequest, preflight_and_launch_hatari};
use archivefs_core::patch_manager::HatariProfileDiscoveryRoots;

pub(crate) const LIMITATION: &str = "Plays a private copy of this disk kept just for this game, so saves carry over between plays (the previous state is backed up first). Your original disk is never written, and no drive B, hard disk or storage setting is inherited; only your Hatari hardware preferences are used. Game identity is local (the disk's own content), not a DAT check.";

#[derive(Clone)]
pub(crate) struct Request {
    request: HatariLaunchRequest,
    identity: CanonicalIdentityStatus,
}

impl Request {
    pub(crate) fn new(request: HatariLaunchRequest, game_key: String) -> Self {
        Self {
            identity: CanonicalIdentityStatus::Resolved(archivefs_core::launch::ResolvedIdentity {
                platform_id: "AtariST".into(),
                game_key,
            }),
            request,
        }
    }

    pub(crate) fn key(&self) -> (PathBuf, String) {
        (self.request.selected_content_path.clone(), "hatari".into())
    }

    pub(crate) fn execute(self) -> Result<WatchedProcess, String> {
        let roots = HatariProfileDiscoveryRoots::from_environment()
            .map_err(|error| format!("Hatari setup could not be located: {error}"))?;
        preflight_and_launch_hatari(&self.request, &roots, &self.identity).map_err(|error| {
            match error {
                archivefs_core::launch::HatariLaunchExecutionError::Preflight(error) => format!(
                    "Hatari was not started: {}. Recheck this game and the Hatari setup.",
                    error.detail
                ),
                archivefs_core::launch::HatariLaunchExecutionError::Spawn(error) => {
                    format!("Hatari could not start: {error}")
                }
            }
        })
    }
}

pub(super) fn show_running(ui: &mut egui::Ui, process: &WatchedProcess) {
    ui.small(format!("Hatari running · PID {}", process.pid));
    ui.small("Your game disk is never written. Saves go to a private copy for this game that EmuWiz keeps between plays, with the previous state backed up.");
}

pub(super) fn show_exited(ui: &mut egui::Ui, process: &mut WatchedProcess) {
    if let Some(report) = process.poll() {
        ui.label(super::exit_line("Hatari", report));
    }
}

pub(super) fn exit_failure(
    report: &archivefs_core::launch::process_spawn::ProcessExitReport,
) -> Option<String> {
    match &report.status {
        Ok(status) if status.success() => None,
        _ => Some(super::exit_line("Hatari", report)),
    }
}
