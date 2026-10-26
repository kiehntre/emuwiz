//! Native, read-only Saves & States projection.
//!
//! The worker owns inventory I/O. This module only translates the provider
//! neutral records into a compact, beginner-friendly view. Restore/export
//! actions use the established PS2 Save Vault embedded in this Blue page.

use super::{
    App,
    imagery::{EmptyArt, empty_state},
    library::Game,
    routes::{Route, Section},
};
use crate::ui::{
    components::{StatusTone, page_hero, status_badge},
    theme,
};
use archivefs_core::persistent_state_inventory::StateEmulator;
use archivefs_core::persistent_state_inventory::{
    PersistentStateInventory, PersistentStateRecord, PersistentStateRoot, PersistentStateType,
    PortabilityClass,
};
use eframe::egui;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Filter {
    #[default]
    All,
    GameSaves,
    MemoryCards,
    SaveStates,
    SystemStorage,
    NeedsAttention,
}

#[derive(Default)]
pub(super) struct SavesStatesState {
    pub(super) inventory: Option<PersistentStateInventory>,
    pub(super) vault_open: bool,
    pub(super) vault_advanced: bool,
    pub(super) loading: bool,
    pub(super) job: Option<u64>,
    pub(super) generation: u64,
    pub(super) observed_completion: u64,
    pub(super) filter: Filter,
    pub(super) search: String,
    pub(super) error: Option<String>,
    pub(super) snapshots: super::save_snapshots::SnapshotPanelState,
}

impl SavesStatesState {
    fn filters_active(&self) -> bool {
        self.filter != Filter::All || !self.search.trim().is_empty()
    }
    fn clear_filters(&mut self) {
        self.filter = Filter::All;
        self.search.clear();
    }
}

/// Completion is published only after the reaper commits its receipt. A
/// restart uses the same worker projection on the first inventory request.
pub(super) fn poll_completed_sessions(app: &mut App) {
    let revision = archivefs_core::sram_provenance::completion_revision();
    if revision != app.saves_states.observed_completion && !app.saves_states.loading {
        app.saves_states.observed_completion = revision;
        app.start_saves_inventory();
    }
}

pub(super) fn configured_roots() -> Vec<PersistentStateRoot> {
    // GUI rendering tests must never inherit the developer's remembered
    // emulator roots. Production still uses the exact remembered/configured
    // roots below; tests supply inventory fixtures directly at the core seam.
    #[cfg(test)]
    return Vec::new();

    #[cfg(not(test))]
    configured_roots_from_environment()
}

#[cfg(not(test))]
fn configured_roots_from_environment() -> Vec<PersistentStateRoot> {
    let mut roots = archivefs_core::patch_manager::load_remembered_emulator_profiles_default()
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|profile| {
            let emulator = match profile.adapter.as_str() {
                "retroarch" => StateEmulator::RetroArch,
                "duckstation" => StateEmulator::DuckStation,
                "pcsx2" => StateEmulator::Pcsx2,
                "rpcs3" => StateEmulator::Rpcs3,
                "ppsspp" => StateEmulator::Ppsspp,
                "dolphin" => StateEmulator::Dolphin,
                "flycast" => StateEmulator::Flycast,
                "mame" => StateEmulator::Mame,
                "hatari" => StateEmulator::Hatari,
                "fs-uae" => StateEmulator::FsUae,
                "xemu" => StateEmulator::Xemu,
                "xenia" => StateEmulator::Xenia,
                "cemu" => StateEmulator::Cemu,
                "vita3k" => StateEmulator::Vita3k,
                _ => StateEmulator::Unknown,
            };
            (emulator != StateEmulator::Unknown).then(|| {
                let mut root = PersistentStateRoot::configured(emulator, profile.root, None);
                root.installation = Some(
                    archivefs_core::persistent_state_inventory::StateInstallation {
                        installation_id: profile.profile_id,
                        executable: None,
                        version: None,
                        profile: None,
                        firmware_context: None,
                        selected: true,
                    },
                );
                root
            })
        })
        .collect::<Vec<_>>();
    if let Some(card) = crate::pcsx2_page::load_saved_ps2_card_path() {
        roots.push(PersistentStateRoot::configured(
            StateEmulator::Pcsx2,
            card,
            Some(PersistentStateType::MemoryCard),
        ));
    }
    roots
}

/// What the embedded, explicitly confirmed restore workflow supports.
pub(super) const SAVE_VAULT_PURPOSE: &str = "Restore PS2 memory-card saves in the embedded Save Vault. Every restore shows a preview, backs up and verifies the card first, and asks before replacing an existing save. It refuses while the emulator is running or the card changed.";

