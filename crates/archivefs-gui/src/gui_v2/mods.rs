//! Native GUI-v2 presentation for the existing mod and cheat backends.
//!
//! This module deliberately owns only navigation, presentation, and cached
//! history. Package inspection, compatibility decisions, transactions, and
//! rollback remain in the established mod pages and archivefs-core.

use std::{
    path::Path,
    sync::mpsc::{self, Receiver, TryRecvError},
};

use archivefs_core::mod_history::{ModHistory, ModReceiptSummary};
use eframe::egui;

use super::{
    activity::Activity,
    native_workflows::NativeWorkflows,
    routes::{Route, Section},
};
use crate::local_mod_package_page::{
    LocalModPackagePageState, show_local_mod_package_panel_with_catalogue,
};
use crate::ui::components as widgets;
use crate::ui::theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Installed,
    Add,
    Stack,
    Conflicts,
    Cheats,
}

#[derive(Default)]
pub(super) struct ModsPageState {
    tab: Tab,
    local: LocalModPackagePageState,
    history: Option<ModHistory>,
    history_error: Option<String>,
    history_loaded: bool,
    history_worker: Option<Receiver<Result<ModHistory, String>>>,
    activity_id: Option<u64>,
}

impl ModsPageState {
    pub(super) fn poll(&mut self) -> bool {
        let local_changed = self.local.poll();
        let Some(worker) = &self.history_worker else {
            return local_changed;
        };
        let result = match worker.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return local_changed,
            Err(TryRecvError::Disconnected) => {
                Err("History worker stopped before finishing.".into())
            }
        };
        self.history_worker = None;
        match result {
            Ok(history) => {
                self.history = Some(history);
                self.history_error = None;
            }
            Err(error) => self.history_error = Some(error),
        }
        true
    }

    fn refresh_history(&mut self, context: &egui::Context) {
        if self.history_worker.is_some() {
            return;
        }
        self.history_error = None;
        self.history_loaded = true;
        let (sender, receiver) = mpsc::channel();
        self.history_worker = Some(receiver);
        let context = context.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let history = archivefs_core::patch_manager::default_shared_history_root()
                    .map_err(|e| e.detail)?;
                let backups = archivefs_core::patch_manager::default_shared_backup_root()
                    .map_err(|e| e.detail)?;
                Ok(ModHistory::from_shared_history(&history, &backups))
            })();
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    #[cfg(test)]
    pub(super) fn select_cheats(&mut self) {
        self.tab = Tab::Cheats;
    }
}

