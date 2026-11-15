//! Blue page listing titles already installed in Vita3K, with a Play action.
//!
//! Read-only over Vita3K: it lists what Vita3K itself has installed (fresh
//! inventory through core's reviewed discovery and `param.sfo` inspector) and
//! hands one title to Vita3K through the existing preflight, with the shared
//! standalone launch/monitor state. EmuWiz never installs, edits or moves
//! titles, firmware or saves. A listing is an installed *launch target*, not a
//! verified game; the title name shown is the title's own metadata, for display.

use std::sync::mpsc::{self, Receiver, TryRecvError};

use archivefs_core::launch::{Vita3kInstalledEntry, Vita3kInventory, scan_vita3k_installed_titles};
use archivefs_core::patch_manager::Vita3kProfileDiscoveryRoots;
use eframe::egui::{self, RichText};

use super::super::App;
use crate::launch_readiness_page::{
    StandaloneLaunchRequest, StandaloneLaunchState, show_standalone_launch_action, vita3k,
};

#[derive(Default)]
pub(in crate::gui_v2) enum Scan {
    #[default]
    NotStarted,
    Scanning(Receiver<Result<Vita3kInventory, String>>),
    Ready(Vita3kInventory),
    Failed(String),
}

#[derive(Default)]
pub(in crate::gui_v2) struct State {
    scan: Scan,
}

impl State {
    fn start(&mut self, context: &egui::Context) {
        let (sender, receiver) = mpsc::channel();
        self.scan = Scan::Scanning(receiver);
        let context = context.clone();
        std::thread::spawn(move || {
            let result = Vita3kProfileDiscoveryRoots::from_environment()
                .map_err(|error| error.to_string())
                .map(|roots| scan_vita3k_installed_titles(&roots));
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    fn poll(&mut self) {
        if let Scan::Scanning(receiver) = &self.scan {
            match receiver.try_recv() {
                Ok(Ok(inventory)) => self.scan = Scan::Ready(inventory),
                Ok(Err(error)) => self.scan = Scan::Failed(error),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.scan = Scan::Failed("the Vita3K scan stopped unexpectedly".into());
                }
            }
        }
    }
}

/// What the page should say for a finished scan. Pure, for tests.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::gui_v2) enum Guidance {
    Vita3kMissing,
    Vita3kNotUsable(Vec<String>),
    NoTitles,
    Titles,
}

pub(in crate::gui_v2) fn guidance(inventory: &Vita3kInventory) -> Guidance {
    if inventory.profiles.is_empty() {
        Guidance::Vita3kMissing
    } else if !inventory.profiles.iter().any(|profile| profile.eligible) {
        Guidance::Vita3kNotUsable(
            inventory
                .profiles
                .iter()
                .map(|profile| {
                    format!(
                        "{}: {}",
                        profile.profile_id,
                        profile.blocker.as_deref().unwrap_or("not usable")
                    )
                })
                .collect(),
        )
    } else if inventory.entries.is_empty() {
        Guidance::NoTitles
    } else {
        Guidance::Titles
    }
}

fn show_entry(
    ui: &mut egui::Ui,
    entry: &Vita3kInstalledEntry,
    standalone: &mut StandaloneLaunchState,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(entry.display_title.as_deref().unwrap_or("(untitled)")).strong());
        ui.small(format!(
            "{} (name from the title's own metadata)",
            entry.title_id
        ));
    });
    match &entry.blocker {
        Some(blocker) => {
            ui.label(format!("Cannot be started: {blocker}."));
        }
        None => show_standalone_launch_action(
            ui,
            standalone,
            StandaloneLaunchRequest::Vita3k(vita3k::Request::new(entry.clone())),
        ),
    }
    ui.separator();
}

pub(in crate::gui_v2) fn show(
    ui: &mut egui::Ui,
    state: &mut State,
    standalone: &mut StandaloneLaunchState,
) {
    if standalone.poll() || standalone.is_active() {
        ui.ctx().request_repaint();
    }
    if matches!(state.scan, Scan::NotStarted) {
        state.start(ui.ctx());
    }
    state.poll();
    ui.label(
        "Titles already installed in Vita3K. EmuWiz starts Vita3K with the title you choose; it \
         never installs, edits or moves titles, firmware or saves, and it cannot see whether the \
         game then runs.",
    );
    let scanning = matches!(state.scan, Scan::Scanning(_));
    if ui
        .add_enabled(!scanning, egui::Button::new("Refresh list"))
        .clicked()
    {
        state.start(ui.ctx());
    }
    ui.separator();
    match &state.scan {
        Scan::NotStarted | Scan::Scanning(_) => {
            ui.label("Looking at your Vita3K installation…");
        }
        Scan::Failed(error) => {
            ui.label(format!("Vita3K could not be checked: {error}"));
        }
        Scan::Ready(inventory) => match guidance(inventory) {
            Guidance::Vita3kMissing => {
                ui.label(
                    "Vita3K was not found. Install Vita3K and run it once so it creates its \
                     folders, then refresh. EmuWiz does not download or install emulators or firmware.",
                );
            }
            Guidance::Vita3kNotUsable(reasons) => {
                ui.label("Vita3K was found but is not ready to start titles:");
                for reason in reasons {
                    ui.label(reason);
                }
            }
            Guidance::NoTitles => {
                ui.label(
                    "Vita3K is ready, but no installed titles were found. Install games inside \
                     Vita3K itself (File, then Install), then refresh. EmuWiz never installs titles.",
                );
            }
            Guidance::Titles => {
                for entry in &inventory.entries {
                    show_entry(ui, entry, standalone);
                }
            }
        },
    }
    if let Scan::Ready(inventory) = &state.scan
        && !inventory.skipped.is_empty()
    {
        ui.collapsing(
            format!("{} folders were not listed", inventory.skipped.len()),
            |ui| {
                for line in &inventory.skipped {
                    ui.small(line);
                }
            },
        );
    }
}

impl App {
    pub(in crate::gui_v2) fn vita3k_titles_page(&mut self, ui: &mut egui::Ui) {
        let workflows = self.native_workflows.get_or_insert_with(|| {
            super::super::native_workflows::NativeWorkflows::new(ui.ctx().clone())
        });
        show(
            ui,
            &mut self.tools.vita3k_titles,
            &mut workflows.app.launch_standalone,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::launch::Vita3kProfileSummary;
    use archivefs_core::patch_manager::Vita3kFirmwareState;

    fn profile(eligible: bool, blocker: Option<&str>) -> Vita3kProfileSummary {
        Vita3kProfileSummary {
            profile_id: "vita3k:/x".into(),
            eligible,
            blocker: blocker.map(str::to_string),
            firmware: Vita3kFirmwareState::Unknown,
        }
    }

    #[test]
    fn guidance_distinguishes_missing_unusable_empty_and_ready() {
        assert_eq!(
            guidance(&Vita3kInventory::default()),
            Guidance::Vita3kMissing
        );
        let unusable = Vita3kInventory {
            profiles: vec![profile(
                false,
                Some("no safe Vita3K executable was discovered"),
            )],
            ..Default::default()
        };
        assert!(
            matches!(guidance(&unusable), Guidance::Vita3kNotUsable(r) if r[0].contains("executable"))
        );
        let empty = Vita3kInventory {
            profiles: vec![profile(true, None)],
            ..Default::default()
        };
        assert_eq!(guidance(&empty), Guidance::NoTitles);
    }
}
