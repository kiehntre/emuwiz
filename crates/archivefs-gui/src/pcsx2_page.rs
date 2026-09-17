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

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use archivefs_core::memory_card_inventory::{
    MemoryCardHealth, MemoryCardInventory, Ps2ClusterChainHealth, Ps2DirectoryEntry,
    Ps2DirectoryEntryKind, Ps2FileExportError, Ps2FileExportPlan, Ps2FileExportResult,
    Ps2InventoryWarning, Ps2PsuExportError, Ps2PsuExportPlan, Ps2PsuExportResult, Ps2SaveDirectory,
    Ps2SaveFile, apply_ps2_file_export, apply_ps2_psu_export, plan_ps2_file_export,
    plan_ps2_psu_export,
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
    ChooseMemoryCard,
    UsePcsx2MemoryCard,
}

#[derive(Default)]
pub(crate) struct Pcsx2SaveVaultState {
    pub(crate) manual_path: Option<PathBuf>,
    pub(crate) source: Pcsx2SaveCardSource,
    pub(crate) manual_inventory: Option<Result<MemoryCardInventory, String>>,
    pub(crate) manual_loading: Option<Receiver<Result<MemoryCardInventory, String>>>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pcsx2SaveCardSource {
    #[default]
    Pcsx2,
    Manual,
}

const SAVED_PS2_CARD_PATH: &str = "pcsx2_save_vault_card.txt";

pub(crate) fn load_saved_ps2_card_path() -> Option<PathBuf> {
    let path = archivefs_core::app_dirs::config_path(SAVED_PS2_CARD_PATH).ok()?;
    let value = std::fs::read_to_string(path).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| PathBuf::from(value))
}

pub(crate) fn save_saved_ps2_card_path(path: &Path) {
    if let Ok(config_path) = archivefs_core::app_dirs::config_path(SAVED_PS2_CARD_PATH) {
        let _ = std::fs::write(config_path, path.to_string_lossy().as_bytes());
    }
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
#[cfg(test)]
pub(crate) fn show_pcsx2_panel(
    ui: &mut egui::Ui,
    advanced_mode: bool,
    verified_ps2_serial: Option<&str>,
    state: &Pcsx2StatusState,
) -> Option<Pcsx2StatusAction> {
    let mut save_vault = Pcsx2SaveVaultState::default();
    show_pcsx2_panel_with_save_vault(
        ui,
        advanced_mode,
        verified_ps2_serial,
        state,
        &mut save_vault,
    )
}

pub(crate) fn show_pcsx2_panel_with_save_vault(
    ui: &mut egui::Ui,
    advanced_mode: bool,
    verified_ps2_serial: Option<&str>,
    state: &Pcsx2StatusState,
    save_vault: &mut Pcsx2SaveVaultState,
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
            let refresh =
                widgets::action_button(ui, "Refresh", widgets::ActionStyle::Quiet, true).clicked();
            action = show_outcome(ui, advanced_mode, verified_ps2_serial, outcome, save_vault)
                .or_else(|| refresh.then_some(Pcsx2StatusAction::Load));
        }
    }

    action
}

fn show_outcome(
    ui: &mut egui::Ui,
    advanced_mode: bool,
    verified_ps2_serial: Option<&str>,
    outcome: &Pcsx2StatusOutcome,
    save_vault: &mut Pcsx2SaveVaultState,
) -> Option<Pcsx2StatusAction> {
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
        return None;
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
    show_save_vault(ui, advanced_mode, memory_cards, save_vault)
}