pub(super) fn show_mods_page(
    ui: &mut egui::Ui,
    state: &mut ModsPageState,
    selected_game: Option<&crate::gui_v2::library::Game>,
    workflows: &mut NativeWorkflows,
    activity: &mut Activity,
) -> Option<Route> {
    state.poll();
    if !state.history_loaded {
        state.refresh_history(ui.ctx());
    }

    let mut destination = None;
    egui::ScrollArea::vertical()
        .id_salt("v2_mods_native")
        .show(ui, |ui| {
            widgets::workflow_header(
                ui,
                "Cheats & Mods workshop",
                "Choose a game and inspect compatibility before reviewing a change. Browsing this workshop does not change game or emulator files.",
            );

            let tabs = [
                (Tab::Installed, "Installed"),
                (Tab::Add, "Available packages"),
                (Tab::Stack, "Active stack"),
                (Tab::Conflicts, "Conflicts"),
                (Tab::Cheats, "Cheats"),
            ];
            ui.horizontal_wrapped(|ui| {
                for (tab, label) in tabs {
                    if ui.selectable_label(state.tab == tab, label).clicked() {
                        state.tab = tab;
                    }
                }
            });
            ui.separator();
            if state.history_worker.is_some() && !(state.tab == Tab::Cheats && selected_game.is_none()) {
                ui.spinner();
                ui.label("Reading change history in the background. You can keep browsing; previous results may be out of date.");
            }
            if let Some(history) = &state.history
                && !history.complete
            {
                ui.colored_label(theme::WARNING, "Some change records could not be read. This is an incomplete view; refresh history before relying on it for recovery.");
                widgets::technical_details(ui, "incomplete_mod_history", |ui| {
                    for problem in &history.problems { ui.label(problem); }
                });
            }
            ui.horizontal_wrapped(|ui| {
                if selected_game.is_none() && ui.button("Choose a game").clicked() { destination = Some(Route::Section(Section::Games)); }
                if let Some(game) = selected_game {
                    if selected_identity(Some(game)).is_none() {
                        ui.label("We need to identify this game before cheats or mods can be matched safely.");
                        if ui.button("Review identity").clicked() { destination = Some(Route::ReviewIdentity(game.archive.id)); }
                    }
                }
                if ui.button("Open History & Undo").clicked() { destination = Some(Route::Section(Section::History)); }
            });
            if state.tab == Tab::Cheats && selected_game.is_some() {
                ui.label("Recognised code syntax does not prove a cheat works in-game. Check the game version, region and emulator before previewing any supported change; installing a file is not proof that a cheat is active.");
            }
            if state.tab == Tab::Cheats && selected_game.is_none() {
                destination = workflows.show_cheats(ui, selected_game, activity).or(destination.take());
            } else {
                selected_game_strip(ui, selected_game);
                workshop_lanes(ui, &mut state.tab);
                match state.tab {
                    Tab::Installed => installed(ui, state.history.as_ref(), selected_game),
                    Tab::Add => add_package(ui, &mut state.local, selected_game),
                    Tab::Stack => stack(ui, state.history.as_ref(), selected_game),
                    Tab::Conflicts => conflicts(ui, state.history.as_ref(), selected_game),
                    Tab::Cheats => {
                        destination = workflows.show_cheats(ui, selected_game, activity).or(destination.take());
                    }
                }
            }

            if let Some(error) = &state.history_error {
                ui.colored_label(theme::WARNING, "Change history could not be refreshed. Previous results may be incomplete. Retry Refresh history; no mods were changed by this check.");
                widgets::technical_details(ui, "mod_history_error", |ui| { ui.label(error); });
            }
            ui.add_space(theme::SPACE_SM);
            ui.horizontal_wrapped(|ui| {
                ui.strong("Change history & recovery");
                ui.label("Review what was changed, and use undo only where EmuWiz reports it is safe.");
                if ui.add_enabled(state.history_worker.is_none(), egui::Button::new("Refresh history")).clicked() {
                    state.refresh_history(ui.ctx());
                }
            });
        });

    let busy = state.local.is_busy();
    if busy {
        ui.label("The mod operation is running. It cannot be cancelled from Activity; wait for the result before starting another change.");
    }
    if busy && state.activity_id.is_none() {
        let id = activity.queue(
            "Inspecting or applying a mod",
            Route::Section(Section::Mods),
            false,
        );
        activity.start(id);
        state.activity_id = Some(id);
    } else if !busy && let Some(id) = state.activity_id.take() {
        activity.finish(
            id,
            "Mod workflow finished. Review the result and shared receipt above.".into(),
            None,
        );
        state.history_loaded = false;
        state.refresh_history(ui.ctx());
    }
    destination
}

fn selected_game_strip(ui: &mut egui::Ui, game: Option<&crate::gui_v2::library::Game>) {
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("◈").size(28.0).color(theme::TEAL));
            ui.vertical(|ui| match game {
                Some(game) => {
                    ui.label(
                        egui::RichText::new(&game.title)
                            .size(theme::SECTION_TITLE_SIZE)
                            .strong(),
                    );
                    ui.horizontal_wrapped(|ui| {
                        widgets::info_chip(ui, &game.platform);
                        widgets::status_badge(ui, "Game selected", widgets::StatusTone::Active);
                    });
                    ui.label(
                        "Changes are scoped to this game and its supported emulator locations.",
                    );
                }
                None => {
                    ui.label(
                        egui::RichText::new("No game selected yet")
                            .size(theme::SECTION_TITLE_SIZE)
                            .strong(),
                    );
                    ui.label(
                        "Open a game from Games to inspect its cheats, mods and change history.",
                    );
                }
            });
        });
    });
}