pub(super) fn show(app: &mut App, ui: &mut egui::Ui, game_id: Option<i64>) {
    let selected_game = game_id.and_then(|id| app.library.game(id)).cloned();
    let state_target = selected_game.as_ref().and_then(|game| {
        app.native_workflows.as_ref().and_then(|workflows| {
            workflows.save_state_target(game.archive.id, &game.archive.absolute_path)
        })
    });
    let loading = app.saves_states.loading;
    let has_inventory = app.saves_states.inventory.is_some();
    let counts = app.saves_states.inventory.as_ref().map(|inventory| {
        [
            inventory
                .records
                .iter()
                .filter(|record| {
                    selected_game
                        .as_ref()
                        .is_none_or(|game| record_belongs_to_game(record, game))
                })
                .filter(|record| record.state_type == PersistentStateType::NativeSave)
                .count(),
            inventory
                .records
                .iter()
                .filter(|record| {
                    selected_game
                        .as_ref()
                        .is_none_or(|game| record_belongs_to_game(record, game))
                })
                .filter(|record| record.state_type == PersistentStateType::MemoryCard)
                .count(),
            inventory
                .records
                .iter()
                .filter(|record| {
                    selected_game
                        .as_ref()
                        .is_none_or(|game| record_belongs_to_game(record, game))
                })
                .filter(|record| record.state_type == PersistentStateType::SaveState)
                .count(),
        ]
    });
    let mut refresh = false;
    page_hero(
        ui,
        checkpoint_motif,
        if app.saves_states.vault_open {
            "PS2 Save Vault"
        } else if selected_game.is_some() {
            "Review this game's saved progress"
        } else {
            "Review your saved progress"
        },
        if app.saves_states.vault_open {
            "Review card operations, verified backups and recovery records."
        } else if selected_game.is_some() {
            "Review explicitly associated progress and restore evidence."
        } else {
            "Your progress, preserved safely."
        },
        Some((
            if app.saves_states.vault_open {
                "Explicit confirmation required"
            } else if loading {
                "Inspecting save locations"
            } else {
                "Read-only inspection"
            },
            if loading {
                StatusTone::Active
            } else {
                StatusTone::Info
            },
        )),
        Some(if app.saves_states.vault_open {
            "Restore and Undo may change a card only after you review and confirm."
        } else {
            "Checkpoints stay yours. Inspection never changes them."
        }),
        |ui| {
            let lines = if let Some([saves, cards, states]) = counts {
                vec![
                    "CHECKPOINT INDEX".to_string(),
                    format!("SAVES   {saves:>3}"),
                    format!("CARDS   {cards:>3}"),
                    format!("STATES  {states:>3}"),
                ]
            } else {
                vec![
                    "CHECKPOINT INDEX".to_string(),
                    "WAITING FOR INSPECTION".to_string(),
                ]
            };
            let line_color = theme::accent(ui).gamma_multiply(0.35);
            crate::ui::components::signal_panel(
                ui,
                egui::vec2(180.0, 86.0),
                &lines,
                |painter, rect| {
                    painter.line_segment(
                        [
                            egui::pos2(rect.left(), rect.center().y),
                            egui::pos2(rect.right(), rect.center().y),
                        ],
                        egui::Stroke::new(1.0_f32, line_color),
                    );
                },
            );
        },
        |ui| {
            if ui
                .add_enabled(
                    !loading,
                    egui::Button::new(if has_inventory {
                        "Refresh save locations"
                    } else {
                        "Inspect save locations"
                    })
                    .fill(theme::primary_action_fill(ui))
                    .min_size(egui::vec2(190.0, 40.0)),
                )
                .clicked()
            {
                refresh = true;
            }
            if ui
                .button("Open Emulator Setup")
                .on_hover_text(
                    "Opens Emulator Setup. Restoring saves is not available on this page.",
                )
                .clicked()
            {
                app.go(Route::Section(Section::Emulators));
            }
            // Restoring happens in the established Save Vault, in its own
            // window; this page stays read-only.
            if ui
                .button("Open PS2 Save Vault")
                .on_hover_text(SAVE_VAULT_PURPOSE)
                .clicked()
            {
                app.saves_states.vault_open = true;
            }
        },
    );
    if app.saves_states.vault_open {
        ui.horizontal(|ui| {
            ui.heading("Save Vault");
            if ui.button("Back to save inventory").clicked() {
                app.saves_states.vault_open = false;
            }
        });
        ui.label("PS2 memory-card operations use the existing restore records and verified-backup workflow. Other save types remain inspection-only; selecting a game does not manufacture save identity.");
        ui.checkbox(
            &mut app.saves_states.vault_advanced,
            "Show advanced restore history and Undo",
        );
        let advanced = app.saves_states.vault_advanced;
        let workflows = app
            .native_workflows
            .get_or_insert_with(|| super::native_workflows::NativeWorkflows::new(ui.ctx().clone()));
        workflows.show_save_vault(ui, advanced);
        return;
    }
    if refresh && !app.saves_states.loading {
        app.start_saves_inventory();
    }

    if loading {
        ui.spinner();
        ui.label("Checking save locations in the background. You can keep browsing; another check can start when this one finishes.");
    }
    if let Some(error) = &app.saves_states.error {
        ui.colored_label(theme::warning(ui), "Save locations could not be checked. Reconnect the drive or review Emulator Setup, then refresh. Your saves were not changed.");
        ui.collapsing("Save inspection details", |ui| {
            ui.label(error);
        });
    }

    if let Some(game) = &selected_game {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.strong("Selected game");
            ui.label(format!("{} · {}", game.title, game.platform));
            ui.label("Only save evidence with an explicit identity match is shown here. Filename-only matches stay in the review view.");
            ui.label("Snapshots are local copies kept by EmuWiz. Making one only reads your save; restoring is not offered here.");
            if ui.button("Back to Game Details").clicked() {
                app.go(Route::Game(game.archive.id));
            }
        });
        ui.add_space(theme::SPACE_SM);
    }

    let Some(inventory) = app.saves_states.inventory.as_ref() else {
        if !app.saves_states.loading
            && empty_state(
                ui,
                &mut app.imagery,
                EmptyArt::Glyph("cartridge"),
                "No saves checked yet",
                "EmuWiz is ready to look for game saves, memory cards and savestates in your configured emulator locations. Inspection never changes them.",
                Some("Refresh save locations"),
            )
        {
            app.start_saves_inventory();
        }
        return;
    };
    let records: Vec<_> = inventory
        .records
        .iter()
        .filter(|record| {
            selected_game
                .as_ref()
                .is_none_or(|game| record_belongs_to_game(record, game))
        })
        .collect();
    summary(ui, &records);
    if let Some(game) = &selected_game {
        let targets = records
            .iter()
            .filter_map(|record| super::save_snapshots::snapshot_target(record, game).ok())
            .collect();
        super::save_snapshots::ensure_history(app, game.archive.id, targets);
        snapshot_overview(ui, &app.saves_states.snapshots);
    }
    let Some(inventory) = app.saves_states.inventory.as_ref() else {
        return;
    };
    let records: Vec<_> = inventory
        .records
        .iter()
        .filter(|record| {
            selected_game
                .as_ref()
                .is_none_or(|game| record_belongs_to_game(record, game))
        })
        .collect();
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        for (filter, label) in [
            (Filter::All, "All"),
            (Filter::GameSaves, "Game saves"),
            (Filter::MemoryCards, "Memory cards"),
            (Filter::SaveStates, "Savestates"),
            (Filter::SystemStorage, "System storage"),
            (Filter::NeedsAttention, "Needs attention"),
        ] {
            if ui
                .selectable_label(app.saves_states.filter == filter, label)
                .clicked()
            {
                app.saves_states.filter = filter;
            }
        }
        ui.add(
            egui::TextEdit::singleline(&mut app.saves_states.search)
                .hint_text("Search game, platform or emulator"),
        );
    });

    let search = app.saves_states.search.to_ascii_lowercase();
    let records: Vec<_> = records
        .into_iter()
        .filter(|record| {
            selected_game
                .as_ref()
                .is_none_or(|game| record_belongs_to_game(record, game))
                && filter_matches(record, app.saves_states.filter)
                && (search.is_empty() || record_matches(record, &search))
        })
        .collect();
    if records.is_empty() {
        let filtered = app.saves_states.filters_active();
        let (title, detail) = if filtered {
            (
                "No saves match these filters",
                "This does not mean your saves are missing. Clear the search and filters to see all records in this view.",
            )
        } else if selected_game.is_some() {
            (
                "No saves found for this game",
                "No save or state record has explicit identity evidence for the selected game. Review unassigned records from the full inventory if needed.",
            )
        } else if inventory.roots_inspected == 0 {
            (
                "No saves found yet",
                "Set up an emulator before EmuWiz can find its saves.",
            )
        } else if inventory.warnings.is_empty() {
            (
                "No saves found yet",
                "EmuWiz looks for game saves, memory cards and savestates in the configured emulator locations. None were found yet; inspection never changes them.",
            )
        } else {
            (
                "No saves found yet",
                "Your configured save location is unavailable or needs review.",
            )
        };
        if empty_state(
            ui,
            &mut app.imagery,
            EmptyArt::Glyph("cartridge"),
            title,
            detail,
            filtered.then_some("Clear search and filters"),
        ) {
            app.saves_states.clear_filters();
            return;
        }
        if selected_game.is_some()
            && ui
                .button("Review all saves, including unassigned")
                .clicked()
        {
            app.go(Route::Section(Section::Saves));
            return;
        }
    }
    let records: Vec<PersistentStateRecord> = records.into_iter().cloned().collect();
    let inventory_warnings = inventory.warnings.clone();
    egui::ScrollArea::vertical()
        .id_salt(("v2_saves_states_records", game_id))
        .show(ui, |ui| {
            for record in &records {
                ui.push_id(&record.path, |ui| {
                    record_card(ui, record, &mut app.imagery);
                    super::save_state_guidance::show(ui, record, state_target.as_ref());
                    if let Some(game) = &selected_game {
                        let target = super::save_snapshots::snapshot_target(record, game);
                        super::save_snapshots::show(
                            ui,
                            app,
                            record,
                            target.as_ref().map_err(|reason| *reason),
                        );
                    }
                });
            }
        });
    if !inventory_warnings.is_empty() {
        ui.colored_label(theme::warning(ui), "Some save locations could not be fully checked. This list may be incomplete; review Emulator Setup and refresh after correcting the locations.");
        ui.collapsing("Advanced inventory details", |ui| {
            for warning in &inventory_warnings {
                ui.label(warning);
            }
        });
    }
}

