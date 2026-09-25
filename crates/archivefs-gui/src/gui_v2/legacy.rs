//! Intentional contextual handoff, not an automatic fallback on navigation.
use super::routes::Section;
use crate::{app::ArchiveFsApp, navigation::MainView, view_mode::GuiMode};
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub(super) fn destination(section: Section, selected: bool) -> MainView {
    match section {
        Section::Check => MainView::CheckGames,
        Section::Problems => MainView::Problems,
        Section::Build => MainView::CanonicalOrganisation,
        Section::Converter => MainView::DiscConversion,
        Section::Museum => MainView::Museum,
        Section::Launch if selected => MainView::Selected,
        Section::Launch => MainView::ReadyToPlay,
        Section::Emulators => MainView::EmulatorSetup,
        Section::Firmware => MainView::BiosProjection,
        Section::Mods => MainView::CheatsMods,
        Section::Artwork => MainView::Settings,
        Section::Sources => MainView::SourcesDiscovery,
        Section::Dat => MainView::DatSources,
        Section::History => MainView::HistoryLogs,
        Section::Advanced => MainView::Library,
        Section::Romm => MainView::Sources,
        Section::Setup => MainView::Doctor,
        Section::Settings => MainView::Settings,
        _ => MainView::Library,
    }
}

pub(super) fn open(section: Section, path: Option<&Path>) -> Result<(), String> {
    let mut command = Command::new(std::env::current_exe().map_err(|error| error.to_string())?);
    command
        .arg("--legacy")
        .arg(serde_json::to_string(&section).map_err(|error| error.to_string())?);
    if let Some(path) = path {
        command.arg(path);
    }
    let mut child = command
        .stdin(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    // Reap the deliberate new window without occupying an application worker.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub(super) struct LegacyHost {
    app: ArchiveFsApp,
    section: Section,
    pending: Option<PathBuf>,
}

impl LegacyHost {
    pub fn new(context: egui::Context, section: Section, path: Option<PathBuf>) -> Self {
        let mut app = ArchiveFsApp::new(context);
        // Session-only. Never change the legacy mode preference on disk.
        app.ui_mode = if section == Section::Advanced {
            GuiMode::AdvancedView
        } else {
            GuiMode::Simple
        };
        app.navigate_to_main_view(destination(section, path.is_some()));
        Self {
            app,
            section,
            pending: path,
        }
    }
}

impl eframe::App for LegacyHost {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if matches!(
            self.app.state,
            crate::live_library_controller::LoadState::Ready(_)
        ) && let Some(path) = self.pending.take()
        {
            self.app.archive_context.select_only(path);
            self.app
                .navigate_to_main_view(destination(self.section, true));
        }
        egui::TopBottomPanel::top("v2_legacy_return").show(ui.ctx(), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Legacy / Advanced interface");
                ui.label("GUI v2 remains open. Close this window to return.");
                if ui.button("Return to GUI v2").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
        self.app.update(ui.ctx(), frame);
    }
}
