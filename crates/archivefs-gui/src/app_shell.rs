//! Application-shell chrome and navigation requests.
//!
//! This module deliberately owns only the frame chrome: menus, mode bars,
//! sidebar layout, and the requests those controls emit. Feature workers,
//! page dispatch, and overlay contents remain with `ArchiveFsApp` and their
//! existing owners so the frame ordering is unchanged.

use std::path::PathBuf;

use eframe::egui;

use crate::navigation::{
    GAMER_MENU_ADD_FOLDER_LABEL, GAMER_MENU_ADVANCED_LABEL, GAMER_MENU_LABEL,
    GAMER_MENU_SCAN_LABEL, GAMER_MENU_SETUP_LABEL, NavClick, TOOLS_MENU_WORKFLOWS, main_view_title,
    show_primary_navigation,
};
use crate::{MainView, ToolsOverlay};

#[derive(Debug)]
pub(crate) enum ShellRequest {
    Navigate(NavClick),
    ScanLibrary,
    RefreshDatabase,
    RefreshView,
    SelectAllVisible,
    ClearSelection,
    ToggleActivity,
    ShowAbout,
    ReturnToGamerView,
    GamerAddFolder(PathBuf),
    GamerScan,
    GamerSetup,
    GamerAdvanced,
}

pub(crate) struct ShellInputs {
    pub(crate) advanced_view: bool,
    pub(crate) view: MainView,
    pub(crate) tools_overlay: ToolsOverlay,
    pub(crate) loading: bool,
    pub(crate) busy: bool,
    pub(crate) has_database: bool,
    pub(crate) selection_count: usize,
    pub(crate) show_activity: bool,
    pub(crate) source_actions_available: bool,
}