fn show_save_vault(
    ui: &mut egui::Ui,
    advanced_mode: bool,
    pcsx2_cards: &[MemoryCardInventory],
    save_vault: &mut Pcsx2SaveVaultState,
) -> Option<Pcsx2StatusAction> {
    let mut source_action = None;
    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("Save source:");
            if ui
                .selectable_label(
                    save_vault.source == Pcsx2SaveCardSource::Pcsx2,
                    "Current PCSX2 card",
                )
                .clicked()
            {
                source_action = Some(Pcsx2StatusAction::UsePcsx2MemoryCard);
            }
            if widgets::action_button(
                ui,
                "Choose another memory-card image…",
                widgets::ActionStyle::Secondary,
                true,
            )
            .clicked()
            {
                source_action = Some(Pcsx2StatusAction::ChooseMemoryCard);
            }
        });
        if let Some(path) = &save_vault.manual_path {
            ui.label(format!("Last manually selected card: {}", path.display()));
        }
        match save_vault.source {
            Pcsx2SaveCardSource::Pcsx2 => {
                ui.label("Source origin: PCSX2 configuration (inspection only)");
                show_memory_card_contents(ui, advanced_mode, pcsx2_cards);
            }
            Pcsx2SaveCardSource::Manual => {
                let Some(path) = save_vault.manual_path.as_ref() else {
                    ui.label("No manual memory-card image has been selected.");
                    return;
                };
                ui.label("Source origin: Manual selection (inspection only)");
                widgets::path_value(ui, "Selected card", path);
                if save_vault.manual_loading.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Reading selected memory card…");
                    });
                } else if let Some(result) = &save_vault.manual_inventory {
                    match result {
                        Ok(card) => {
                            show_memory_card_contents(ui, advanced_mode, std::slice::from_ref(card))
                        }
                        Err(error) => {
                            widgets::empty_state(ui, "Memory card could not be read", error, None);
                        }
                    }
                } else {
                    ui.label("Selected card is waiting for inspection.");
                }
            }
        }
    });
    source_action
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
        ("Corrupt", widgets::StatusTone::Warning)
    } else if !chain.complete {
        ("Incomplete", widgets::StatusTone::Warning)
    } else if chain
        .clusters
        .windows(2)
        .any(|clusters| clusters[1] != clusters[0].saturating_add(1))
    {
        ("Fragmented but readable", widgets::StatusTone::Success)
    } else if !chain.warnings.is_empty() {
        ("Warning", widgets::StatusTone::Warning)
    } else {
        ("Healthy", widgets::StatusTone::Success)
    }
}

