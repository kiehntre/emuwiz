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
use crate::cheatbase_page::{CheatBaseGameSeed, CheatBasePageState, show_cheatbase_browse_page};
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
    Activation,
    Cheats,
}

#[derive(Default)]
pub(super) struct ModsPageState {
    tab: Tab,
    local: LocalModPackagePageState,
    cheatbase: CheatBasePageState,
    cheatbase_game_id: Option<i64>,
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
    let game_id = selected_game.map(|game| game.archive.id);
    if state.cheatbase_game_id != game_id {
        state.cheatbase = CheatBasePageState::default();
        state.cheatbase_game_id = game_id;
    }
    state.poll();
    state.cheatbase.poll(ui.ctx());
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
                (Tab::Stack, "Recorded changes"),
                (Tab::Conflicts, "Conflicts"),
                (Tab::Activation, "Enable / disable"),
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
                ui.colored_label(theme::warning(ui), "Some change records could not be read. This is an incomplete view; refresh history before relying on it for recovery.");
                widgets::technical_details(ui, "incomplete_mod_history", |ui| {
                    for problem in &history.problems { ui.label(problem); }
                });
            }
            ui.horizontal_wrapped(|ui| {
                if selected_game.is_none() && ui.button("Choose a game").clicked() { destination = Some(Route::Section(Section::Games)); }
                if let Some(game) = selected_game {
                    if selected_identity(Some(game)).is_none() {
                        if game.dat_exact.is_some() {
                            // Verified by its saved exact match, but mod/cheat matching keys on
                            // the game's own identity data, which this file does not provide.
                            ui.label("This game is verified. Matching its mods and cheats also needs the game's own identity data (such as a title ID), which EmuWiz could not read from this file.");
                            if ui.button("View evidence").clicked() { destination = Some(Route::ReviewIdentity(game.archive.id)); }
                        } else {
                            ui.label("We need to identify this game before cheats or mods can be matched safely.");
                            if ui.button("Review identity").clicked() { destination = Some(Route::ReviewIdentity(game.archive.id)); }
                        }
                    }
                }
                if ui.button("Open History & Undo").clicked() { destination = Some(Route::Section(Section::History)); }
            });
            if state.tab == Tab::Cheats && selected_game.is_some() {
                ui.label("Recognised code syntax does not prove a cheat works in-game. Check the game version, region and emulator before previewing any supported change; installing a file is not proof that a cheat is active.");
            }
            if state.tab == Tab::Cheats && selected_game.is_none() {
                destination = workflows.show_cheats(ui, selected_game, activity).or(destination.take());
                show_cheatbase(ui, &mut state.cheatbase, selected_game);
            } else {
                selected_game_strip(ui, selected_game);
                glance_card(
                    ui,
                    &glance(
                        state.history.as_ref(),
                        &game_scope(selected_game),
                        state.history_worker.is_some(),
                        state.history_error.is_some(),
                    ),
                );
                workshop_lanes(ui, &mut state.tab);
                match state.tab {
                    Tab::Installed => installed(ui, state.history.as_ref(), selected_game),
                    Tab::Add => add_package(ui, &mut state.local, selected_game),
                    Tab::Stack => stack(ui, state.history.as_ref(), selected_game),
                    Tab::Conflicts => conflicts(ui, state.history.as_ref(), selected_game),
                    Tab::Activation => {
                        destination = workflows.show_mod_activation(ui, selected_game, activity).or(destination.take());
                    }
                    Tab::Cheats => {
                        destination = workflows.show_cheats(ui, selected_game, activity).or(destination.take());
                        show_cheatbase(ui, &mut state.cheatbase, selected_game);
                    }
                }
            }

            if let Some(error) = &state.history_error {
                ui.colored_label(theme::warning(ui), "Change history could not be refreshed. Previous results may be incomplete. Retry Refresh history; no mods were changed by this check.");
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

fn cheatbase_seed(game: Option<&crate::gui_v2::library::Game>) -> Option<CheatBaseGameSeed> {
    let game = game?;
    // Use current verified game evidence; an incomplete scan makes saved
    // identity historical, so it cannot seed a new catalogue match.
    if game.inspection_incomplete || !(game.identified || game.dat_exact.is_some()) {
        return None;
    }
    let accepted = game.screenscraper.as_ref().map(|entry| &entry.values);
    Some(CheatBaseGameSeed {
        title: accepted
            .and_then(|values| values.title.clone())
            .unwrap_or_else(|| game.title.clone()),
        platform: game.archive.platform.clone(),
        region: accepted.and_then(|values| values.region.clone()),
    })
}

fn show_cheatbase(
    ui: &mut egui::Ui,
    state: &mut CheatBasePageState,
    game: Option<&crate::gui_v2::library::Game>,
) {
    ui.separator();
    ui.label("CheatBase is a read-only catalogue. A title match does not verify the game's revision or activate a cheat.");
    if game.is_some() && cheatbase_seed(game).is_none() {
        ui.label("This game's current identity is not verified. Search CheatBase manually if you want to inspect its catalogue.");
    }
    if let Some(action) = show_cheatbase_browse_page(ui, state, cheatbase_seed(game))
        && matches!(
            action,
            crate::cheatbase_page::CheatBasePageAction::Refresh
                | crate::cheatbase_page::CheatBasePageAction::Search
                | crate::cheatbase_page::CheatBasePageAction::Inspect(_)
        )
    {
        state.handle(action, ui.ctx().clone());
    }
}

/// Which part of the history can be attributed to the selected game.
#[derive(Clone, Debug, PartialEq, Eq)]
enum GameScope {
    NoGame,
    IdentityUnavailable,
    Identified(String),
}

fn game_scope(game: Option<&crate::gui_v2::library::Game>) -> GameScope {
    match (game, selected_identity(game)) {
        (None, _) => GameScope::NoGame,
        (Some(_), None) => GameScope::IdentityUnavailable,
        (Some(_), Some(identity)) => GameScope::Identified(identity),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GlanceCaveat {
    /// Some records could not be read: counts are a lower bound.
    Incomplete,
    /// A refresh is running or failed: counts may be out of date.
    MayBeStale,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Glance {
    NeedGame,
    NeedIdentity,
    Loading,
    Unavailable,
    Counts {
        recorded: usize,
        undo_candidates: usize,
        with_conflicts: usize,
        caveat: Option<GlanceCaveat>,
    },
}

/// Summarises existing ModHistory evidence for the selected game. Never turns
/// "unknown" into a zero: without a game, a usable game identity or loaded
/// history there are no counts at all.
fn glance(
    history: Option<&ModHistory>,
    scope: &GameScope,
    loading: bool,
    refresh_failed: bool,
) -> Glance {
    let identity = match scope {
        GameScope::NoGame => return Glance::NeedGame,
        GameScope::IdentityUnavailable => return Glance::NeedIdentity,
        GameScope::Identified(identity) => identity,
    };
    let Some(history) = history else {
        return if loading {
            Glance::Loading
        } else {
            Glance::Unavailable
        };
    };
    let receipts: Vec<&ModReceiptSummary> = history
        .receipts
        .iter()
        .filter(|receipt| receipt.verified_identity.as_deref() == Some(identity.as_str()))
        .collect();
    let caveat = if !history.complete {
        Some(GlanceCaveat::Incomplete)
    } else if loading || refresh_failed {
        Some(GlanceCaveat::MayBeStale)
    } else {
        None
    };
    Glance::Counts {
        recorded: receipts.len(),
        undo_candidates: receipts
            .iter()
            .filter(|r| {
                matches!(
                    r.rollback,
                    archivefs_core::mod_history::ModRollbackStatus::ReadyToUndo
                )
            })
            .count(),
        with_conflicts: receipts
            .iter()
            .filter(|r| !r.conflicts.is_empty() || !r.conflict_kinds.is_empty())
            .count(),
        caveat,
    }
}

fn glance_card(ui: &mut egui::Ui, glance: &Glance) {
    widgets::card(ui, |ui| {
        ui.strong("Mods at a glance");
        match glance {
            Glance::NeedGame => {
                ui.label("Choose a game to see which changes EmuWiz has recorded for it.");
            }
            Glance::NeedIdentity => {
                ui.label("This game's identity is unavailable, so recorded changes cannot be matched to it. No counts are shown.");
            }
            Glance::Loading => {
                ui.label("Reading change history… counts will appear when it has loaded.");
            }
            Glance::Unavailable => {
                ui.label("Change history is not available, so counts are unknown. Use Refresh history below.");
            }
            Glance::Counts {
                recorded,
                undo_candidates,
                with_conflicts,
                caveat,
            } => {
                let prefix = if *caveat == Some(GlanceCaveat::Incomplete) {
                    "at least "
                } else {
                    ""
                };
                ui.horizontal_wrapped(|ui| {
                    widgets::info_chip(ui, &format!("Recorded changes: {prefix}{recorded}"));
                    widgets::info_chip(ui, &format!("Undo candidates: {prefix}{undo_candidates}"));
                    widgets::info_chip(
                        ui,
                        &format!("With recorded conflicts: {prefix}{with_conflicts}"),
                    );
                });
                match caveat {
                    Some(GlanceCaveat::Incomplete) => {
                        ui.colored_label(
                            theme::warning(ui),
                            "Some change records could not be read, so these are minimum counts.",
                        );
                    }
                    Some(GlanceCaveat::MayBeStale) => {
                        ui.colored_label(theme::warning(ui), "History is refreshing or could not be refreshed; these counts may be out of date.");
                    }
                    None => {}
                }
                ui.label("These are records of changes EmuWiz made, not proof that a mod is active or loaded by an emulator. An undo candidate still needs review and confirmation.");
            }
        }
    });
}

fn selected_game_strip(ui: &mut egui::Ui, game: Option<&crate::gui_v2::library::Game>) {
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            // A painted diamond: the "◈" glyph is missing from egui's default fonts.
            let (mark, _) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
            let c = mark.center();
            ui.painter().add(egui::Shape::closed_line(
                vec![
                    c + egui::vec2(0.0, -10.0),
                    c + egui::vec2(10.0, 0.0),
                    c + egui::vec2(0.0, 10.0),
                    c + egui::vec2(-10.0, 0.0),
                ],
                egui::Stroke::new(2.0_f32, theme::accent(ui)),
            ));
            ui.painter().circle_filled(c, 3.0, theme::accent(ui));
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
            theme::card_fill(ui)
        })
        .stroke(egui::Stroke::new(
            if selected { 1.5_f32 } else { 1.0_f32 },
            if selected {
                theme::accent(ui)
            } else {
                theme::border_color(ui)
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
                            .color(theme::accent(ui))
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
                            theme::accent(ui)
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
    game.and_then(|game| {
        game.archive
            .identity_report
            .as_ref()
            .map(|report| (game, report))
    })
    .and_then(|(game, report)| {
        // An incomplete inspection makes the saved report history, not proof.
        let freshness = archivefs_core::launch::EvidenceFreshness::from_stored_health(
            &game.archive.last_known_health,
        );
        match archivefs_core::launch::canonical_identity_for_saved_report(report, freshness).0 {
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
    ui.label("For supported local RPCS3 runtime patches, use Enable / disable to inspect installed definitions and preview their configuration. File-package installation records below do not establish an enabled state.");
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
    ui.label("Changing the order of these records would not change emulator precedence. Load-order editing is unavailable for these adapters.");
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
    fn verified_game_seeds_cheatbase_without_inventing_region() {
        let fixture = crate::gui_v2::verified_game_fixture::VerifiedGame::new();
        let game = fixture.game().expect("verified game");
        let seed = cheatbase_seed(Some(&game)).expect("verified seed");
        assert_eq!(seed.title, game.title);
        assert_eq!(seed.platform, game.archive.platform);
        assert_eq!(seed.region, None);

        let mut stale = game;
        stale.inspection_incomplete = true;
        assert!(cheatbase_seed(Some(&stale)).is_none());
        stale.inspection_incomplete = false;
        stale.identified = false;
        stale.dat_exact = None;
        assert!(cheatbase_seed(Some(&stale)).is_none());
    }

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

    #[test]
    fn activation_tab_requires_a_game_and_does_not_open_a_write_workflow() {
        let context = egui::Context::default();
        let mut state = ModsPageState {
            tab: Tab::Activation,
            history_loaded: true,
            ..Default::default()
        };
        let mut activity = Activity::default();
        let mut workflows = NativeWorkflows::new(context.clone());
        assert!(workflows.app.cheat_workflow.is_none());
        let output = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert_eq!(
                    show_mods_page(ui, &mut state, None, &mut workflows, &mut activity),
                    None
                );
            });
        });
        let text = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Enable / disable supported runtime patches"));
        assert!(text.contains("Choose a game before inspecting"));
        assert!(workflows.app.cheat_workflow.is_none());
        assert!(state.history_worker.is_none());
    }

    fn receipt(
        id: &str,
        identity: &str,
        rollback: archivefs_core::mod_history::ModRollbackStatus,
        conflicted: bool,
    ) -> ModReceiptSummary {
        let mut r: ModReceiptSummary = serde_json::from_value(serde_json::json!({
            "schema_version": 1, "transaction_id": id, "kind": "local_package",
            "platform": null, "emulator": null, "game_title": null,
            "verified_identity": identity, "source_package": null, "source_fingerprint": null,
            "installed_at_unix": 0, "files_created": 0, "files_replaced": 0, "files_unchanged": 0,
            "destination_root": null, "journal_path": null, "backup_root": null,
            "rollback": "ReadyToUndo", "conflicts": [], "notes": []
        }))
        .expect("receipt fixture");
        r.rollback = rollback;
        if conflicted {
            r.conflict_kinds = vec!["same destination".into()];
        }
        r
    }

    fn scope() -> GameScope {
        GameScope::Identified("game-a".into())
    }

    #[test]
    fn glance_never_turns_unknown_into_zero() {
        let empty = ModHistory {
            complete: true,
            ..Default::default()
        };
        assert_eq!(
            glance(Some(&empty), &GameScope::NoGame, false, false),
            Glance::NeedGame
        );
        assert_eq!(
            glance(Some(&empty), &GameScope::IdentityUnavailable, false, false),
            Glance::NeedIdentity
        );
        assert_eq!(glance(None, &scope(), true, false), Glance::Loading);
        assert_eq!(glance(None, &scope(), false, true), Glance::Unavailable);
        assert!(matches!(
            glance(Some(&empty), &scope(), false, false),
            Glance::Counts {
                recorded: 0,
                caveat: None,
                ..
            }
        ));
    }

    #[test]
    fn glance_counts_only_the_selected_games_recorded_evidence() {
        use archivefs_core::mod_history::ModRollbackStatus as R;
        let history = ModHistory {
            complete: true,
            receipts: vec![
                receipt("1", "game-a", R::ReadyToUndo, false),
                receipt("2", "game-a", R::AlreadyUndone, true),
                receipt("3", "game-a", R::NeedsReview { reason: "x".into() }, false),
                receipt("4", "game-b", R::ReadyToUndo, true),
            ],
            problems: vec![],
        };
        assert_eq!(
            glance(Some(&history), &scope(), false, false),
            Glance::Counts {
                recorded: 3,
                undo_candidates: 1,
                with_conflicts: 1,
                caveat: None
            }
        );
    }

    #[test]
    fn glance_flags_incomplete_and_stale_history() {
        let incomplete = ModHistory {
            complete: false,
            ..Default::default()
        };
        assert!(matches!(
            glance(Some(&incomplete), &scope(), false, false),
            Glance::Counts {
                caveat: Some(GlanceCaveat::Incomplete),
                ..
            }
        ));
        let complete = ModHistory {
            complete: true,
            ..Default::default()
        };
        for (loading, failed) in [(true, false), (false, true)] {
            assert!(matches!(
                glance(Some(&complete), &scope(), loading, failed),
                Glance::Counts {
                    caveat: Some(GlanceCaveat::MayBeStale),
                    ..
                }
            ));
        }
    }

    #[test]
    fn glance_card_fits_1024x600_at_160_percent_and_labels_the_tab() {
        // 1024x600 at 160% scaling is a 640x375 logical viewport.
        let history = ModHistory {
            complete: false,
            ..Default::default()
        };
        for g in [
            glance(Some(&history), &scope(), false, false),
            Glance::NeedIdentity,
            Glance::Loading,
        ] {
            let context = egui::Context::default();
            let _ = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 375.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    ctx.set_pixels_per_point(1.6);
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let right = ui.max_rect().right();
                        glance_card(ui, &g);
                        assert!(ui.min_rect().right() <= right + 1.0, "{g:?} overflows");
                    });
                },
            );
        }
        let source = include_str!("mods.rs");
        assert!(source.contains("(Tab::Stack, \"Recorded changes\")"));
        assert!(!source.contains(&format!("(Tab::Stack, \"{} {}\")", "Active", "stack")));
    }
}