/// Game-page snapshot status: last result and unreadable snapshot entries.
fn snapshot_overview(ui: &mut egui::Ui, panel: &super::save_snapshots::SnapshotPanelState) {
    if let Some((ok, message)) = &panel.feedback {
        ui.colored_label(
            if *ok {
                theme::success(ui)
            } else {
                theme::warning(ui)
            },
            message,
        );
    }
    if let Some(report) = &panel.report
        && !report.warnings.is_empty()
    {
        ui.collapsing("Some snapshots could not be read", |ui| {
            ui.label("They were left exactly as they are.");
            for warning in &report.warnings {
                ui.label(warning);
            }
        });
    }
}

fn summary(ui: &mut egui::Ui, records: &[&PersistentStateRecord]) {
    let count = |kind| {
        records
            .iter()
            .filter(|record| record.state_type == kind)
            .count()
    };
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (label, value, detail) in [
                (
                    "Save data",
                    count(PersistentStateType::NativeSave),
                    "normal progress / Savedata",
                ),
                (
                    "Save states",
                    count(PersistentStateType::SaveState),
                    "emulator snapshots",
                ),
                (
                    "Memory cards",
                    count(PersistentStateType::MemoryCard),
                    "container storage",
                ),
                ("Backups", 0, "no backup records in this inventory"),
            ] {
                ui.vertical(|ui| {
                    ui.strong(format!("{value} {label}"));
                    ui.label(egui::RichText::new(detail).color(theme::muted(ui)).small());
                });
            }
        });
    });
}