fn save_health_label(directory: &Ps2SaveDirectory) -> (&'static str, widgets::StatusTone) {
    if !directory.warnings.is_empty() || !directory.entry.warnings.is_empty() {
        ("Needs review", widgets::StatusTone::Warning)
    } else {
        chain_health_label(&directory.chain_health)
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
    raw_name_bytes_label(&entry.raw_name)
}

fn raw_name_bytes_label(raw_name: &[u8]) -> String {
    let bytes = raw_name
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

#[derive(Clone)]
enum ExportDialogState {
    Confirm(Ps2FileExportPlan),
    Success(Ps2FileExportResult),
    Refused(String),
}

#[derive(Clone)]
enum PsuExportDialogState {
    /// Boxed: this plan dwarfs the other variants and the whole value is
    /// cloned out of egui's temp store on every frame the dialog is open.
    Confirm(Box<Ps2PsuExportPlan>),
    Success(Ps2PsuExportResult),
    Refused(String),
}

fn export_dialog_id() -> egui::Id {
    egui::Id::new("ps2_memory_card_file_export")
}

fn psu_export_dialog_id() -> egui::Id {
    egui::Id::new("ps2_memory_card_psu_export")
}

fn safe_export_filename(file: &Ps2DirectoryEntry) -> (String, bool) {
    let name = file.display_name.trim();
    let safe = !name.is_empty()
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|character| character.is_control() || character == '/' || character == '\\');
    if safe {
        (name.to_string(), false)
    } else {
        let mut derived = name
            .chars()
            .map(|character| {
                if character.is_control() || character == '/' || character == '\\' {
                    '_'
                } else {
                    character
                }
            })
            .collect::<String>();
        if derived.trim_matches('_').is_empty() || derived == "." || derived == ".." {
            derived = "ps2-file.bin".to_string();
        }
        (derived, true)
    }
}

fn safe_psu_filename(directory: &Ps2DirectoryEntry) -> (String, bool) {
    let (name, sanitised) = safe_export_filename(directory);
    (format!("{name}.psu"), sanitised)
}

fn psu_export_blocked(directory: &Ps2SaveDirectory) -> bool {
    directory.entry.kind != Ps2DirectoryEntryKind::Directory
        || !directory.entry.warnings.is_empty()
        || !directory.warnings.is_empty()
        || !directory.chain_health.complete
        || !directory.chain_health.warnings.is_empty()
        || directory
            .children
            .iter()
            .any(|entry| entry.kind != Ps2DirectoryEntryKind::Unused)
        || directory.files.iter().any(|file| {
            file.entry.kind != Ps2DirectoryEntryKind::RegularFile
                || !file.entry.warnings.is_empty()
                || !file.chain_health.complete
                || !file.chain_health.warnings.is_empty()
                || file.entry.created.is_none()
                || file.entry.modified.is_none()
        })
}

fn psu_export_error_message(error: &Ps2PsuExportError) -> String {
    match error {
        Ps2PsuExportError::SourceChanged => {
            "The memory card changed since it was inspected. PSU export was refused; inspect it again and retry.".into()
        }
        Ps2PsuExportError::DestinationExists(path) => format!(
            "The destination already exists, so EmuWiz did not overwrite it:\n{}",
            path.display()
        ),
        Ps2PsuExportError::UnsafeDestination(path) => format!(
            "The PSU destination is unsafe (including a symlink), so export was refused:\n{}",
            path.display()
        ),
        Ps2PsuExportError::InvalidPlan(detail) => {
            format!("PSU export was refused because this save is not safe: {detail}")
        }
        Ps2PsuExportError::Io(detail) => format!("PSU export could not be completed: {detail}"),
    }
}

fn show_psu_export_dialog(ui: &mut egui::Ui, advanced_mode: bool) {
    let id = psu_export_dialog_id();
    let Some(mut state) = ui.data_mut(|data| data.get_temp::<PsuExportDialogState>(id)) else {
        return;
    };
    let mut close = false;
    let mut next_state = None;
    egui::Window::new("Export PS2 save as PSU")
        .id(id)
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| match &state {
            PsuExportDialogState::Confirm(plan) => {
                ui.heading("Export this complete PS2 save as a PSU file");
                ui.label("EmuWiz reads the card only. The original save and memory card remain unchanged.");
                ui.label(format!("Memory card: {}", plan.source_card_path.display()));
                ui.label(format!("Save directory: {}", plan.save_display_name));
                if plan.save_display_name != raw_name_bytes_label(&plan.save_raw_name) {
                    ui.label(format!("Original PS2 directory name: {}", raw_name_bytes_label(&plan.save_raw_name)));
                }
                ui.label(format!("Files: {}", plan.files.len()));
                ui.label(format!("Total logical save size: {} bytes", save_size_from_files(&plan.files)));
                ui.label("Format: PSU (PS2 save container)");
                ui.label(format!("Destination: {}", plan.destination.display()));
                ui.label("Source-card identity: verified (SHA-256 bound to this export plan)");
                if advanced_mode {
                    ui.label("Metadata: raw names, timestamps, modes, attributes, logical lengths, and card data-page bytes preserved.");
                    for file in &plan.files {
                        ui.label(format!("Member: {} ({} bytes)", file.entry.display_name, file.declared_size_bytes));
                    }
                }
                ui.horizontal(|ui| {
                    if widgets::action_button(ui, "Export Save as PSU", widgets::ActionStyle::Primary, true).clicked() {
                        match apply_ps2_psu_export(plan) {
                            Ok(result) => next_state = Some(PsuExportDialogState::Success(result)),
                            Err(error) => next_state = Some(PsuExportDialogState::Refused(psu_export_error_message(&error))),
                        }
                    }
                    if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true).clicked() {
                        close = true;
                    }
                });
            }
            PsuExportDialogState::Success(result) => {
                ui.heading("PS2 save exported as PSU");
                ui.label(format!("Destination: {}", result.destination.display()));
                ui.label(format!("Size: {} bytes", result.output_bytes));
                ui.label(format!("SHA-256: {}", result.sha256));
                ui.label(format!("Files: {}", result.file_count));
                ui.label("The source PS2 memory card and original save were unchanged.");
                if widgets::action_button(ui, "Close", widgets::ActionStyle::Secondary, true).clicked() {
                    close = true;
                }
            }
            PsuExportDialogState::Refused(message) => {
                ui.heading("PSU export refused");
                ui.label(message.as_str());
                ui.label("No memory-card data was changed.");
                if widgets::action_button(ui, "Close", widgets::ActionStyle::Secondary, true).clicked() {
                    close = true;
                }
            }
        });
    if close {
        ui.data_mut(|data| data.remove::<PsuExportDialogState>(id));
    } else {
        if let Some(next_state) = next_state {
            state = next_state;
        }
        ui.data_mut(|data| data.insert_temp(id, state));
    }
}

