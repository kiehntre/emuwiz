//! Native GUI-v2 presentation for the existing mod and cheat backends.
//!
//! This module deliberately owns only navigation, presentation, and cached
//! history. Package inspection, compatibility decisions, transactions, and
//! rollback remain in the established mod pages and archivefs-core.

use std::path::Path;

use archivefs_core::mod_history::{ModHistory, ModReceiptSummary};
use eframe::egui;

use crate::local_mod_package_page::{
    LocalModPackagePageState, show_local_mod_package_panel_with_catalogue,
};
use crate::ui::components as widgets;
use super::{activity::Activity, routes::{Route, Section}};

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
    activity_id: Option<u64>,
}

impl ModsPageState {
    pub(super) fn poll(&mut self) -> bool {
        self.local.poll()
    }

    fn refresh_history(&mut self) {
        self.history_error = None;
        match (
            archivefs_core::patch_manager::default_shared_history_root(),
            archivefs_core::patch_manager::default_shared_backup_root(),
        ) {
            (Ok(history_root), Ok(backup_root)) => {
                self.history = Some(ModHistory::from_shared_history(&history_root, &backup_root));
            }
            (Err(error), _) | (_, Err(error)) => {
                self.history = None;
                self.history_error = Some(error.detail);
            }
        }
        self.history_loaded = true;
    }
}

pub(super) fn show_mods_page(
    ui: &mut egui::Ui,
    state: &mut ModsPageState,
    selected_game: Option<&crate::gui_v2::library::Game>,
    activity: &mut Activity,
) {
    if !state.history_loaded {
        state.refresh_history();
    }

    egui::ScrollArea::vertical()
        .id_salt("v2_mods_native")
        .show(ui, |ui| {
            ui.heading("Mods & Cheats");
            ui.label("Inspect changes before installing them. EmuWiz never changes a game until you confirm a safe preview.");

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

            match state.tab {
                Tab::Installed => installed(ui, state.history.as_ref(), selected_game),
                Tab::Add => add_package(ui, &mut state.local, selected_game),
                Tab::Stack => stack(ui, state.history.as_ref(), selected_game),
                Tab::Conflicts => conflicts(ui, state.history.as_ref(), selected_game),
                Tab::Cheats => cheats(ui),
            }

            if let Some(error) = &state.history_error {
                ui.collapsing("Advanced details", |ui| ui.label(error));
            }
            if ui.button("Refresh installed history").clicked() {
                state.refresh_history();
            }
        });

    let busy = state.local.is_busy();
    if busy && state.activity_id.is_none() {
        let id = activity.queue("Inspecting or applying a mod", Route::Section(Section::Mods), true);
        activity.start(id);
        state.activity_id = Some(id);
    } else if !busy && let Some(id) = state.activity_id.take() {
        activity.finish(id, "Mod workflow finished. Review the result and shared receipt above.".into(), None);
        state.refresh_history();
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
        ui.label("No installed mod transactions were found for this game.");
        ui.label("Choose Available packages to inspect a local mod safely.");
        return;
    }
    for receipt in receipts {
        receipt_card(ui, receipt);
    }
}

fn receipt_card(ui: &mut egui::Ui, receipt: &ModReceiptSummary) {
    widgets::card(ui, |ui| {
        ui.strong(receipt.kind.label());
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
            ui.colored_label(egui::Color32::YELLOW, "Potential mod conflict — review related transactions.");
        }
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
                | archivefs_core::mod_history::ModRollbackStatus::NeedsReview { reason } => Some(reason),
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
            ui.heading("Choose a game first");
            ui.label("Open a game from Games, then choose Mods & Cheats to inspect a compatible package.");
        });
        return;
    };
    let Some(identity) = game.archive.identity_report.as_ref() else {
        widgets::card(ui, |ui| {
            ui.heading("This game needs verified identity");
            ui.label("EmuWiz will not guess which game a mod belongs to. Verify the game before installing a package.");
        });
        return;
    };
    widgets::card(ui, |ui| {
        ui.heading("Add a local mod");
        ui.label(format!("Game: {} · {}", game.title, game.platform));
        ui.label("Choose a package or folder. Inspection is read-only and shows every file, conflict, and compatibility decision before apply.");
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
    ui.heading("Active stack");
    ui.label("Enabled mods are shown in the deterministic order recorded by the existing activation and transaction history.");
    let Some(history) = history else { return };
    let receipts = for_game(history, game);
    if receipts.is_empty() {
        ui.label("No active mod layers are recorded for this game.");
    }
    for (index, receipt) in receipts.into_iter().enumerate() {
        ui.label(format!("{}. {} — {}", index + 1, receipt.kind.label(), receipt.rollback.label()));
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
            ui.label(format!(
                "These transactions touch the same destination: {} and {}",
                receipt.transaction_id, conflict.transaction_id
            ));
            ui.monospace(&conflict.destination);
        }
        for kind in &receipt.conflict_kinds {
            found = true;
            ui.label(format!("Potential mod conflict: {kind}"));
        }
    }
    if !found {
        ui.label("No recorded mod conflicts need your attention.");
    }
}

fn cheats(ui: &mut egui::Ui) {
    widgets::card(ui, |ui| {
        ui.heading("Cheats");
        ui.label("Cheats stay separate from mods. Use the existing safe cheat workflow for the selected game.");
        ui.label("No cheat is changed by browsing this page.");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;

    #[test]
    fn native_mod_page_explains_tabs_without_a_game() {
        let context = egui::Context::default();
        let mut state = ModsPageState::default();
        let mut activity = Activity::default();
        let _ = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default()
                .show(ctx, |ui| show_mods_page(ui, &mut state, None, &mut activity));
        });
        assert_eq!(state.tab, Tab::Installed);
    }
}