fn record_belongs_to_game(record: &PersistentStateRecord, game: &Game) -> bool {
    if record.state_type == PersistentStateType::NativeSave
        && record.emulator == StateEmulator::RetroArch
    {
        if let Some(origin) = record.sram_origin.as_ref() {
            // The worker revalidated the sealed content digest and exact path.
            // No UI title or filename participates in this join.
            use archivefs_core::game_identity::{IdentityKind, IdentityStatus};
            let gold = &origin.context.cartridge_identity;
            let digest = origin.context.cartridge.sha256.as_str();
            // A DAT-only library row need not cache SHA256. The authority is
            // current, sealed Gold byte evidence from the inventory worker.
            // Any redundant strong cache evidence still refuses disagreement.
            let mut cached = game
                .dat_exact_hashes
                .as_ref()
                .and_then(|h| h.sha256.as_deref())
                .into_iter()
                .chain(
                    game.archive
                        .identity_report
                        .as_ref()
                        .into_iter()
                        .flat_map(|r| r.evidence.iter())
                        .filter(|e| {
                            e.kind == IdentityKind::LooseRomSha256
                                && e.status == IdentityStatus::Verified
                        })
                        .filter_map(|e| e.value.as_deref()),
                );
            return origin.state == archivefs_core::sram_provenance::OriginState::Associated
                && gold.kind == IdentityKind::LooseRomSha256
                && gold.status == IdentityStatus::Verified
                && gold.value.as_deref() == Some(digest)
                && !game.inspection_incomplete
                && cached.all(|value| value == digest)
                && origin.context.cartridge.path == game.archive.absolute_path;
        }
    }
    let Some(report) = game.archive.identity_report.as_ref() else {
        return false;
    };
    if record.state_type == PersistentStateType::SaveState {
        return archivefs_core::save_state_compatibility::same_game(
            &record.game_identity,
            &report.evidence,
        );
    }
    // Preserve main's ordinary-save association and management behavior.
    report.evidence.iter().any(|game_identity| {
        game_identity.status == archivefs_core::game_identity::IdentityStatus::Verified
            && game_identity.value.is_some()
            && record.game_identity.iter().any(|record_identity| {
                record_identity.status == archivefs_core::game_identity::IdentityStatus::Verified
                    && record_identity.kind == game_identity.kind
                    && record_identity.value == game_identity.value
            })
    })
}

fn compatibility_label(record: &PersistentStateRecord) -> (&'static str, StatusTone) {
    if record.state_type == PersistentStateType::SaveState {
        return ("Unknown", StatusTone::Warning);
    }
    if !record.warnings.is_empty() || record.portability_class == PortabilityClass::DoNotTouch {
        ("Restore unavailable", StatusTone::Warning)
    } else {
        match record.portability_class {
            PortabilityClass::SafeToCopy => ("Ready to preview", StatusTone::Success),
            PortabilityClass::CopyWithMetadata => ("Backup required", StatusTone::Warning),
            PortabilityClass::VersionBound | PortabilityClass::EmulatorBound => {
                ("Compatibility needs review", StatusTone::Warning)
            }
            PortabilityClass::NeedsReview => ("Compatibility unknown", StatusTone::Warning),
            PortabilityClass::DoNotTouch => ("Restore unavailable", StatusTone::Warning),
        }
    }
}