fn save_size_from_files(files: &[Ps2SaveFile]) -> u64 {
    files.iter().map(|file| file.declared_size_bytes).sum()
}

fn export_blocked(file: &Ps2SaveFile) -> bool {
    file.entry.kind != Ps2DirectoryEntryKind::RegularFile
        || !file.entry.warnings.is_empty()
        || !file.chain_health.complete
        || file.chain_health.warnings.iter().any(|warning| {
            warning.kind
                != archivefs_core::memory_card_inventory::Ps2CorruptionKind::FileSizeExceedsChain
        })
}

fn export_error_message(error: &Ps2FileExportError) -> String {
    match error {
        Ps2FileExportError::SourceChanged => {
            "The memory card changed since it was inspected. Export was refused; inspect it again and retry.".into()
        }
        Ps2FileExportError::DestinationExists(path) => {
            format!("The destination already exists, so EmuWiz did not overwrite it:\n{}", path.display())
        }
        Ps2FileExportError::UnsafeDestination(path) => {
            format!("The destination is unsafe (including a symlink), so export was refused:\n{}", path.display())
        }
        Ps2FileExportError::InvalidPlan(detail) => format!("Export was refused because this file is not safe to reconstruct: {detail}"),
        Ps2FileExportError::Io(detail) => format!("Export could not be completed: {detail}"),
    }
}

fn show_export_dialog(ui: &mut egui::Ui) {
    let id = export_dialog_id();
    let Some(mut state) = ui.data_mut(|data| data.get_temp::<ExportDialogState>(id)) else {
        return;
    };
    let mut close = false;
    let mut next_state = None;
    egui::Window::new("Export one PS2 file")
        .id(id)
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| match &state {
            ExportDialogState::Confirm(plan) => {
                ui.heading("Export one file from this PS2 memory card");
                ui.label(
                    "EmuWiz reads the card only. Exporting does not modify the PS2 memory card.",
                );
                ui.label(format!("Memory card: {}", plan.source_card_path.display()));
                ui.label(format!("File: {}", plan.display_name));
                if plan.display_name != raw_name_bytes_label(&plan.raw_name) {
                    ui.label(format!(
                        "Original PS2 filename: {}",
                        raw_name_bytes_label(&plan.raw_name)
                    ));
                }
                ui.label(format!(
                    "Logical file size: {} bytes",
                    plan.declared_size_bytes
                ));
                ui.label(format!("Destination: {}", plan.destination.display()));
                ui.label("Source-card identity: verified (SHA-256 bound to this export plan)");
                if !plan.warnings.is_empty() {
                    ui.label(format!("Warning: {}", plan.warnings.join(" ")));
                }
                ui.horizontal(|ui| {
                    if widgets::action_button(
                        ui,
                        "Export File",
                        widgets::ActionStyle::Primary,
                        true,
                    )
                    .clicked()
                    {
                        match apply_ps2_file_export(plan) {
                            Ok(result) => next_state = Some(ExportDialogState::Success(result)),
                            Err(error) => {
                                next_state =
                                    Some(ExportDialogState::Refused(export_error_message(&error)))
                            }
                        }
                    }
                    if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true)
                        .clicked()
                    {
                        close = true;
                    }
                });
            }
            ExportDialogState::Success(result) => {
                ui.heading("File exported");
                ui.label(format!(
                    "Exported: {}",
                    result
                        .destination
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("PS2 file")
                ));
                ui.label(format!("Destination: {}", result.destination.display()));
                ui.label(format!("Size: {} bytes", result.bytes_written));
                ui.label(format!("SHA-256: {}", result.sha256));
                ui.label("The source PS2 memory card was unchanged.");
                if widgets::action_button(ui, "Close", widgets::ActionStyle::Secondary, true)
                    .clicked()
                {
                    close = true;
                }
            }
            ExportDialogState::Refused(message) => {
                ui.heading("Export refused");
                ui.label(message.as_str());
                ui.label("No memory-card data was changed.");
                if widgets::action_button(ui, "Close", widgets::ActionStyle::Secondary, true)
                    .clicked()
                {
                    close = true;
                }
            }
        });
    if close {
        ui.data_mut(|data| data.remove::<ExportDialogState>(id));
    } else {
        if let Some(next_state) = next_state {
            state = next_state;
        }
        ui.data_mut(|data| data.insert_temp(id, state));
    }
}

