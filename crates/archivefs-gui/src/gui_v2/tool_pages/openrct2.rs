//! Blue page: an alternative native launch of RollerCoaster Tycoon 2 through
//! an already-installed OpenRCT2.
//!
//! Reuses the alternative-provider planner for readiness and the shared
//! standalone launch/monitor state for the launch itself. EmuWiz never
//! installs OpenRCT2, never downloads or copies game files, and never writes
//! to the chosen RCT2 folder; the folder is chosen explicitly each session.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use archivefs_core::alternative_launch_provider_planner::AlternativeLaunchReadiness;
use archivefs_core::launch::{
    OpenRct2Install, OpenRct2LaunchRequest, Rct2DataReport, discover_openrct2, inspect_rct2_data,
    openrct2_search_dirs, plan_openrct2, probe_openrct2_version,
};
use eframe::egui::{self, RichText};

use super::super::App;
use crate::launch_readiness_page::{
    StandaloneLaunchRequest, StandaloneLaunchState, openrct2::Request,
    show_standalone_launch_action,
};

#[derive(Default)]
enum Scan {
    #[default]
    NotStarted,
    Scanning(Receiver<Vec<OpenRct2Install>>),
    Ready(Vec<OpenRct2Install>),
}

#[derive(Default)]
pub(in crate::gui_v2) struct State {
    scan: Scan,
    data: Option<Rct2DataReport>,
}

impl State {
    fn start(&mut self, context: &egui::Context) {
        let (sender, receiver) = mpsc::channel();
        self.scan = Scan::Scanning(receiver);
        let context = context.clone();
        std::thread::spawn(move || {
            let found = discover_openrct2(&openrct2_search_dirs(), &probe_openrct2_version);
            let _ = sender.send(found);
            context.request_repaint();
        });
    }

    fn poll(&mut self) {
        if let Scan::Scanning(receiver) = &self.scan {
            match receiver.try_recv() {
                Ok(found) => self.scan = Scan::Ready(found),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.scan = Scan::Ready(Vec::new()),
            }
        }
    }

    fn choose(&mut self, root: PathBuf) {
        self.data = Some(inspect_rct2_data(&root));
    }
}

/// Plain-language readiness line. Pure, for tests.
pub(in crate::gui_v2) fn readiness_text(readiness: AlternativeLaunchReadiness) -> &'static str {
    use AlternativeLaunchReadiness as R;
    match readiness {
        R::Ready => "Ready: OpenRCT2 and your RollerCoaster Tycoon 2 folder both check out.",
        R::ProviderMissing => "OpenRCT2 was not found.",
        R::IdentityInsufficient => "Choose your RollerCoaster Tycoon 2 folder.",
        R::VersionUnsupported => "This OpenRCT2 version is too old.",
        R::RequirementsUnknown => "The OpenRCT2 version could not be read, so it is not trusted.",
        R::OriginalAssetsMissing => {
            "The chosen folder is not a usable RollerCoaster Tycoon 2 folder."
        }
        R::Ambiguous | R::UnsupportedGame => "Not ready.",
    }
}

fn show_install(
    ui: &mut egui::Ui,
    install: &OpenRct2Install,
    data: Option<&Rct2DataReport>,
    standalone: &mut StandaloneLaunchState,
) {
    let projection = plan_openrct2(Some(install), data);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("OpenRCT2").strong());
        ui.small(format!(
            "{} · version {}",
            install.executable.display(),
            install.version.as_deref().unwrap_or("unknown")
        ));
    });
    ui.label(readiness_text(projection.readiness));
    if projection.readiness != AlternativeLaunchReadiness::Ready {
        ui.small(&projection.reason);
    }
    if let (AlternativeLaunchReadiness::Ready, Some(data)) = (projection.readiness, data) {
        show_standalone_launch_action(
            ui,
            standalone,
            StandaloneLaunchRequest::OpenRct2(Request::new(OpenRct2LaunchRequest {
                install: install.clone(),
                data: data.clone(),
            })),
        );
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
        "An alternative way to play RollerCoaster Tycoon 2: the open-source OpenRCT2 engine \
         running your own copy of the game's data. This is a different engine from the original \
         game, not the original game itself. EmuWiz starts an OpenRCT2 you already installed; it \
         never installs OpenRCT2, downloads or copies game files, or changes your folders, and \
         it cannot see whether the game then runs.",
    );
    ui.small(
        "OpenRCT2 is a separate GPL-3.0 project (openrct2.org). RollerCoaster Tycoon is a \
         trademark of its owners; EmuWiz is not affiliated with either.",
    );
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        if ui.button("Choose RollerCoaster Tycoon 2 folder…").clicked()
            && let Some(path) = rfd::FileDialog::new().pick_folder()
        {
            state.choose(path);
        }
        let scanning = matches!(state.scan, Scan::Scanning(_));
        if ui
            .add_enabled(!scanning, egui::Button::new("Check again"))
            .clicked()
        {
            if let Some(data) = state.data.take() {
                state.data = Some(inspect_rct2_data(&data.root));
            }
            state.start(ui.ctx());
        }
    });
    match &state.data {
        None => {
            ui.label("No game folder chosen yet. Choose the folder that contains the Data folder.");
        }
        Some(data) => {
            ui.label(format!("Game folder: {}", data.root.display()));
            match &data.blocker {
                None => {
                    ui.small(
                        "Data/g1.dat is present. Your word that this is RollerCoaster Tycoon 2 \
                         is trusted; other data files are not checked.",
                    );
                }
                Some(blocker) => {
                    ui.label(format!("Not usable: {blocker}."));
                }
            }
        }
    }
    ui.separator();
    match &state.scan {
        Scan::NotStarted | Scan::Scanning(_) => {
            ui.label("Looking for OpenRCT2…");
        }
        Scan::Ready(installs) if installs.is_empty() => {
            ui.label(readiness_text(AlternativeLaunchReadiness::ProviderMissing));
            ui.small(
                "Install OpenRCT2 yourself (for example from your distribution), then check \
                 again. EmuWiz does not download or install it.",
            );
        }
        Scan::Ready(installs) => {
            for install in installs {
                show_install(ui, install, state.data.as_ref(), standalone);
            }
        }
    }
}

impl App {
    pub(in crate::gui_v2) fn openrct2_page(&mut self, ui: &mut egui::Ui) {
        let workflows = self.native_workflows.get_or_insert_with(|| {
            super::super::native_workflows::NativeWorkflows::new(ui.ctx().clone())
        });
        show(
            ui,
            &mut self.tools.openrct2,
            &mut workflows.app.launch_standalone,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_non_ready_state_has_its_own_truthful_text() {
        use AlternativeLaunchReadiness as R;
        assert!(readiness_text(R::Ready).starts_with("Ready"));
        for r in [
            R::ProviderMissing,
            R::IdentityInsufficient,
            R::VersionUnsupported,
            R::RequirementsUnknown,
            R::OriginalAssetsMissing,
        ] {
            assert!(!readiness_text(r).starts_with("Ready"), "{r:?}");
        }
    }

    #[test]
    fn choosing_a_bad_folder_records_a_blocker_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = State::default();
        state.choose(dir.path().to_path_buf());
        assert!(state.data.as_ref().unwrap().blocker.is_some());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
