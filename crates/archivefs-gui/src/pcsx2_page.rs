//! PCSX2 GUI Integration Batch H2: a read-only "PCSX2" section - environment
//! health plus, only when the caller already has an authoritative verified
//! PS2 serial (and, separately, an authoritative verified executable CRC),
//! a mapping of local PCSX2 per-game config/patch/cheat/texture/memory-card/
//! save-state state for the selected title.
//!
//! # PCSX2 is not an identity authority
//!
//! This module never resolves or guesses a PS2 serial or executable CRC
//! itself. It only ever accepts them, already verified, from its caller
//! (see [`gather_pcsx2_status`]'s `verified_ps2_serial`/
//! `verified_executable_crc` parameters) and passes them straight through
//! to
//! [`archivefs_core::patch_manager::Pcsx2GameRequest`], the exact same
//! identity-safety boundary
//! [`archivefs_core::patch_manager::pcsx2_local`] itself enforces core-side
//! (see that module's own doc comment). When no verified serial is
//! available this panel still shows PCSX2's own local environment health
//! (detected, config readable, BIOS), but never claims a title-specific
//! mapping - no per-game config, patch/cheat state, texture pack, memory
//! card, or save-state ownership is ever shown for an unresolved,
//! ambiguous, or conflicting selection.
//!
//! # No mutation
//!
//! This module never starts PCSX2, edits a config file, enables a patch or
//! cheat, installs a texture pack, or touches a memory card/save state.
//! Loading is always an explicit action, off the UI thread, following the
//! same generation-guarded background-load convention this crate already
//! uses elsewhere (`thread::spawn` + `mpsc::channel` + a stale-result
//! generation check) - see the RPCS3 panel (`rpcs3_page.rs`) for the
//! identical pattern this module mirrors.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use archivefs_core::memory_card_inventory::{
    MemoryCardHealth, MemoryCardInventory, Ps2ClusterChainHealth, Ps2DirectoryEntry,
    Ps2InventoryWarning, Ps2SaveDirectory, Ps2SaveFile,
};
use archivefs_core::patch_manager::{
    Pcsx2BiosVerification, Pcsx2GameInspection, Pcsx2GameRequest, Pcsx2InstallationType,
    Pcsx2MemcardKind, Pcsx2ProfileDiscoveryRoots, Pcsx2SerialMapping, discover_pcsx2_profiles,
    inspect_pcsx2_game,
};
use eframe::egui;

use crate::ui::components as widgets;

// ---------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pcsx2FoundProfile {
    pub(crate) configuration_path: PathBuf,
    pub(crate) installation_type: Pcsx2InstallationType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Pcsx2StatusOutcome {
    /// No eligible PCSX2 profile was found in any documented location.
    NotFound,
    Found {
        profile: Pcsx2FoundProfile,
        inspection: Box<Pcsx2GameInspection>,
        memory_cards: Vec<MemoryCardInventory>,
    },
}

/// Real, read-only environment probing: resolves discovery roots from the
/// process environment, discovers documented PCSX2 profiles (no home
/// directory recursion), and - when one is eligible - inspects it for the
/// supplied verified identity (never re-derived here; see this module's
/// own doc comment). Must not be called on every frame; callers gate it
/// behind an explicit action, the same convention every other gather in
/// this crate uses.
pub(crate) fn gather_pcsx2_status(
    verified_ps2_serial: Option<String>,
    verified_executable_crc: Option<String>,
) -> Pcsx2StatusOutcome {
    let Ok(roots) = Pcsx2ProfileDiscoveryRoots::from_environment() else {
        return Pcsx2StatusOutcome::NotFound;
    };
    let Ok(discovery) = discover_pcsx2_profiles(&roots) else {
        return Pcsx2StatusOutcome::NotFound;
    };
    let Some(profile) = discovery
        .profiles
        .into_iter()
        .find(|profile| profile.eligible)
    else {
        return Pcsx2StatusOutcome::NotFound;
    };
    let request = Pcsx2GameRequest {
        verified_ps2_serial,
        verified_executable_crc,
        emulator_serial: None,
    };
    let inspection = inspect_pcsx2_game(&profile, &request);
    let memory_cards = inspection
        .memcards
        .iter()
        .filter(|card| card.present)
        .filter_map(|card| {
            archivefs_core::memory_card_inventory::inspect_memory_card(&card.path).ok()
        })
        .collect();
    Pcsx2StatusOutcome::Found {
        profile: Pcsx2FoundProfile {
            configuration_path: profile.configuration_path,
            installation_type: profile.installation_type,
        },
        inspection: Box::new(inspection),
        memory_cards,
    }
}