fn workshop_lanes(ui: &mut egui::Ui, tab: &mut Tab) {
    ui.label(
        egui::RichText::new("Choose what to work on")
            .size(theme::SECTION_TITLE_SIZE)
            .strong(),
    );
    let available = ui.available_width();
    let width = if available < 600.0 {
        available
    } else {
        (available - ui.spacing().item_spacing.x) / 2.0
    };
    let layout = if available < 600.0 {
        egui::Layout::top_down(egui::Align::Min)
    } else {
        egui::Layout::left_to_right(egui::Align::Min)
    };
    ui.with_layout(layout, |ui| {
        lane_card(
            ui,
            width,
            "Cheats",
            "Gameplay codes for a particular game and emulator. Review compatibility first; a recognised code is not proof it works in-game.",
            *tab == Tab::Cheats,
            || *tab = Tab::Cheats,
        );
        lane_card(
            ui,
            width,
            "Mods",
            "Texture packs, patches and replacement files. Inspect local or provider-supplied packages before they touch an emulator folder.",
            *tab != Tab::Cheats,
            || *tab = Tab::Installed,
        );
    });
}

fn lane_card(
    ui: &mut egui::Ui,
    width: f32,
    title: &str,
    detail: &str,
    selected: bool,
    choose: impl FnOnce(),
) {
    let response = egui::Frame::new()
        .fill(if selected {
            theme::PRIMARY_ACTION.gamma_multiply(0.24)
        } else {
            theme::CARD_SURFACE
        })
        .stroke(egui::Stroke::new(
            if selected { 1.5_f32 } else { 1.0_f32 },
            if selected {
                theme::TEAL
            } else {
                theme::BORDER_SUBTLE
            },
        ))
        .corner_radius(8)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width((width - 22.0).max(1.0));
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(if title == "Cheats" { "CODE" } else { "PATCH" })
                            .color(theme::TEAL)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new(title)
                            .size(theme::SECTION_TITLE_SIZE)
                            .strong(),
                    );
                });
                ui.label(egui::RichText::new(detail).color(theme::muted(ui)));
                ui.label(
                    egui::RichText::new(if selected { "Selected" } else { "Open" }).color(
                        if selected {
                            theme::TEAL
                        } else {
                            theme::muted(ui)
                        },
                    ),
                );
            });
        })
        .response;
    let response = response.interact(egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, title));
    theme::paint_focus_ring(ui, &response, response.rect, 8.0);
    if response.clicked() {
        choose();
    }
}

fn selected_identity(game: Option<&crate::gui_v2::library::Game>) -> Option<String> {
    game.and_then(|game| game.archive.identity_report.as_ref())
        .and_then(|report| {
            match archivefs_core::launch::canonical_identity_from_game_report(report).0 {
                archivefs_core::launch::CanonicalIdentityStatus::Resolved(identity) => {
                    Some(identity.game_key)
                }
                _ => None,
            }
        })
}

fn for_game<'a>(
    history: &'a ModHistory,
    game: Option<&crate::gui_v2::library::Game>,
) -> Vec<&'a ModReceiptSummary> {
    match selected_identity(game) {
        Some(identity) => history
            .receipts
            .iter()
            .filter(|receipt| receipt.verified_identity.as_deref() == Some(identity.as_str()))
            .collect(),
        None if game.is_some() => Vec::new(),
        None => history.receipts.iter().collect(),
    }
}

