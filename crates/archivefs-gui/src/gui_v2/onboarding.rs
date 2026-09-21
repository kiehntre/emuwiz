//! Native first-run and environment Doctor presentation.
//!
//! The page is a calm projection over EnvironmentSnapshot and the
//! already-loaded library. It does not scan, install, migrate, or repair.

use super::{environment::EnvironmentSnapshot, library::Library, routes::Section};
use eframe::egui;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Action {
    DismissWelcome,
    DismissAndGoHome,
    Refresh,
    Open(Section),
    OpenPlatform(String),
    OpenGame(i64),
}

fn button(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add(
        egui::Button::new(egui::RichText::new(label).strong())
            .fill(egui::Color32::from_rgb(30, 85, 137))
            .min_size(egui::vec2(180.0, 42.0)),
    )
    .clicked()
}

fn status(ui: &mut egui::Ui, label: &str, tone: egui::Color32) {
    ui.colored_label(tone, label);
}

fn ready(ui: &mut egui::Ui, label: &str) {
    status(
        ui,
        &format!("Ready · {label}"),
        egui::Color32::from_rgb(90, 205, 150),
    );
}

fn attention(ui: &mut egui::Ui, label: &str) {
    status(
        ui,
        &format!("Needs attention · {label}"),
        egui::Color32::from_rgb(235, 178, 76),
    );
}

fn optional(ui: &mut egui::Ui, label: &str) {
    status(
        ui,
        &format!("Optional · {label}"),
        egui::Color32::from_rgb(150, 180, 220),
    );
}

pub(super) fn show(
    ui: &mut egui::Ui,
    snapshot: Option<&EnvironmentSnapshot>,
    library: &Library,
    welcome_dismissed: bool,
    selected_platform: Option<&str>,
) -> Option<Action> {
    let Some(snapshot) = snapshot else {
        ui.heading("Setup & Doctor");
        ui.label("Checking this computer. You can keep browsing while this finishes.");
        ui.spinner();
        return None;
    };

    if snapshot.is_fresh() && !welcome_dismissed {
        return welcome(ui);
    }

    let mut action = None;
    egui::ScrollArea::vertical()
        .id_salt("v2_setup_doctor")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.heading("Setup & Doctor");
            ui.label("See what EmuWiz can use now, what needs attention, and the next safe step.");

            if snapshot.both_roots_conflict {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("Two existing setups were found");
                    ui.label("EmuWiz found both an EmuWiz setup and an older ArchiveFS setup.");
                    ui.label("Nothing will be merged automatically.");
                    ui.label("Run Upgrade Preflight before choosing which setup should remain active.");
                    ui.collapsing("Advanced details", |ui| {
                        if let Some(path) = &snapshot.active_config_root {
                            ui.label(format!("Active configuration folder: {}", path.display()));
                        }
                        if let Some(path) = &snapshot.active_data_root {
                            ui.label(format!("Active data folder: {}", path.display()));
                        }
                    });
                });
            }

            let ready_count = snapshot.setup_ready_count(library);
            let attention_count = snapshot.setup_attention_count(library);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading("System setup");
                ui.label(format!("{ready_count} ready · {attention_count} need attention"));
                if attention_count == 0 {
                    ready(ui, "EmuWiz can guide you through the next step");
                }
                if button(ui, "Check again") {
                    action = Some(Action::Refresh);
                }
            });

            if snapshot.database_upgrade_required {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("Your library needs an upgrade");
                    ui.label("The library database is from an older EmuWiz version and must be upgraded before it can be opened.");
                    ui.label(format!(
                        "Current database schema: {} · required: {}",
                        snapshot.database_schema.map_or_else(|| "unknown".to_string(), |schema| schema.to_string()),
                        snapshot.required_schema
                    ));
                    ui.label("Open the existing database upgrade workflow from Advanced when you are ready.");
                    if button(ui, "Open upgrade tools") {
                        action = Some(Action::Open(Section::Advanced));
                    }
                });
            }

            checklist(ui, snapshot, library, &mut action);

            ui.heading("Emulators");
            ui.label("EmuWiz only checks installed programs here. It never installs or replaces one silently.");
            for emulator in &snapshot.emulator_summaries {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(&emulator.label);
                        if emulator.installations == 0 {
                            attention(ui, "Not found");
                        } else {
                            ready(ui, &format!("{} installation(s) found", emulator.installations));
                        }
                    });
                    if emulator.installations > 0 {
                        ui.label(
                            emulator
                                .installation_types
                                .iter()
                                .map(|kind| friendly_installation_type(kind))
                                .collect::<Vec<_>>()
                                .join(", "),
                        );
                        ui.collapsing("Advanced details", |ui| {
                            for path in &emulator.paths {
                                ui.label(path.display().to_string());
                            }
                        });
                    }
                });
            }

            if snapshot.source_needs_attention() {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("Game drive unavailable");
                    for path in &snapshot.unavailable_sources {
                        ui.label(format!("{} may not be mounted.", path.display()));
                    }
                    if let Some(error) = &snapshot.source_error {
                        ui.label("EmuWiz could not read the saved game-folder settings.");
                        ui.collapsing("Advanced details", |ui| {
                            ui.label(error);
                        });
                    } else {
                        ui.label("EmuWiz will not rescan or replace this source, and will not treat it as an empty library.");
                    }
                    if button(ui, "Open Games folders") {
                        action = Some(Action::Open(Section::Sources));
                    }
                });
            }

            if let Some(error) = &snapshot.database_error {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("The library needs attention");
                    ui.label("EmuWiz could not safely inspect the library database.");
                    ui.collapsing("Advanced details", |ui| {
                        ui.label(error);
                    });
                    if button(ui, "Open Problems & Repair") {
                        action = Some(Action::Open(Section::Problems));
                    }
                });
            }

            ui.add_space(8.0);
            ui.heading("Environment Doctor");
            ui.label("Why can’t this game or system launch? Select a system for the checks EmuWiz already knows about.");

            for (platform, count) in &library.platforms {
                let selected = selected_platform == Some(platform.as_str());
                if ui
                    .selectable_label(selected, format!("{platform} · {count} game(s)"))
                    .clicked()
                {
                    action = Some(Action::OpenPlatform(platform.clone()));
                }
            }
            if library.platforms.is_empty() {
                ui.label("No games are available yet. Add a Games folder to begin.");
            }
            if let Some(platform) = selected_platform
                && library.platforms.contains_key(platform)
            {
                platform_details(ui, snapshot, library, platform, &mut action);
            }
        });
    action
}