// ---------------------------------------------------------------------
// State
// ---------------------------------------------------------------------

#[derive(Default)]
pub(crate) enum Pcsx2StatusState {
    #[default]
    Idle,
    Loading {
        generation: u64,
        receiver: Receiver<(u64, Pcsx2StatusOutcome)>,
    },
    Ready {
        #[allow(dead_code)]
        generation: u64,
        outcome: Pcsx2StatusOutcome,
    },
}

pub(crate) enum Pcsx2StatusAction {
    /// Load (or reload) PCSX2 status. The only action this panel ever
    /// asks for - there is no mutating action in this vocabulary.
    Load,
}

// ---------------------------------------------------------------------
// Label helpers
// ---------------------------------------------------------------------

fn installation_type_label(kind: Pcsx2InstallationType) -> &'static str {
    match kind {
        Pcsx2InstallationType::Native => "Native",
        Pcsx2InstallationType::NativeAlternate => "Native (alternate data location)",
        Pcsx2InstallationType::FlatpakUser => "Flatpak (user)",
        Pcsx2InstallationType::FlatpakSystem => "Flatpak (system)",
        Pcsx2InstallationType::Portable => "Portable/AppImage",
    }
}

fn bios_label(status: Pcsx2BiosVerification) -> (&'static str, widgets::StatusTone) {
    match status {
        Pcsx2BiosVerification::Verified => ("Ready", widgets::StatusTone::Success),
        Pcsx2BiosVerification::PresentUnverified => {
            ("Present (unverified)", widgets::StatusTone::Pending)
        }
        Pcsx2BiosVerification::Missing => ("Missing", widgets::StatusTone::Warning),
        Pcsx2BiosVerification::Unreadable => ("Unreadable", widgets::StatusTone::Warning),
    }
}

// ---------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------

/// Draws the "PCSX2" section. `verified_ps2_serial`/`verified_executable_crc`
/// must already be authoritative (or `None`) - this panel never derives
/// either. Returns an action the caller should perform; drawing itself
/// never mutates anything.
pub(crate) fn show_pcsx2_panel(
    ui: &mut egui::Ui,
    advanced_mode: bool,
    verified_ps2_serial: Option<&str>,
    state: &Pcsx2StatusState,
) -> Option<Pcsx2StatusAction> {
    let mut action = None;
    widgets::section_header(
        ui,
        "PCSX2",
        Some("Local PS2 emulator environment and, for the selected title, its PCSX2 assets."),
    );

    match state {
        Pcsx2StatusState::Idle => {
            widgets::card(ui, |ui| {
                ui.label("PCSX2 status has not been checked yet.");
                if widgets::action_button(ui, "Check PCSX2", widgets::ActionStyle::Secondary, true)
                    .clicked()
                {
                    action = Some(Pcsx2StatusAction::Load);
                }
            });
        }
        Pcsx2StatusState::Loading { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Checking local PCSX2 installation…");
            });
        }
        Pcsx2StatusState::Ready { outcome, .. } => {
            if widgets::action_button(ui, "Refresh", widgets::ActionStyle::Quiet, true).clicked() {
                action = Some(Pcsx2StatusAction::Load);
            }
            show_outcome(ui, advanced_mode, verified_ps2_serial, outcome);
        }
    }

    action
}