fn installed(
    ui: &mut egui::Ui,
    history: Option<&ModHistory>,
    game: Option<&crate::gui_v2::library::Game>,
) {
    ui.heading("Installed mods");
    let Some(history) = history else {
        ui.label("Installed mod history is not available yet.");
        return;
    };
    let receipts = for_game(history, game);
    if receipts.is_empty() {
        ui.label("No matching installation records are available in this view. Mods installed outside EmuWiz may not be recorded here.");
        ui.label("Choose Available packages to inspect a local mod safely.");
        return;
    }
    for receipt in receipts {
        ui.push_id(&receipt.transaction_id, |ui| {
            receipt_card(ui, receipt);
        });
    }
}

fn receipt_card(ui: &mut egui::Ui, receipt: &ModReceiptSummary) {
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong(receipt.kind.label());
            widgets::status_badge(
                ui,
                receipt.rollback.label(),
                if matches!(
                    receipt.rollback,
                    archivefs_core::mod_history::ModRollbackStatus::ReadyToUndo
                ) {
                    widgets::StatusTone::Success
                } else {
                    widgets::StatusTone::Info
                },
            );
        });
        ui.label(format!(
            "{} · {} changed · {}",
            receipt.platform.as_deref().unwrap_or("Unknown system"),
            receipt.changed_files(),
            receipt.rollback.label()
        ));
        if let Some(source) = &receipt.source_package {
            ui.label(format!("Source: {source}"));
        }
        if !receipt.conflicts.is_empty() || !receipt.conflict_kinds.is_empty() {
            ui.colored_label(
                egui::Color32::YELLOW,
                "Two recorded changes may affect the same files. Review Conflicts and History before applying another package.",
            );
        }
        ui.label("This record shows what changed and whether you can undo it.");
        ui.collapsing("Details", |ui| {
            ui.label(format!("Transaction: {}", receipt.transaction_id));
            if let Some(identity) = &receipt.verified_identity {
                ui.label(format!("Verified game identity: {identity}"));
            }
            ui.label(format!("Files created: {}", receipt.files_created));
            ui.label(format!("Files replaced: {}", receipt.files_replaced));
            ui.label(format!("Files unchanged: {}", receipt.files_unchanged));
            ui.label(format!("Rollback: {}", receipt.rollback.label()));
            if let Some(reason) = match &receipt.rollback {
                archivefs_core::mod_history::ModRollbackStatus::CannotSafelyUndo { reason }
                | archivefs_core::mod_history::ModRollbackStatus::NeedsReview { reason } => {
                    Some(reason)
                }
                _ => None,
            } {
                ui.label(reason);
            }
            for path in receipt.affected_destinations.iter().take(20) {
                ui.monospace(path);
            }
        });
    });
}

fn add_package(
    ui: &mut egui::Ui,
    state: &mut LocalModPackagePageState,
    game: Option<&crate::gui_v2::library::Game>,
) {
    let Some(game) = game else {
        widgets::card(ui, |ui| {
            ui.heading("Select a game first");
            ui.label("Open a game from Games, then choose Mods & Cheats to inspect a compatible package.");
            ui.label("Browsing is safe: no original files are changed by opening this page.");
        });
        return;
    };
    let Some(identity) = game.archive.identity_report.as_ref() else {
        widgets::card(ui, |ui| {
            ui.heading("This game needs a verified identity");
            ui.label("EmuWiz will not guess which game a mod belongs to. Verify the game before installing a package.");
        });
        return;
    };
    widgets::card(ui, |ui| {
        ui.heading("Inspect a mod package");
        ui.label(format!("Game: {} · {}", game.title, game.platform));
        ui.label("Choose a local package or folder. Inspection is read-only and shows every file, conflict and compatibility decision before apply.");
    });
    show_local_mod_package_panel_with_catalogue(
        ui,
        state,
        Path::new(&game.archive.absolute_path),
        Some(identity),
        &[],
    );
}