fn friendly_installation_type(kind: &str) -> &str {
    match kind {
        "Flatpak" => "Managed by Flatpak",
        "AppImage" => "EmuWiz-managed AppImage",
        "SystemPackage" => "System package",
        "Portable" => "Portable installation",
        "Manual" => "Installed manually",
        "Managed" => "Managed installation",
        _ => "Installation type unknown",
    }
}

fn welcome(ui: &mut egui::Ui) -> Option<Action> {
    let mut action = None;
    egui::ScrollArea::vertical()
        .id_salt("v2_first_run")
        .show(ui, |ui| {
            ui.heading("Welcome to EmuWiz");
            ui.label("EmuWiz can find your games, check what each file is, and help you get them ready to play.");
            ui.label("You do not need to set everything up today. Start with one Games folder and return whenever you like.");
            ui.add_space(10.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading("Get started");
                ui.label("Set up the basics so EmuWiz can find and check your games.");
                if button(ui, "Get started") {
                    action = Some(Action::DismissWelcome);
                }
            });
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading("I already know what I’m doing");
                ui.label("Go straight to the normal interface. You can always return to Setup & Doctor.");
                if ui.button("Open normal interface").clicked() {
                    action = Some(Action::DismissAndGoHome);
                }
            });
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading("Recover an existing install");
                ui.label("Check for an older EmuWiz or ArchiveFS setup before changing anything.");
                if ui.button("Check existing setup").clicked() {
                    action = Some(Action::DismissWelcome);
                }
            });
        });
    action
}