fn show_outcome(
    ui: &mut egui::Ui,
    advanced_mode: bool,
    verified_ps2_serial: Option<&str>,
    outcome: &Pcsx2StatusOutcome,
) {
    let Pcsx2StatusOutcome::Found {
        profile,
        inspection,
        memory_cards,
    } = outcome
    else {
        widgets::empty_state(
            ui,
            "PCSX2 not found",
            "No PCSX2 installation was found in any documented location (native, Flatpak, or a \
             configured custom path).",
            None,
        );
        return;
    };

    widgets::card(ui, |ui| {
        widgets::status_badge(ui, "Emulator detected", widgets::StatusTone::Success);
        let (bios_headline, bios_tone) = bios_label(inspection.bios.verification);
        ui.label(format!("BIOS: {bios_headline}"));
        let _ = bios_tone;

        if verified_ps2_serial.is_none() {
            ui.label(
                "No verified PS2 serial is available for the current selection - showing \
                 PCSX2's own environment only.",
            );
        } else if inspection.serial_mapping == Pcsx2SerialMapping::VerifiedPs2Serial {
            ui.label("✓ PS2 serial matched");
            if let Some(config) = &inspection.per_game_config {
                ui.label(format!(
                    "Per-game config: {}",
                    if config.exists { "Found" } else { "Not found" }
                ));
            }
            if let Some(patches) = &inspection.patches
                && !patches.files.is_empty()
            {
                ui.label(format!(
                    "Patches/cheats: {} file(s) found",
                    patches.files.len()
                ));
            }
            if let Some(textures) = &inspection.textures
                && textures.present
            {
                ui.label(format!("Texture pack: {} file(s)", textures.file_count));
            }
            if inspection
                .memcards
                .iter()
                .any(|card| card.kind == Pcsx2MemcardKind::PerGameFolder && card.present)
            {
                ui.label("Memory card: per-game folder found");
            } else if inspection
                .memcards
                .iter()
                .any(|card| card.kind == Pcsx2MemcardKind::Shared && card.present)
            {
                ui.label("Memory card: shared card present");
            }
            if inspection.savestates.matched_count > 0 {
                ui.label(format!(
                    "Save states: {} found",
                    inspection.savestates.matched_count
                ));
            }
            if inspection.controllers.profile_configured {
                ui.label("Controller config: configured");
            }
        } else {
            ui.label("PS2 serial matched, but no title-specific PCSX2 data was found locally.");
        }

        if !advanced_mode {
            show_memory_card_contents(ui, false, memory_cards);
            return;
        }
        widgets::technical_details(
            ui,
            ("pcsx2_technical_detail", &profile.configuration_path),
            |ui| {
                ui.label(format!(
                    "Installation kind: {}",
                    installation_type_label(profile.installation_type)
                ));
                widgets::path_value(ui, "Configuration root", &profile.configuration_path);
                if let Some(id) = verified_ps2_serial {
                    widgets::copyable_value(ui, "Verified PS2 serial", id);
                }
                if let Some(serial) = &inspection.serial {
                    widgets::copyable_value(ui, "PCSX2 serial mapping input", serial);
                }
                ui.label(format!("Serial mapping: {:?}", inspection.serial_mapping));
                if let Some(bios_path) = &inspection.bios.path {
                    widgets::path_value(ui, "BIOS path", bios_path);
                }
                if let Some(config) = &inspection.per_game_config
                    && config.exists
                {
                    widgets::path_value(ui, "Per-game config path", &config.path);
                }
                if let Some(patches) = &inspection.patches {
                    ui.label(format!("Patch files inspected: {}", patches.files.len()));
                }
                if let Some(patch_match) = &inspection.patch_match {
                    ui.label(format!("Patch match state: {:?}", patch_match.state));
                }
                if let Some(textures) = &inspection.textures
                    && textures.present
                {
                    widgets::path_value(ui, "Texture pack path", &textures.path);
                }
                for card in &inspection.memcards {
                    if card.present {
                        widgets::path_value(ui, "Memory card path", &card.path);
                    }
                }
                if !inspection.health.warnings.is_empty() {
                    ui.label("Warnings:");
                    for warning in &inspection.health.warnings {
                        ui.add(egui::Label::new(warning.as_str()).selectable(true).wrap());
                    }
                }
            },
        );
    });
    show_memory_card_contents(ui, advanced_mode, memory_cards);
}