fn filter_matches(record: &PersistentStateRecord, filter: Filter) -> bool {
    match filter {
        Filter::All => true,
        Filter::GameSaves => record.state_type == PersistentStateType::NativeSave,
        Filter::MemoryCards => record.state_type == PersistentStateType::MemoryCard,
        Filter::SaveStates => record.state_type == PersistentStateType::SaveState,
        Filter::SystemStorage => record.state_type == PersistentStateType::NandOrVirtualDisk,
        Filter::NeedsAttention => {
            !record.warnings.is_empty()
                || matches!(
                    record.portability_class,
                    PortabilityClass::NeedsReview | PortabilityClass::DoNotTouch
                )
        }
    }
}

fn record_matches(record: &PersistentStateRecord, search: &str) -> bool {
    record
        .path
        .to_string_lossy()
        .to_ascii_lowercase()
        .contains(search)
        || record
            .emulator
            .as_str()
            .to_ascii_lowercase()
            .contains(search)
        || record.game_identity.iter().any(|identity| {
            identity
                .value
                .as_deref()
                .unwrap_or("")
                .to_ascii_lowercase()
                .contains(search)
        })
}

fn record_card(
    ui: &mut egui::Ui,
    record: &PersistentStateRecord,
    imagery: &mut super::imagery::Imagery,
) {
    let (restore_state, tone) = compatibility_label(record);
    egui::Frame::new()
        .fill(theme::card_fill(ui))
        .stroke(theme::border(ui))
        .corner_radius(8)
        .inner_margin(egui::Margin::same(theme::SPACE_LG as i8))
        .show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if let Some(platform) = emulator_platform_artwork(record.emulator) {
                imagery.platform_icon(ui, platform, 38.0);
            }
            ui.vertical(|ui| {
                ui.strong(state_type_label(record.state_type));
                ui.label(record.emulator.as_str());
            });
            status_badge(ui, restore_state, tone);
        });
        ui.label(match record.state_type {
            PersistentStateType::NativeSave => "Usually contains your game progress.",
            PersistentStateType::MemoryCard => "Contains saves for one or more games.",
            PersistentStateType::SaveState => "Captures the running emulator and may stop working after a version or configuration change.",
            PersistentStateType::NandOrVirtualDisk => "Contains more than one game's data and may include accounts, firmware or installed content.",
            PersistentStateType::ConfigBoundState => "Emulator settings or state tied to a particular profile.",
            PersistentStateType::CloudManaged => "Managed outside EmuWiz.",
            PersistentStateType::Unknown => "EmuWiz could not safely classify this state.",
        });
        if let Some(origin)=record.sram_origin.as_ref(){
            let label=match origin.state{
                archivefs_core::sram_provenance::OriginState::Associated=>"Associated · observed SRAM write",
                archivefs_core::sram_provenance::OriginState::NeedsReview=>"Needs review · origin evidence changed",
            };
            ui.strong(label);
            ui.label(&origin.explanation);
            ui.small(format!("Verified cartridge SHA-256: {}",origin.context.cartridge.sha256));
        } else if !record.game_identity.is_empty() {
            ui.add_space(theme::SPACE_XS);
            ui.label(egui::RichText::new("Game identity").strong());
            ui.label(
                record
                    .game_identity
                    .iter()
                    .filter_map(|i| i.value.as_deref())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        } else if matches!(record.state_type, PersistentStateType::NativeSave | PersistentStateType::SaveState) {
            ui.label(egui::RichText::new("Unassigned · no trusted cartridge-to-save observation. Historical saves are never matched by filename.").color(theme::warning(ui)));
        }
        ui.label(match record.state_type {
            PersistentStateType::SaveState => format!(
                "Compatibility: {} · emulator {}{}",
                restore_state,
                record.emulator.as_str(),
                record
                    .emulator_version
                    .as_deref()
                    .map(|version| format!(" version {version}"))
                    .unwrap_or_default()
            ),
            PersistentStateType::MemoryCard => "Container: restore the card through its supported workflow; it is not a single save file.".to_string(),
            PersistentStateType::NativeSave if record.provenance.contains("PS2 card save directory") => "Card save set: this is a directory/set and must be handled as one unit.".to_string(),
            _ => format!("Restore status: {restore_state}"),
        });
        if matches!(record.portability_class, PortabilityClass::VersionBound | PortabilityClass::EmulatorBound) {
            ui.label("Check the emulator, version and profile before considering restore.");
        }
        // This list only shows where saves are; the buttons are never enabled here.
        ui.weak("Restore is not available on this page. It only shows where your saves are and never changes them.");
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled(false, egui::Button::new("Preview Restore"))
                .on_hover_text("Not available here: this page is a read-only list of saves.");
            ui.add_enabled(false, egui::Button::new("Restore"))
                .on_hover_text("Not available here: this page is a read-only list of saves. Nothing is restored or changed.");
        });
        ui.collapsing("Advanced details", |ui| {
            ui.label(format!("Path: {}", record.path.display()));
            ui.label(format!("State type: {}", state_type_label(record.state_type)));
            ui.label(format!("Portability: {}", portability_label(record.portability_class)));
            ui.label(format!("Size: {} bytes", record.size_bytes));
            ui.label(&record.provenance);
            if let Some(origin)=&record.sram_origin{
                ui.label(format!("Observed installation: {}",origin.context.executable.path.display()));
                ui.label(format!("Observed core: {} · SHA-256 {}",origin.context.core.path.display(),origin.context.core.sha256));
                ui.label(format!("Settings scope: {} · session {}",origin.context.scope.configuration.path.display(),origin.session));
            }
            for warning in &record.warnings { ui.label(warning); }
        });
    });
}