fn checklist(
    ui: &mut egui::Ui,
    snapshot: &EnvironmentSnapshot,
    library: &Library,
    action: &mut Option<Action>,
) {
    ui.heading("Setup checklist");
    egui::Grid::new("v2_setup_checklist")
        .num_columns(3)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.strong("1. Games folder");
            if snapshot.source_count == 0 {
                status(ui, "Not started", egui::Color32::from_rgb(150, 180, 220));
            } else if snapshot.source_needs_attention() {
                attention(ui, "Game drive unavailable");
            } else {
                ready(ui, "Folder available");
            }
            ui.label(format!(
                "{} configured · {} available",
                snapshot.source_count, snapshot.available_sources
            ));
            ui.end_row();

            ui.strong("2. Emulators");
            if snapshot.installed_emulator_count() == 0 {
                if library.games.is_empty() {
                    optional(ui, "Choose when needed");
                } else {
                    attention(ui, "No supported emulator found");
                }
            } else {
                ready(ui, "Installed emulators found");
            }
            ui.label(format!(
                "{} installation(s) found",
                snapshot.installed_emulator_count()
            ));
            ui.end_row();

            ui.strong("3. Game identification data");
            if snapshot.identification_data_ready {
                ready(ui, "Catalogue available");
            } else {
                optional(ui, "Not added yet");
            }
            ui.label("Trusted catalogues identify exact releases and regions.");
            ui.end_row();

            ui.strong("4. System software");
            if library.games.is_empty() {
                optional(ui, "Check when you choose a system");
            } else if snapshot.installed_emulator_count() == 0 {
                attention(ui, "Choose an emulator first");
            } else {
                optional(ui, "Checked when you prepare to play");
            }
            ui.label("Some systems need original system software before games can start.");
            ui.end_row();

            ui.strong("5. Ready to play");
            if !library.games.is_empty() && snapshot.installed_emulator_count() > 0 {
                ready(ui, "Open a game to check it");
            } else {
                optional(ui, "Choose a Games folder first");
            }
            ui.label("Setup is resumable; it does not need to be completed all at once.");
            ui.end_row();

            if snapshot.source_count == 0 && ui.button("Add a Games folder").clicked() {
                *action = Some(Action::Open(Section::Sources));
            }
            if snapshot.installed_emulator_count() == 0
                && ui.button("Open Emulator Setup").clicked()
            {
                *action = Some(Action::Open(Section::Emulators));
            }
            if !snapshot.identification_data_ready
                && ui.button("Open identification data").clicked()
            {
                *action = Some(Action::Open(Section::Advanced));
            }
            ui.end_row();
        });
}

fn platform_details(
    ui: &mut egui::Ui,
    snapshot: &EnvironmentSnapshot,
    library: &Library,
    platform: &str,
    action: &mut Option<Action>,
) {
    let games = library
        .games
        .iter()
        .filter(|game| game.platform == platform)
        .collect::<Vec<_>>();
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.heading(platform);
        ui.label(format!("{} game(s) in this system", games.len()));
        if snapshot.installed_emulator_count() == 0 {
            attention(ui, "Choose an emulator");
            ui.label("No supported emulator was found by the existing inventory check.");
            if ui.button("Open Emulator Setup").clicked() {
                *action = Some(Action::Open(Section::Emulators));
            }
        } else {
            ready(ui, "An emulator installation was found");
            ui.label("Open a game to run the existing launch-readiness checks for its emulator and system software.");
            if ui.button("Open Emulator Setup").clicked() {
                *action = Some(Action::Open(Section::Emulators));
            }
        }
        let unresolved = games.iter().filter(|game| !game.identified).count();
        let attention_games = games.iter().filter(|game| game.attention).count();
        if unresolved > 0 {
            attention(ui, &format!("{unresolved} game(s) need identification review"));
            if ui.button("Open game checks").clicked() {
                *action = Some(Action::Open(Section::Check));
            }
        } else if attention_games > 0 {
            attention(ui, &format!("{attention_games} game(s) need attention"));
            if ui.button("Open Problems & Repair").clicked() {
                *action = Some(Action::Open(Section::Problems));
            }
        } else {
            ready(ui, "No library blocker is recorded for this system");
        }
        for game in games.iter().filter(|game| !game.identified || game.attention).take(8) {
            if ui
                .button(format!("Why won’t {} start?", game.title))
                .clicked()
            {
                *action = Some(Action::OpenGame(game.archive.id));
            }
        }
        ui.collapsing("Advanced details", |ui| {
            ui.label("EmuWiz uses the existing launch planner and identity evidence. This page does not recalculate readiness.");
            ui.label(format!("Platform key: {platform}"));
        });
    });
}