fn memory_card_health(health: MemoryCardHealth) -> (&'static str, widgets::StatusTone) {
    match health {
        MemoryCardHealth::Healthy => ("Healthy", widgets::StatusTone::Success),
        MemoryCardHealth::StructuralWarning | MemoryCardHealth::CorruptionSuspected => {
            ("Warning", widgets::StatusTone::Warning)
        }
        MemoryCardHealth::Malformed
        | MemoryCardHealth::Truncated
        | MemoryCardHealth::UnsupportedVariant
        | MemoryCardHealth::OutOfRangeMetadata => {
            ("Partially readable", widgets::StatusTone::Warning)
        }
        MemoryCardHealth::Unknown => ("Unknown", widgets::StatusTone::Pending),
    }
}

fn chain_health_label(chain: &Ps2ClusterChainHealth) -> (&'static str, widgets::StatusTone) {
    if chain.warnings.iter().any(|warning| {
        matches!(
            warning.kind,
            archivefs_core::memory_card_inventory::Ps2CorruptionKind::FatLoop
                | archivefs_core::memory_card_inventory::Ps2CorruptionKind::InvalidFatReference
                | archivefs_core::memory_card_inventory::Ps2CorruptionKind::ClusterOutOfRange
        )
    }) {
        ("Corrupt chain", widgets::StatusTone::Warning)
    } else if !chain.complete || !chain.warnings.is_empty() {
        ("Warning", widgets::StatusTone::Warning)
    } else {
        ("Healthy", widgets::StatusTone::Success)
    }
}

fn timestamp_label(
    timestamp: Option<&archivefs_core::memory_card_inventory::Ps2Timestamp>,
) -> Option<String> {
    timestamp.map(|value| {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {}",
            value.year,
            value.month,
            value.day,
            value.hour,
            value.minute,
            value.second,
            value.timezone
        )
    })
}

fn raw_name_label(entry: &Ps2DirectoryEntry) -> String {
    let bytes = entry
        .raw_name
        .iter()
        .copied()
        .take_while(|byte| *byte != 0)
        .collect::<Vec<_>>();
    bytes
        .iter()
        .map(|byte| match byte {
            0x20..=0x7e => (*byte as char).to_string(),
            value => format!("\\x{value:02x}"),
        })
        .collect()
}

fn warning_lines<'a>(warnings: impl IntoIterator<Item = &'a Ps2InventoryWarning>) -> Vec<String> {
    warnings
        .into_iter()
        .map(|warning| warning.message.clone())
        .collect()
}

fn save_size(directory: &Ps2SaveDirectory) -> u64 {
    directory
        .files
        .iter()
        .map(|file| file.declared_size_bytes)
        .sum()
}

fn file_health(file: &Ps2SaveFile) -> (&'static str, widgets::StatusTone) {
    chain_health_label(&file.chain_health)
}