fn stack(
    ui: &mut egui::Ui,
    history: Option<&ModHistory>,
    game: Option<&crate::gui_v2::library::Game>,
) {
    ui.heading("Recorded changes");
    ui.label("These are installation and undo records, not a live list of enabled mods or emulator load order. Already-undone changes remain visible in history.");
    let Some(history) = history else { return };
    let receipts = for_game(history, game);
    if receipts.is_empty() {
        ui.label("No matching mod changes are recorded in this view.");
    }
    for (index, receipt) in receipts.into_iter().enumerate() {
        ui.label(format!(
            "{}. {} — {}",
            index + 1,
            receipt.kind.label(),
            receipt.rollback.label()
        ));
    }
}

fn conflicts(
    ui: &mut egui::Ui,
    history: Option<&ModHistory>,
    game: Option<&crate::gui_v2::library::Game>,
) {
    ui.heading("Conflicts");
    let Some(history) = history else { return };
    let mut found = false;
    for receipt in for_game(history, game) {
        for conflict in &receipt.conflicts {
            found = true;
            ui.label("Two recorded changes affect the same destination. Review their history before replacing either file.");
            widgets::technical_details(
                ui,
                (
                    &receipt.transaction_id,
                    &conflict.transaction_id,
                    &conflict.destination,
                ),
                |ui| {
                    ui.label(format!(
                        "{} and {}",
                        receipt.transaction_id, conflict.transaction_id
                    ));
                    ui.monospace(&conflict.destination);
                },
            );
        }
        for kind in &receipt.conflict_kinds {
            found = true;
            ui.label(format!("Potential mod conflict: {kind}"));
        }
    }
    if !found {
        ui.label("No conflicts were found in the available mod records.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;

    #[test]
    fn history_refresh_is_single_flight_and_failure_retains_previous_evidence() {
        let (sender, receiver) = mpsc::channel();
        let previous = ModHistory {
            complete: true,
            ..Default::default()
        };
        let mut state = ModsPageState {
            history: Some(previous.clone()),
            history_loaded: true,
            history_worker: Some(receiver),
            ..Default::default()
        };
        state.refresh_history(&egui::Context::default());
        assert!(!state.poll());
        sender.send(Err("synthetic read failure".into())).unwrap();
        assert!(state.poll());
        assert_eq!(state.history, Some(previous));
        assert_eq!(
            state.history_error.as_deref(),
            Some("synthetic read failure")
        );
        assert!(state.history_worker.is_none());
    }

    #[test]
    fn disconnected_history_worker_becomes_a_retryable_error() {
        let (sender, receiver) = mpsc::channel();
        let mut state = ModsPageState {
            history_worker: Some(receiver),
            ..Default::default()
        };
        drop(sender);
        assert!(state.poll());
        assert!(state.history_error.is_some());
        assert!(state.history_worker.is_none());
    }

    #[test]
    fn workshop_choices_fit_narrow_and_desktop_content_widths() {
        for width in [380.0, 700.0, 1280.0] {
            let context = egui::Context::default();
            let mut tab = Tab::Installed;
            let _ = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 800.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let right = ui.max_rect().right();
                        workshop_lanes(ui, &mut tab);
                        assert!(
                            ui.min_rect().right() <= right + 1.0,
                            "choices overflow at {width}: {:?} exceeds {right}",
                            ui.min_rect()
                        );
                    });
                },
            );
            assert_eq!(tab, Tab::Installed);
        }
    }

    #[test]
    fn native_mod_page_explains_tabs_without_a_game() {
        let context = egui::Context::default();
        let mut state = ModsPageState::default();
        let mut activity = Activity::default();
        let mut workflows = NativeWorkflows::new(context.clone());
        let _ = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_mods_page(ui, &mut state, None, &mut workflows, &mut activity);
            });
        });
        assert_eq!(state.tab, Tab::Installed);
    }
}
