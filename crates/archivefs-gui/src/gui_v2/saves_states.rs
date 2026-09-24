//! Native, read-only Saves & States projection.
//!
//! The worker owns inventory I/O. This module only translates the provider
//! neutral records into a compact, beginner-friendly view. Restore/export
//! actions continue to live in the established PS1/PS2 Save Vault.

use super::{
    App,
    imagery::{EmptyArt, empty_state},
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
    pub(super) loading: bool,
    pub(super) job: Option<u64>,
    pub(super) generation: u64,
    pub(super) filter: Filter,
    pub(super) search: String,
    pub(super) error: Option<String>,
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

pub(super) fn show(app: &mut App, ui: &mut egui::Ui) {
    let loading = app.saves_states.loading;
    let has_inventory = app.saves_states.inventory.is_some();
    let counts = app.saves_states.inventory.as_ref().map(|inventory| {
        [
            inventory
                .records
                .iter()
                .filter(|record| record.state_type == PersistentStateType::NativeSave)
                .count(),
            inventory
                .records
                .iter()
                .filter(|record| record.state_type == PersistentStateType::MemoryCard)
                .count(),
            inventory
                .records
                .iter()
                .filter(|record| record.state_type == PersistentStateType::SaveState)
                .count(),
        ]
    });
    let mut refresh = false;
    page_hero(
        ui,
        checkpoint_motif,
        "Saves & States",
        "Your progress, preserved safely.",
        Some((
            if loading {
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
        Some("Checkpoints stay yours. Inspection never changes them."),
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
                        egui::Stroke::new(1.0_f32, theme::TEAL.gamma_multiply(0.35)),
                    );
                },
            );
        },
        |ui| {
            if ui
                .add(
                    egui::Button::new(if has_inventory {
                        "Refresh save locations"
                    } else {
                        "Inspect save locations"
                    })
                    .fill(theme::PRIMARY_ACTION)
                    .min_size(egui::vec2(190.0, 40.0)),
                )
                .clicked()
            {
                refresh = true;
            }
            if ui.button("Open PS1/PS2 Save Vault").clicked() {
                app.go(Route::Section(Section::Emulators));
            }
        },
    );
    if refresh && !app.saves_states.loading {
        app.start_saves_inventory();
    }

    if let Some(error) = &app.saves_states.error {
        ui.colored_label(egui::Color32::YELLOW, error);
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
    summary(ui, inventory);
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
    let records: Vec<_> = inventory
        .records
        .iter()
        .filter(|record| {
            filter_matches(record, app.saves_states.filter)
                && (search.is_empty() || record_matches(record, &search))
        })
        .collect();
    if records.is_empty() {
        let detail = if inventory.roots_inspected == 0 {
            "Set up an emulator before EmuWiz can find its saves."
        } else if !inventory.warnings.is_empty() {
            "Your configured save location is unavailable or needs review."
        } else {
            "EmuWiz looks for game saves, memory cards and savestates in the configured emulator locations. None were found yet; inspection never changes them."
        };
        empty_state(
            ui,
            &mut app.imagery,
            EmptyArt::Glyph("cartridge"),
            "No saves found yet",
            detail,
            None,
        );
    }
    egui::ScrollArea::vertical()
        .id_salt("v2_saves_states_records")
        .show(ui, |ui| {
            for record in records {
                record_card(ui, record, &mut app.imagery);
            }
        });
    for warning in &inventory.warnings {
        ui.collapsing("Advanced inventory details", |ui| ui.label(warning));
    }
}

fn summary(ui: &mut egui::Ui, inventory: &PersistentStateInventory) {
    let count = |kind| {
        inventory
            .records
            .iter()
            .filter(|r| r.state_type == kind)
            .count()
    };
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (label, value, detail) in [
                (
                    "Game saves",
                    count(PersistentStateType::NativeSave),
                    "portable progress",
                ),
                (
                    "Memory cards",
                    count(PersistentStateType::MemoryCard),
                    "shared save storage",
                ),
                (
                    "Savestates",
                    count(PersistentStateType::SaveState),
                    "emulator checkpoints",
                ),
                (
                    "System storage",
                    count(PersistentStateType::NandOrVirtualDisk),
                    "accounts and installed data",
                ),
            ] {
                ui.vertical(|ui| {
                    ui.strong(format!("{value} {label}"));
                    ui.label(egui::RichText::new(detail).color(theme::muted(ui)).small());
                });
            }
        });
    });
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
    let tone = if !record.warnings.is_empty()
        || matches!(
            record.portability_class,
            PortabilityClass::NeedsReview | PortabilityClass::DoNotTouch
        ) {
        StatusTone::Warning
    } else {
        StatusTone::Success
    };
    egui::Frame::new()
        .fill(theme::CARD_SURFACE)
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
            status_badge(ui, portability_label(record.portability_class), tone);
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
        if !record.game_identity.is_empty() {
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
            ui.label(egui::RichText::new("Needs review · game ownership is not confirmed.").color(theme::WARNING));
        }
        ui.collapsing("Advanced details", |ui| {
            ui.label(format!("Path: {}", record.path.display()));
            ui.label(format!("State type: {:?}", record.state_type));
            ui.label(format!("Portability: {:?}", record.portability_class));
            ui.label(format!("Size: {} bytes", record.size_bytes));
            ui.label(&record.provenance);
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
    painter.rect_filled(card, 6.0, theme::RAISED_SURFACE);
    painter.rect_stroke(
        card,
        6.0,
        egui::Stroke::new(1.5_f32, theme::TEAL.gamma_multiply(0.8)),
        egui::StrokeKind::Inside,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(card.left() + 8.0, card.top() + 9.0),
            egui::pos2(card.right() - 8.0, card.top() + 16.0),
        ),
        2.0,
        theme::TEAL.gamma_multiply(0.7),
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
}