pub(crate) fn show_shell(context: &egui::Context, inputs: ShellInputs) -> Option<ShellRequest> {
    let mut navigation_request = None;
    if inputs.advanced_view {
        egui::TopBottomPanel::top("menu_bar").show(context, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(main_view_title(inputs.view)).strong());
                ui.separator();
                ui.menu_button("File", |ui| {
                    if ui.button("Quit").clicked() {
                        context.send_viewport_cmd(egui::ViewportCommand::Close);
                        ui.close();
                    }
                });
                ui.menu_button("Library", |ui| {
                    if ui
                        .add_enabled(!inputs.loading && !inputs.busy, egui::Button::new("Scan library"))
                        .on_hover_text("Scan your configured source folders for new and changed files.")
                        .clicked()
                    {
                        navigation_request = Some(ShellRequest::ScanLibrary);
                        ui.close();
                    }
                    if ui
                        .add_enabled(!inputs.busy, egui::Button::new("Refresh database status"))
                        .on_hover_text("Re-read the catalogue database status without rescanning your folders.")
                        .clicked()
                    {
                        navigation_request = Some(ShellRequest::RefreshDatabase);
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .button("Select all visible")
                        .on_hover_text("Select every archive currently shown in the Library.")
                        .clicked()
                    {
                        navigation_request = Some(ShellRequest::SelectAllVisible);
                        ui.close();
                    }
                    if ui
                        .add_enabled(!inputs.selection_count.eq(&0), egui::Button::new("Clear selection"))
                        .on_hover_text("Deselect every selected archive.")
                        .clicked()
                    {
                        navigation_request = Some(ShellRequest::ClearSelection);
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(!inputs.loading && !inputs.busy, egui::Button::new("Refresh"))
                        .on_hover_text("Refresh EmuWiz's current view of your files without running a full scan.")
                        .clicked()
                    {
                        navigation_request = Some(ShellRequest::RefreshView);
                        ui.close();
                    }
                });
                ui.menu_button("Sources", |ui| {
                    if ui.button("Open Sources page").clicked() {
                        navigation_request = Some(ShellRequest::Navigate(NavClick::View(MainView::Sources)));
                        ui.close();
                    }
                    if ui
                        .button("RomM")
                        .on_hover_text("Connect EmuWiz to your RomM server and browse its records (Sources -> Libraries).")
                        .clicked()
                    {
                        navigation_request = Some(ShellRequest::Navigate(NavClick::Romm));
                        ui.close();
                    }
                });
                ui.menu_button("Tools", |ui| {
                    if ui
                        .add_enabled(!inputs.busy, egui::Button::new("Diagnostics"))
                        .on_hover_text("Check configuration, mount root and source-folder health.")
                        .clicked()
                    {
                        navigation_request = Some(ShellRequest::Navigate(NavClick::Overlay(ToolsOverlay::Diagnostics)));
                        ui.close();
                    }
                    if ui.button("Doctor checks").on_hover_text("Run the read-only Doctor scan of this EmuWiz installation.").clicked() {
                        navigation_request = Some(ShellRequest::Navigate(NavClick::Overlay(ToolsOverlay::DoctorChecks)));
                        ui.close();
                    }
                    if ui.button("Platform Aliases").on_hover_text("Review the folder and filename aliases EmuWiz uses to recognise platforms.").clicked() {
                        navigation_request = Some(ShellRequest::Navigate(NavClick::Overlay(ToolsOverlay::PlatformAliases)));
                        ui.close();
                    }
                    if ui.button("Database Status").on_hover_text("Inspect the catalogue database and its health.").clicked() {
                        navigation_request = Some(ShellRequest::Navigate(NavClick::Overlay(ToolsOverlay::DatabaseStatus)));
                        ui.close();
                    }
                    ui.separator();
                    for (label, hover, target) in TOOLS_MENU_WORKFLOWS {
                        if ui.button(label).on_hover_text(hover).clicked() {
                            navigation_request = Some(ShellRequest::Navigate(NavClick::View(target)));
                            ui.close();
                        }
                    }
                    ui.separator();
                    let activity_label = if inputs.show_activity {
                        "Hide Activity"
                    } else {
                        "Show Activity"
                    };
                    if ui.button(activity_label).on_hover_text("Show or hide the recent-activity panel.").clicked() {
                        navigation_request = Some(ShellRequest::ToggleActivity);
                        ui.close();
                    }
                });
                ui.menu_button("Help", |ui| {
                    if ui.button("About EmuWiz").clicked() {
                        navigation_request = Some(ShellRequest::ShowAbout);
                        ui.close();
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if inputs.loading || inputs.busy {
                        ui.spinner();
                    }
                    if ui.button("Return to Gamer View").on_hover_text("Switch back to the simple, one-screen view.").clicked() {
                        navigation_request = Some(ShellRequest::ReturnToGamerView);
                        ui.close();
                    }
                });
            });
        });
        egui::SidePanel::left("app_navigation")
            .resizable(false)
            .exact_width(218.0)
            .show(context, |ui| {
                ui.add_space(14.0);
                if let Some(click) = show_primary_navigation(
                    ui,
                    inputs.view,
                    inputs.tools_overlay,
                    inputs.has_database,
                ) {
                    navigation_request = Some(ShellRequest::Navigate(click));
                }
            });
    } else {
        egui::TopBottomPanel::top("gamer_top_bar").show(context, |ui| {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if inputs.loading || inputs.busy {
                        ui.spinner();
                    }
                    ui.menu_button(GAMER_MENU_LABEL, |ui| {
                        if ui
                            .add_enabled(
                                inputs.source_actions_available,
                                egui::Button::new(GAMER_MENU_ADD_FOLDER_LABEL),
                            )
                            .on_hover_text("Choose another folder for EmuWiz to scan for games.")
                            .clicked()
                        {
                            if let Some(folder) = rfd::FileDialog::new()
                                .set_title("Choose another games folder")
                                .pick_folder()
                            {
                                navigation_request = Some(ShellRequest::GamerAddFolder(folder));
                            }
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                inputs.source_actions_available,
                                egui::Button::new(GAMER_MENU_SCAN_LABEL),
                            )
                            .on_hover_text(
                                "Look through all enabled game folders for new and changed games.",
                            )
                            .clicked()
                        {
                            navigation_request = Some(ShellRequest::GamerScan);
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .button(GAMER_MENU_SETUP_LABEL)
                            .on_hover_text("Check emulator setup and launch readiness.")
                            .clicked()
                        {
                            navigation_request = Some(ShellRequest::GamerSetup);
                            ui.close();
                        }
                        if ui.button(GAMER_MENU_ADVANCED_LABEL).clicked() {
                            navigation_request = Some(ShellRequest::GamerAdvanced);
                            ui.close();
                        }
                    });
                });
            });
        });
    }
    navigation_request
}