fn show_memory_card_contents(
    ui: &mut egui::Ui,
    advanced_mode: bool,
    memory_cards: &[MemoryCardInventory],
) {
    widgets::section_header(
        ui,
        "Memory Card Contents",
        Some(
            "Read-only inventory of shared PS2 memory-card containers. No individual save can be extracted, restored, repaired, or deleted here.",
        ),
    );
    if memory_cards.is_empty() {
        ui.label("No readable memory card was found for this PCSX2 profile.");
        return;
    }
    for (card_index, card) in memory_cards.iter().enumerate() {
        let (health, tone) = memory_card_health(card.health);
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("PS2 Memory Card");
                widgets::status_badge(ui, health, tone);
            });
            ui.label(format!(
                "{} save director{} · {} bytes · shared memory-card container",
                card.ps2_inventory
                    .as_ref()
                    .map_or(0, |inventory| inventory.save_directories.len()),
                if card
                    .ps2_inventory
                    .as_ref()
                    .map_or(0, |inventory| inventory.save_directories.len())
                    == 1
                {
                    "y"
                } else {
                    "ies"
                },
                card.card_size_bytes
            ));
            if let Some(geometry) = &card.ps2_geometry {
                ui.label(format!(
                    "Filesystem: PS2 v{} · {}",
                    geometry.version.as_deref().unwrap_or("unknown"),
                    match geometry.representation {
                        archivefs_core::memory_card_inventory::Ps2PageRepresentation::RawDataOnly => "data-only pages",
                        archivefs_core::memory_card_inventory::Ps2PageRepresentation::RawWithSpare => "pages with spare bytes",
                        archivefs_core::memory_card_inventory::Ps2PageRepresentation::Unknown => "page representation unknown",
                    }
                ));
            }
            if !card.warnings.is_empty() {
                widgets::banner(
                    ui,
                    "Card warning",
                    &card.warnings.join(" "),
                    widgets::StatusTone::Warning,
                );
            }
            let Some(inventory) = &card.ps2_inventory else {
                ui.label("The card structure was recognised, but its save directory could not be read safely.");
                return;
            };
            let mut directories = inventory.save_directories.iter().collect::<Vec<_>>();
            directories.sort_by_key(|directory| directory.entry.display_name.to_lowercase());
            if directories.is_empty() {
                ui.label("No saves found on this memory card.");
            }
            for (save_index, directory) in directories.into_iter().enumerate() {
                let (save_health, save_tone) = if !directory.warnings.is_empty() {
                    ("Warning", widgets::StatusTone::Warning)
                } else {
                    chain_health_label(&directory.chain_health)
                };
                let id = ("ps2_memory_card_save", card_index, save_index);
                ui.push_id(id, |ui| {
                    ui.collapsing(
                        format!(
                            "{} · {} file{} · {} bytes",
                            directory.entry.display_name,
                            directory.files.len(),
                            if directory.files.len() == 1 { "" } else { "s" },
                            save_size(directory)
                        ),
                        |ui| {
                            ui.horizontal(|ui| {
                                widgets::status_badge(ui, save_health, save_tone);
                                if let Some(modified) = timestamp_label(directory.entry.modified.as_ref()) {
                                    ui.label(format!("Modified: {modified}"));
                                }
                            });
                            if directory.entry.display_name != raw_name_label(&directory.entry) {
                                ui.label(format!("Raw directory name: {}", raw_name_label(&directory.entry)));
                            }
                            for file in &directory.files {
                                let (file_health, file_tone) = file_health(file);
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(&file.entry.display_name);
                                    ui.label(format!("{} bytes", file.declared_size_bytes));
                                    widgets::status_badge(ui, file_health, file_tone);
                                    if let Some(modified) = timestamp_label(file.entry.modified.as_ref()) {
                                        ui.label(modified);
                                    }
                                });
                                if file.entry.display_name != raw_name_label(&file.entry) {
                                    ui.weak(format!("Raw name: {}", raw_name_label(&file.entry)));
                                }
                                if !file.chain_health.warnings.is_empty() {
                                    for warning in &file.chain_health.warnings {
                                        ui.label(format!("Warning: {}", warning.message));
                                    }
                                }
                            }
                            for warning in warning_lines(directory.warnings.iter()) {
                                ui.label(format!("Warning: {warning}"));
                            }
                            if advanced_mode {
                                widgets::technical_details(ui, ("ps2_save_technical", &directory.entry.raw_entry_offset), |ui| {
                                    ui.label(format!("Raw entry offset: {}", directory.entry.raw_entry_offset));
                                    ui.label(format!("Start cluster: {}", directory.entry.start_cluster));
                                    ui.label(format!("Chain length: {}", directory.chain_health.clusters.len()));
                                    ui.label(format!("Raw mode: 0x{:08x}", directory.entry.raw_mode));
                                    for file in &directory.files {
                                        ui.label(format!("{}: raw offset {}, start cluster {}, chain length {}", file.entry.display_name, file.entry.raw_entry_offset, file.entry.start_cluster, file.chain_health.clusters.len()));
                                    }
                                });
                            }
                        },
                    );
                });
            }
            if advanced_mode {
                widgets::technical_details(ui, ("ps2_card_technical", &card.path), |ui| {
                    widgets::path_value(ui, "Card path", &card.path);
                    ui.label(format!("Shared container: {}", card.shared_container));
                    ui.label(format!("Root entries: {}", inventory.root_entries.len()));
                    ui.label(format!(
                        "Root chain length: {}",
                        inventory.root_chain_health.clusters.len()
                    ));
                    for warning in &inventory.warnings {
                        ui.label(format!("Warning: {}", warning.message));
                    }
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use archivefs_core::memory_card_inventory::{
        MemoryCardFormat, MemoryCardFormatConfidence, Ps2ClusterChainHealth, Ps2DirectoryEntryKind,
        Ps2MemoryCardInventory, Ps2SaveDirectory, Ps2SaveFile,
    };
    use archivefs_core::patch_manager::{
        Pcsx2BiosInfo, Pcsx2Config, Pcsx2ControllerInfo, Pcsx2Health, Pcsx2SaveStateInventory,
    };

    use super::*;

    fn empty_inspection() -> Pcsx2GameInspection {
        Pcsx2GameInspection {
            serial: None,
            serial_mapping: Pcsx2SerialMapping::Unavailable,
            global_config: Pcsx2Config {
                path: PathBuf::from("/config/pcsx2/inis/PCSX2.ini"),
                exists: true,
                readable: true,
                settings: Default::default(),
                warnings: Vec::new(),
            },
            per_game_config: None,
            overridden_setting_keys: Vec::new(),
            patches: None,
            patch_match: None,
            textures: None,
            memcards: Vec::new(),
            savestates: Pcsx2SaveStateInventory::default(),
            controllers: Pcsx2ControllerInfo::default(),
            bios: Pcsx2BiosInfo {
                path: None,
                verification: Pcsx2BiosVerification::Missing,
                filename_hint: None,
                warnings: Vec::new(),
            },
            health: Pcsx2Health {
                detected: true,
                config_readable: true,
                bios: Pcsx2BiosVerification::Missing,
                patch_data_available: false,
                serial_mapping: Pcsx2SerialMapping::Unavailable,
                warnings: Vec::new(),
            },
        }
    }

    fn found_outcome(inspection: Pcsx2GameInspection) -> Pcsx2StatusOutcome {
        Pcsx2StatusOutcome::Found {
            profile: Pcsx2FoundProfile {
                configuration_path: PathBuf::from("/config/pcsx2"),
                installation_type: Pcsx2InstallationType::Native,
            },
            inspection: Box::new(inspection),
            memory_cards: Vec::new(),
        }
    }

    fn rendered_text_contains(output: &egui::FullOutput, needle: &str) -> bool {
        fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
            match shape {
                egui::Shape::Text(text_shape) => text_shape.galley.text().contains(needle),
                egui::Shape::Vec(nested) => nested.iter().any(|s| shape_contains(s, needle)),
                _ => false,
            }
        }
        output
            .shapes
            .iter()
            .any(|clipped| shape_contains(&clipped.shape, needle))
    }

    fn run_panel(
        advanced_mode: bool,
        verified_ps2_serial: Option<&str>,
        state: &Pcsx2StatusState,
    ) -> egui::FullOutput {
        let ctx = egui::Context::default();
        let draw = |ctx: &egui::Context| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_pcsx2_panel(ui, advanced_mode, verified_ps2_serial, state);
            });
        };
        let _ = ctx.run(egui::RawInput::default(), draw);
        ctx.run(egui::RawInput::default(), draw)
    }

    #[test]
    fn not_found_renders_an_empty_state() {
        let state = Pcsx2StatusState::Ready {
            generation: 1,
            outcome: Pcsx2StatusOutcome::NotFound,
        };
        let output = run_panel(false, None, &state);
        assert!(rendered_text_contains(&output, "PCSX2 not found"));
    }

    #[test]
    fn unresolved_identity_shows_environment_only_never_a_game_mapping() {
        let state = Pcsx2StatusState::Ready {
            generation: 1,
            outcome: found_outcome(empty_inspection()),
        };
        let output = run_panel(false, None, &state);
        assert!(rendered_text_contains(&output, "Emulator detected"));
        assert!(!rendered_text_contains(&output, "PS2 serial matched"));
    }

    #[test]
    fn ambiguous_or_conflicting_identity_never_produces_title_specific_mapping() {
        // Even with an inspection that *would* show title-specific data, a
        // caller must never pass a verified serial derived from anything
        // but a genuinely verified identity - this test represents the
        // Ambiguous/Conflict/Unknown case, where the caller correctly
        // passes `None` regardless of what PCSX2 itself has on disk.
        let mut inspection = empty_inspection();
        inspection.serial = Some("SLUS-20312".to_string());
        inspection.serial_mapping = Pcsx2SerialMapping::EmulatorMetadataOnly;
        let state = Pcsx2StatusState::Ready {
            generation: 1,
            outcome: found_outcome(inspection),
        };
        let output = run_panel(false, None, &state);
        assert!(!rendered_text_contains(&output, "PS2 serial matched"));
    }

    #[test]
    fn a_verified_serial_with_no_local_data_reports_that_honestly() {
        let state = Pcsx2StatusState::Ready {
            generation: 1,
            outcome: found_outcome(empty_inspection()),
        };
        let output = run_panel(false, Some("SLUS-20312"), &state);
        assert!(rendered_text_contains(
            &output,
            "PS2 serial matched, but no title-specific PCSX2 data was found locally."
        ));
    }

    #[test]
    fn bios_missing_state_is_shown() {
        let state = Pcsx2StatusState::Ready {
            generation: 1,
            outcome: found_outcome(empty_inspection()),
        };
        let output = run_panel(false, None, &state);
        assert!(rendered_text_contains(&output, "BIOS: Missing"));
    }

    #[test]
    fn per_game_config_found_is_shown_for_a_verified_match() {
        let mut inspection = empty_inspection();
        inspection.serial = Some("SLUS-20312".to_string());
        inspection.serial_mapping = Pcsx2SerialMapping::VerifiedPs2Serial;
        inspection.per_game_config = Some(Pcsx2Config {
            path: PathBuf::from("/config/pcsx2/inis/gamesettings/SLUS-20312.ini"),
            exists: true,
            readable: true,
            settings: Default::default(),
            warnings: Vec::new(),
        });
        let state = Pcsx2StatusState::Ready {
            generation: 1,
            outcome: found_outcome(inspection),
        };
        let output = run_panel(false, Some("SLUS-20312"), &state);
        assert!(rendered_text_contains(&output, "Per-game config: Found"));
    }

    #[test]
    fn gamer_mode_hides_technical_details_advanced_mode_offers_it() {
        let state = Pcsx2StatusState::Ready {
            generation: 1,
            outcome: found_outcome(empty_inspection()),
        };
        let gamer = run_panel(false, None, &state);
        let advanced = run_panel(true, None, &state);
        assert!(!rendered_text_contains(&gamer, "Technical details"));
        assert!(rendered_text_contains(&advanced, "Technical details"));
    }

    fn ps2_entry(name: &str, kind: Ps2DirectoryEntryKind, length: u32) -> Ps2DirectoryEntry {
        Ps2DirectoryEntry {
            raw_entry_offset: 512,
            raw_mode: 0x20,
            kind,
            raw_name: name.as_bytes().to_vec(),
            display_name: name.to_string(),
            length,
            start_cluster: 7,
            parent_entry: 0,
            attributes: 0,
            created: None,
            modified: None,
            warnings: Vec::new(),
        }
    }

    fn sample_ps2_card() -> MemoryCardInventory {
        let directory = Ps2SaveDirectory {
            entry: ps2_entry("BASLUS-00000SAVE", Ps2DirectoryEntryKind::Directory, 0),
            chain_health: Ps2ClusterChainHealth {
                clusters: vec![7],
                complete: true,
                warnings: Vec::new(),
            },
            children: Vec::new(),
            files: vec![Ps2SaveFile {
                entry: ps2_entry("icon.sys", Ps2DirectoryEntryKind::RegularFile, 2048),
                declared_size_bytes: 2048,
                chain_health: Ps2ClusterChainHealth {
                    clusters: vec![8],
                    complete: true,
                    warnings: Vec::new(),
                },
            }],
            warnings: Vec::new(),
        };
        MemoryCardInventory {
            path: PathBuf::from("/tmp/synthetic-card.ps2"),
            format: MemoryCardFormat::Ps2,
            format_confidence: MemoryCardFormatConfidence::ConfirmedFormat,
            health: MemoryCardHealth::Healthy,
            card_size_bytes: 8 * 1024 * 1024,
            entries: Vec::new(),
            used_blocks: None,
            free_blocks: None,
            warnings: Vec::new(),
            shared_container: true,
            ps2_geometry: None,
            ps2_inventory: Some(Ps2MemoryCardInventory {
                root_chain_health: Ps2ClusterChainHealth {
                    clusters: vec![1],
                    complete: true,
                    warnings: Vec::new(),
                },
                root_entries: vec![directory.entry.clone()],
                save_directories: vec![directory],
                warnings: Vec::new(),
            }),
        }
    }

    #[test]
    fn memory_card_contents_render_save_and_child_file_without_mutating_actions() {
        let ctx = egui::Context::default();
        let card = sample_ps2_card();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_memory_card_contents(ui, true, std::slice::from_ref(&card));
            });
        });
        assert!(rendered_text_contains(&output, "PS2 Memory Card"));
        assert!(rendered_text_contains(&output, "BASLUS-00000SAVE"));
        assert!(rendered_text_contains(&output, "icon.sys"));
        assert!(rendered_text_contains(&output, "2048 bytes"));
        assert!(rendered_text_contains(
            &output,
            "shared memory-card container"
        ));
        assert!(rendered_text_contains(&output, "Raw entry offset"));
        assert!(!rendered_text_contains(&output, "Export Save"));
        assert!(!rendered_text_contains(&output, "Repair"));
        assert!(!rendered_text_contains(&output, "Delete Save"));
    }

    #[test]
    fn empty_ps2_card_uses_a_neutral_empty_state() {
        let mut card = sample_ps2_card();
        card.ps2_inventory
            .as_mut()
            .unwrap()
            .save_directories
            .clear();
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_memory_card_contents(ui, false, std::slice::from_ref(&card));
            });
        });
        assert!(rendered_text_contains(
            &output,
            "No saves found on this memory card."
        ));
        assert!(!rendered_text_contains(&output, "error"));
    }
}