fn show_memory_card_contents(
    ui: &mut egui::Ui,
    advanced_mode: bool,
    memory_cards: &[MemoryCardInventory],
) {
    widgets::section_header(
        ui,
        "Save Vault · Memory Card Contents",
        Some(
            "Inspect PS2 memory-card saves and export one complete save as a PSU file. The card is read-only; restore/import is not available yet.",
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
            widgets::path_value(ui, "Source card", &card.path);
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
                let (save_health, save_tone) = save_health_label(directory);
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
                                if card.ps2_geometry.is_some()
                                    && card.ps2_inventory.is_some()
                                    && !psu_export_blocked(directory)
                                    && widgets::action_button(ui, "Export Save as PSU", widgets::ActionStyle::Secondary, true).clicked()
                                {
                                    let (filename, _sanitised) = safe_psu_filename(&directory.entry);
                                    let destination = rfd::FileDialog::new()
                                        .set_title("Choose destination for exported PS2 save")
                                        .set_file_name(&filename)
                                        .save_file();
                                    if let Some(destination) = destination {
                                        let _ = match plan_ps2_psu_export(card, directory, &destination) {
                                            Ok(plan) => ui.data_mut(|data| data.insert_temp(psu_export_dialog_id(), PsuExportDialogState::Confirm(Box::new(plan)))),
                                            Err(error) => ui.data_mut(|data| data.insert_temp(psu_export_dialog_id(), PsuExportDialogState::Refused(psu_export_error_message(&error)))),
                                        };
                                    }
                                };
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
                                    if !export_blocked(file)
                                        && widgets::action_button(ui, "Export File", widgets::ActionStyle::Secondary, true).clicked()
                                    {
                                        let (filename, sanitised) = safe_export_filename(&file.entry);
                                        let destination = rfd::FileDialog::new()
                                            .set_title("Choose destination for exported PS2 file")
                                            .set_file_name(&filename)
                                            .save_file();
                                        if let Some(destination) = destination {
                                            match plan_ps2_file_export(card, file, &destination) {
                                                Ok(plan) => {
                                                    ui.data_mut(|data| data.insert_temp(export_dialog_id(), ExportDialogState::Confirm(plan)));
                                                }
                                                Err(error) => {
                                                    ui.data_mut(|data| data.insert_temp(export_dialog_id(), ExportDialogState::Refused(export_error_message(&error))));
                                                }
                                            };
                                        }
                                        let _ = sanitised;
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
    show_export_dialog(ui);
    show_psu_export_dialog(ui, advanced_mode);
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
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_memory_card_contents(ui, true, std::slice::from_ref(&card));
            });
        });
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_memory_card_contents(ui, true, std::slice::from_ref(&card));
            });
        });
        assert!(rendered_text_contains(&output, "PS2 Memory Card"));
        assert!(rendered_text_contains(&output, "BASLUS-00000SAVE"));
        assert!(rendered_text_contains(
            &output,
            "shared memory-card container"
        ));
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
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_memory_card_contents(ui, false, std::slice::from_ref(&card));
            });
        });
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

    #[test]
    fn save_vault_uses_plain_health_states_and_names_the_source_card() {
        let mut card = sample_ps2_card();
        card.ps2_inventory.as_mut().unwrap().save_directories[0]
            .chain_health
            .clusters = vec![1, 4];
        assert_eq!(
            save_health_label(&card.ps2_inventory.as_ref().unwrap().save_directories[0]).0,
            "Fragmented but readable"
        );
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_memory_card_contents(ui, false, std::slice::from_ref(&card));
            });
        });
        assert!(rendered_text_contains(&output, "Save Vault"));
        assert!(rendered_text_contains(&output, "Source card"));
        assert!(rendered_text_contains(
            &output,
            "restore/import is not available yet"
        ));
    }

    #[test]
    fn save_vault_manual_source_is_distinct_and_fails_cleanly() {
        let state = Pcsx2StatusState::Ready {
            generation: 1,
            outcome: found_outcome(empty_inspection()),
        };
        let mut save_vault = Pcsx2SaveVaultState {
            manual_path: Some(PathBuf::from("/tmp/not-a-memory-card.ps2")),
            source: Pcsx2SaveCardSource::Manual,
            manual_inventory: Some(Err("unsupported memory-card format".to_string())),
            manual_loading: None,
        };
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_pcsx2_panel_with_save_vault(ui, false, None, &state, &mut save_vault);
            });
        });
        assert!(rendered_text_contains(&output, "Manual selection"));
        assert!(rendered_text_contains(&output, "Selected card"));
        assert!(rendered_text_contains(
            &output,
            "unsupported memory-card format"
        ));
        assert!(rendered_text_contains(
            &output,
            "Choose another memory-card image"
        ));
    }

    #[test]
    fn directory_and_warned_files_do_not_expose_export_action() {
        let mut card = sample_ps2_card();
        card.ps2_inventory.as_mut().unwrap().save_directories[0].files[0]
            .entry
            .warnings
            .push(archivefs_core::memory_card_inventory::Ps2InventoryWarning {
                kind: archivefs_core::memory_card_inventory::Ps2CorruptionKind::InvalidFilename,
                message: "unsafe name".into(),
            });
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_memory_card_contents(ui, false, std::slice::from_ref(&card));
            });
        });
        assert!(!rendered_text_contains(&output, "Export File"));
    }

    #[test]
    fn safe_filename_and_zero_length_regular_file_are_supported() {
        let valid = ps2_entry("icon.sys", Ps2DirectoryEntryKind::RegularFile, 0);
        assert_eq!(safe_export_filename(&valid), ("icon.sys".into(), false));
        let unsafe_entry = ps2_entry("bad/name", Ps2DirectoryEntryKind::RegularFile, 0);
        assert_eq!(
            safe_export_filename(&unsafe_entry),
            ("bad_name".into(), true)
        );
        let empty = Ps2SaveFile {
            entry: ps2_entry("empty.dat", Ps2DirectoryEntryKind::RegularFile, 0),
            declared_size_bytes: 0,
            chain_health: Ps2ClusterChainHealth {
                clusters: Vec::new(),
                complete: true,
                warnings: Vec::new(),
            },
        };
        assert!(!export_blocked(&empty));
    }

    #[test]
    fn valid_save_directory_is_eligible_for_psu_export_and_filename_is_deterministic() {
        let mut card = sample_ps2_card();
        let directory = &mut card.ps2_inventory.as_mut().unwrap().save_directories[0];
        directory.entry.kind = Ps2DirectoryEntryKind::Directory;
        directory.entry.created = Some(archivefs_core::memory_card_inventory::Ps2Timestamp {
            raw: [0, 1, 2, 3, 4, 5, 0xea, 0x07],
            year: 2026,
            month: 5,
            day: 4,
            hour: 3,
            minute: 2,
            second: 1,
            timezone: "JST (UTC+09:00)",
        });
        directory.entry.modified = directory.entry.created.clone();
        directory.files[0].entry.created = directory.entry.created.clone();
        directory.files[0].entry.modified = directory.entry.modified.clone();
        assert!(!psu_export_blocked(directory));
        assert_eq!(
            safe_psu_filename(&directory.entry),
            ("BASLUS-00000SAVE.psu".into(), false)
        );
    }

    #[test]
    fn psu_dialog_uses_immutable_source_wording_and_shows_result_facts() {
        let result = PsuExportDialogState::Success(Ps2PsuExportResult {
            destination: PathBuf::from("/tmp/save.psu"),
            output_bytes: 2048,
            sha256: "psuhash".into(),
            file_count: 2,
            source_card_path: PathBuf::from("/tmp/card.ps2"),
            source_card_sha256: "cardhash".into(),
            provenance: "synthetic".into(),
        });
        let ctx = egui::Context::default();
        ctx.data_mut(|data| data.insert_temp(psu_export_dialog_id(), result));
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show_psu_export_dialog(ui, false));
        });
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show_psu_export_dialog(ui, false));
        });
        assert!(rendered_text_contains(&output, "exported as PSU"));
        assert!(rendered_text_contains(&output, "Size: 2048 bytes"));
        assert!(rendered_text_contains(&output, "SHA-256: psuhash"));
        assert!(rendered_text_contains(
            &output,
            "source PS2 memory card and original save were unchanged"
        ));
    }

    #[test]
    fn psu_error_messages_are_plain_and_do_not_offer_other_formats() {
        let message = psu_export_error_message(&Ps2PsuExportError::SourceChanged);
        assert!(message.contains("changed since it was inspected"));
        assert!(!message.contains("MAX"));
        assert!(!message.contains("Ps2PsuExportError"));
    }

    #[test]
    fn refusal_dialog_shows_existing_destination_and_source_read_only_wording() {
        let ctx = egui::Context::default();
        ctx.data_mut(|data| {
            data.insert_temp(
                export_dialog_id(),
                ExportDialogState::Refused(
                    "The destination already exists, so EmuWiz did not overwrite it:\n/tmp/existing".into(),
                ),
            )
        });
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, show_export_dialog);
        });
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, show_export_dialog);
        });
        assert!(rendered_text_contains(&output, "Export refused"));
        assert!(rendered_text_contains(&output, "did not overwrite it"));
        assert!(rendered_text_contains(
            &output,
            "No memory-card data was changed"
        ));
        assert!(!rendered_text_contains(&output, "Ps2FileExportError"));
    }

    #[test]
    fn typed_source_changed_and_symlink_refusals_are_novice_facing() {
        let source_changed = export_error_message(&Ps2FileExportError::SourceChanged);
        assert!(source_changed.contains("changed since it was inspected"));
        let symlink = export_error_message(&Ps2FileExportError::UnsafeDestination(PathBuf::from(
            "/tmp/link",
        )));
        assert!(symlink.contains("including a symlink"));
        assert!(!symlink.contains("Ps2FileExportError"));
    }

    #[test]
    fn success_dialog_shows_result_hash_size_and_immutable_source() {
        let ctx = egui::Context::default();
        ctx.data_mut(|data| {
            data.insert_temp(
                export_dialog_id(),
                ExportDialogState::Success(Ps2FileExportResult {
                    destination: PathBuf::from("/tmp/icon.sys"),
                    bytes_written: 0,
                    sha256: "abc123".into(),
                    source_card_path: PathBuf::from("/tmp/card.ps2"),
                    source_card_sha256: "cardhash".into(),
                    provenance: "synthetic".into(),
                }),
            )
        });
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, show_export_dialog);
        });
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, show_export_dialog);
        });
        assert!(rendered_text_contains(&output, "File exported"));
        assert!(rendered_text_contains(&output, "Size: 0 bytes"));
        assert!(rendered_text_contains(&output, "SHA-256: abc123"));
        assert!(rendered_text_contains(
            &output,
            "source PS2 memory card was unchanged"
        ));
    }

    #[test]
    fn export_surface_has_no_whole_save_actions_and_keeps_advanced_evidence_separate() {
        let ctx = egui::Context::default();
        let card = sample_ps2_card();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_memory_card_contents(ui, true, std::slice::from_ref(&card));
            });
        });
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_memory_card_contents(ui, true, std::slice::from_ref(&card));
            });
        });
        assert!(rendered_text_contains(&output, "Memory Card Contents"));
        for action in [
            "Export Save",
            "Export PSU",
            "Export MAX",
            "Export CBS",
            "Export SPS",
            "Export XPS",
        ] {
            assert!(!rendered_text_contains(&output, action));
        }
        assert!(rendered_text_contains(
            &output,
            "export one complete save as a PSU file"
        ));
    }
}