fn emulator_platform_artwork(emulator: StateEmulator) -> Option<&'static str> {
    match emulator {
        StateEmulator::DuckStation => Some("PSX"),
        StateEmulator::Pcsx2 => Some("PS2"),
        StateEmulator::Ppsspp => Some("PSP"),
        StateEmulator::Dolphin => Some("GameCube"),
        StateEmulator::Rpcs3 => Some("PS3"),
        StateEmulator::RetroArch
        | StateEmulator::Flycast
        | StateEmulator::Mame
        | StateEmulator::Hatari
        | StateEmulator::FsUae
        | StateEmulator::Xemu
        | StateEmulator::Xenia
        | StateEmulator::Cemu
        | StateEmulator::Vita3k
        | StateEmulator::Unknown => None,
    }
}

fn checkpoint_motif(ui: &mut egui::Ui, size: egui::Vec2) {
    let rect = egui::Rect::from_min_size(ui.min_rect().min, size).shrink(10.0);
    let painter = ui.painter();
    let card = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 5.0, rect.top()),
        egui::pos2(rect.right() - 2.0, rect.bottom() - 4.0),
    );
    painter.rect_filled(card, 6.0, theme::raised_fill(ui));
    painter.rect_stroke(
        card,
        6.0,
        egui::Stroke::new(1.5_f32, theme::accent(ui).gamma_multiply(0.8)),
        egui::StrokeKind::Inside,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(card.left() + 8.0, card.top() + 9.0),
            egui::pos2(card.right() - 8.0, card.top() + 16.0),
        ),
        2.0,
        theme::accent(ui).gamma_multiply(0.7),
    );
    for index in 0..3 {
        painter.circle_filled(
            egui::pos2(
                card.left() + 13.0 + index as f32 * 12.0,
                card.bottom() - 12.0,
            ),
            2.5,
            if index == 1 {
                theme::AMBER
            } else {
                theme::SECONDARY_TEXT
            },
        );
    }
}

fn state_type_label(value: PersistentStateType) -> &'static str {
    match value {
        PersistentStateType::NativeSave => "Game save",
        PersistentStateType::MemoryCard => "Memory card",
        PersistentStateType::SaveState => "Savestate",
        PersistentStateType::NandOrVirtualDisk => "System storage",
        PersistentStateType::ConfigBoundState => "Emulator settings/state",
        PersistentStateType::CloudManaged => "Managed externally",
        PersistentStateType::Unknown => "Unknown state",
    }
}

fn portability_label(value: PortabilityClass) -> &'static str {
    match value {
        PortabilityClass::SafeToCopy => "Safe to back up",
        PortabilityClass::CopyWithMetadata => "Back up with emulator/game details",
        PortabilityClass::VersionBound => "Keep with this emulator version",
        PortabilityClass::EmulatorBound => "Tied to this emulator",
        PortabilityClass::NeedsReview => "Needs review",
        PortabilityClass::DoNotTouch => "Do not restore automatically",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_at(path: &str) -> PersistentStateRecord {
        PersistentStateRecord {
            sram_origin: None,
            emulator: StateEmulator::DuckStation,
            selected_installation: None,
            state_type: PersistentStateType::NativeSave,
            game_identity: Vec::new(),
            path: path.into(),
            container_path: None,
            slot_profile_account: None,
            emulator_version: None,
            firmware_context: None,
            portability_class: PortabilityClass::SafeToCopy,
            source_path_origin:
                archivefs_core::persistent_state_inventory::StatePathOrigin::Configured,
            provenance: "fixture".into(),
            sha256: None,
            size_bytes: 1,
            warnings: Vec::new(),
        }
    }

    /// Regression test for the "Advanced details flashes open/closed"
    /// symptom: every record card renders an identically-labelled
    /// `ui.collapsing("Advanced details", ...)`. `egui::Frame::show` gives
    /// its content `Ui` the same default id (`"child"`) relative to its
    /// parent, so without a `ui.push_id` keyed on something unique per
    /// record (here, the save's path — always distinct on disk), every
    /// card's "Advanced details" header resolves to the *same* persisted
    /// open/closed id and one record's toggle bleeds into every other
    /// record's card, appearing as a flash when the list repaints.
    #[test]
    fn witnessed_sram_joins_exact_content_path_and_never_grants_state_ownership() {
        use archivefs_core::sram_provenance::*;
        let stamp = Stamp {
            device: 1,
            inode: 2,
            size: 8192,
            modified: 0,
            modified_ns: 0,
            changed: 0,
            changed_ns: 0,
            links: 1,
            mode: 0o100600,
            uid: 1,
        };
        let object = Object {
            path: "/games/one/Game.gb".into(),
            stamp,
            sha256: "verified bytes".into(),
        };
        let mut archive = super::super::tests::archive(91, "A cartridge", Some("Game Boy"));
        archive.absolute_path = object.path.clone();
        let mut game = Game::from_archive(archive);
        game.dat_exact_hashes = Some(
            archivefs_core::dat::library_identity_summary::LibraryItemHashes {
                sha256: Some(object.sha256.clone()),
                ..Default::default()
            },
        );
        let mut record = record_at("/saves/arbitrary.srm");
        record.emulator = StateEmulator::RetroArch;
        let identity = archivefs_core::game_identity::IdentityEvidence {
            kind: archivefs_core::game_identity::IdentityKind::LooseRomSha256,
            status: archivefs_core::game_identity::IdentityStatus::Verified,
            value: Some(object.sha256.clone()),
            confidence: archivefs_core::game_identity::IdentityConfidence::StructuredMetadata,
            provenance: archivefs_core::game_identity::IdentityProvenance {
                archive_path: object.path.clone(),
                member_path: None,
                member_index: None,
                method: "worker fixture".into(),
            },
            diagnostic: "fixture".into(),
        };
        record.sram_origin = Some(SramOrigin {
            state: OriginState::Associated,
            session: "session".into(),
            context: Context {
                cartridge: object.clone(),
                cartridge_identity: identity,
                core: object.clone(),
                executable: object.clone(),
                scope: Scope {
                    selected_executable: object.path.clone(),
                    selected_core: object.path.clone(),
                    configuration: object.clone(),
                    saves: "/saves".into(),
                    saves_identity: DirectoryIdentity {
                        device: 1,
                        inode: 3,
                    },
                    profile: "Native/User".into(),
                },
            },
            artifact: object,
            explanation: "witness".into(),
        });
        assert!(record_belongs_to_game(&record, &game));
        game.dat_exact_hashes = None;
        assert!(
            record_belongs_to_game(&record, &game),
            "fresh sealed Gold evidence works without redundant library SHA256"
        );
        game.dat_exact_hashes = Some(
            archivefs_core::dat::library_identity_summary::LibraryItemHashes {
                sha256: Some("conflicting cached digest".into()),
                ..Default::default()
            },
        );
        assert!(!record_belongs_to_game(&record, &game));
        game.dat_exact_hashes = None;
        game.inspection_incomplete = true;
        assert!(!record_belongs_to_game(&record, &game));
        game.inspection_incomplete = false;
        record
            .sram_origin
            .as_mut()
            .unwrap()
            .context
            .cartridge_identity
            .status = archivefs_core::game_identity::IdentityStatus::Candidate;
        assert!(!record_belongs_to_game(&record, &game));
        record
            .sram_origin
            .as_mut()
            .unwrap()
            .context
            .cartridge_identity
            .status = archivefs_core::game_identity::IdentityStatus::Verified;
        assert!(record.game_identity.is_empty());
        let mut other = game.clone();
        other.archive.absolute_path = "/games/two/Game.gb".into();
        assert!(!record_belongs_to_game(&record, &other));
        record.sram_origin.as_mut().unwrap().state = OriginState::NeedsReview;
        assert!(!record_belongs_to_game(&record, &game));
        record.sram_origin.as_mut().unwrap().state = OriginState::Associated;
        record.state_type = PersistentStateType::SaveState;
        assert!(!record_belongs_to_game(&record, &game));
    }

    #[test]
    fn clearing_save_filters_keeps_inspection_and_selection_evidence() {
        let mut state = SavesStatesState {
            filter: Filter::SaveStates,
            search: "no match".into(),
            generation: 7,
            ..Default::default()
        };
        assert!(state.filters_active());
        state.clear_filters();
        assert!(!state.filters_active());
        assert_eq!(state.generation, 7);
        assert!(state.job.is_none());
    }

    #[test]
    fn advanced_details_id_is_isolated_per_record() {
        let context = egui::Context::default();
        let record_a = record_at("/fixture/saves/a.bin");
        let record_b = record_at("/fixture/saves/b.bin");
        let mut ids = Vec::new();
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    for record in [&record_a, &record_b] {
                        // Mirrors the caller in `show()`: the record must be
                        // pushed as an id scope *before* `record_card` opens
                        // its `Frame`, or the frame's content `Ui` collapses
                        // every record onto the same default "child" id.
                        ui.push_id(&record.path, |ui| {
                            ids.push(
                                ui.id()
                                    .with(egui::Id::from("child"))
                                    .with("Advanced details"),
                            );
                        });
                    }
                });
            },
        );
        assert_eq!(ids.len(), 2);
        assert_ne!(
            ids[0], ids[1],
            "two records must not share an \"Advanced details\" collapsing id"
        );
    }

    #[test]
    fn friendly_labels_do_not_leak_backend_taxonomy() {
        assert_eq!(
            state_type_label(PersistentStateType::NativeSave),
            "Game save"
        );
        assert_eq!(
            state_type_label(PersistentStateType::SaveState),
            "Savestate"
        );
        assert_eq!(
            portability_label(PortabilityClass::SafeToCopy),
            "Safe to back up"
        );
        assert_eq!(
            portability_label(PortabilityClass::EmulatorBound),
            "Tied to this emulator"
        );
    }

    #[test]
    fn restore_readiness_does_not_call_unknown_or_bound_state_ready() {
        let mut record = record_at("/fixture/states/checkpoint.state");
        record.state_type = PersistentStateType::SaveState;
        record.portability_class = PortabilityClass::VersionBound;
        assert_eq!(compatibility_label(&record).0, "Unknown");
        record.portability_class = PortabilityClass::NeedsReview;
        assert_eq!(compatibility_label(&record).0, "Unknown");
    }

    #[test]
    fn verified_cartridge_and_hashed_saves_do_not_manufacture_save_ownership() {
        use archivefs_core::game_identity::{IdentityKind, IdentityStatus};
        use archivefs_core::persistent_state_inventory::inventory_persistent_state;
        use archivefs_core::verified_game_fixture::VerifiedGameFixture;

        let root = tempfile::tempdir().unwrap();
        let rom = archivefs_core::synthetic_media::gameboy(0);
        let fixture = VerifiedGameFixture::build_at_with_game(
            &root.path().join("profile"),
            "Save QA.gb",
            &rom,
            "Nintendo - Game Boy",
            &[],
        )
        .unwrap();
        let library = super::super::verified_game_fixture_tests::load(&fixture);
        let mut game = library.games[0].clone();
        assert_eq!(game.identity_summary(), "Verified");
        // Library DAT verification and selected-game cartridge inspection are
        // separate observations. Inspect real fixture bytes, as Launch does.
        game.archive.identity_report = Some(
            archivefs_core::game_identity::inspect_catalogued_game_identity(
                &fixture.rom_path,
                Some("Game Boy"),
            ),
        );
        assert!(
            game.archive
                .identity_report
                .as_ref()
                .unwrap()
                .evidence
                .iter()
                .any(|evidence| evidence.kind == IdentityKind::LooseRomSha256
                    && evidence.status == IdentityStatus::Verified)
        );

        for (extension, kind, bytes) in [
            ("srm", PersistentStateType::NativeSave, vec![0xff; 8192]),
            (
                "state",
                PersistentStateType::SaveState,
                b"opaque state".to_vec(),
            ),
        ] {
            let path = root.path().join(format!("Save QA.{extension}"));
            std::fs::write(&path, &bytes).unwrap();
            let inventory = inventory_persistent_state(&[PersistentStateRoot::configured(
                StateEmulator::RetroArch,
                &path,
                Some(kind),
            )]);
            let [record] = inventory.records.as_slice() else {
                panic!("the configured save must remain visible in the full inventory");
            };
            assert!(record.sha256.is_some(), "save integrity was checked");
            assert!(record.game_identity.is_empty());
            assert!(!record_belongs_to_game(record, &game));
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn ordinary_save_association_survives_auxiliary_invalid_identity() {
        use archivefs_core::game_identity::*;
        let identity = IdentityEvidence {
            kind: IdentityKind::Ps2Serial,
            status: IdentityStatus::Verified,
            value: Some("SCUS-12345".into()),
            confidence: IdentityConfidence::StructuredMetadata,
            provenance: IdentityProvenance {
                archive_path: "synthetic".into(),
                member_path: None,
                member_index: None,
                method: "fixture".into(),
            },
            diagnostic: "fixture".into(),
        };
        let mut invalid = identity.clone();
        invalid.kind = IdentityKind::Pcsx2ExecutableCrc;
        invalid.status = IdentityStatus::Invalid;
        let mut archive = super::super::tests::archive(91, "Synthetic game", Some("PS2"));
        archive.identity_report = Some(GameIdentityReport {
            archive_path: archive.absolute_path.clone(),
            platform: IdentityPlatform::PlayStation2,
            format: IdentityImageFormat::Iso,
            evidence: vec![identity.clone(), invalid],
            warnings: vec![],
            bytes_read: 0,
            archive_members_inspected: 0,
            metadata_paths_inspected: 0,
            nested_container_depth: 0,
            complete: false,
        });
        let game = Game::from_archive(archive);
        let mut record = record_at("/synthetic/save");
        record.game_identity = vec![identity];
        assert!(record_belongs_to_game(&record, &game));
        record.state_type = PersistentStateType::SaveState;
        assert!(!record_belongs_to_game(&record, &game));
    }

    #[test]
    fn warnings_make_restore_unavailable_and_keep_preview_before_apply() {
        let mut record = record_at("/fixture/saves/checkpoint.bin");
        record.warnings.push("source changed".into());
        assert_eq!(compatibility_label(&record).0, "Restore unavailable");
        assert_eq!(
            state_type_label(PersistentStateType::MemoryCard),
            "Memory card"
        );
        assert_eq!(
            state_type_label(PersistentStateType::SaveState),
            "Savestate"
        );
    }
}
