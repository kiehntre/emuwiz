//! Read-only PPSSPP resolution for the Emulators page.
//!
//! This is the same preview as the DuckStation one: the shared
//! [`archivefs_core::emulator_profile_resolver`] decides, this module gathers
//! the inputs, runs the resolver off the GUI thread, tags the answer with the
//! request it belongs to, and hands the typed outcome to the shared
//! presentation. PPSSPP's own executable/profile pairing rules stay in the
//! core adapter. It never writes a preference, creates a folder or launches.

use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use archivefs_core::emulator_inventory::InventoryEmulator;
use archivefs_core::emulator_profile_resolver::desktop::default_desktop_directories;
use archivefs_core::emulator_profile_resolver::{
    EmulatorSelectionMode, PpssppAdapter, load_selection_from, resolve_emulator_profile,
};
use archivefs_core::patch_manager::PpssppProfileDiscoveryRoots;

use super::{ResolutionMessage, ResolutionView};
use crate::emulator_setup_overrides::EmulatorPathOverrides;

/// Everything the worker needs, captured on the GUI thread.
#[derive(Debug, Clone)]
pub(crate) struct PpssppResolutionInputs {
    pub(crate) roots: PpssppProfileDiscoveryRoots,
    pub(crate) desktop_directories: Vec<PathBuf>,
    /// The existing remembered-selection file; `None` is read as AUTO.
    pub(crate) selection_path: Option<PathBuf>,
}

impl PpssppResolutionInputs {
    pub(crate) fn from_environment(overrides: &EmulatorPathOverrides) -> Result<Self, String> {
        let mut roots =
            PpssppProfileDiscoveryRoots::from_environment().map_err(|e| e.to_string())?;
        apply_overrides_to_roots(&mut roots, overrides);
        let desktop_directories = default_desktop_directories(&roots.home, &roots.xdg_data_home);
        Ok(Self {
            roots,
            desktop_directories,
            selection_path:
                archivefs_core::emulator_profile_resolver::persist::default_selection_path().ok(),
        })
    }
}

/// Apply the existing Setup overrides to a read-only PPSSPP discovery root.
/// They are executable/profile evidence only; native preflight still checks
/// the same exact paths and every content identity fact.
pub(crate) fn apply_overrides_to_roots(
    roots: &mut PpssppProfileDiscoveryRoots,
    overrides: &EmulatorPathOverrides,
) {
    // A remembered path is evidence for discovery, never an order.
    if let Some(executable) = &overrides.ppsspp_executable {
        roots.explicit_executables.push(executable.clone());
    }
    if let Some(folder) = &overrides.ppsspp_configuration_folder {
        roots.explicit_configuration_roots.push(folder.clone());
    }
}

/// Pure and read-only: reads the policy file and the filesystem facts.
pub(crate) fn compute_ppsspp_resolution(inputs: &PpssppResolutionInputs) -> ResolutionView {
    let (mode, selection_note) = match &inputs.selection_path {
        None => (
            EmulatorSelectionMode::Auto,
            Some("The settings folder is unavailable, so no remembered choice was read.".into()),
        ),
        Some(path) => match load_selection_from(path, InventoryEmulator::Ppsspp) {
            Ok(mode) => (mode, None),
            Err(error) => (
                EmulatorSelectionMode::Auto,
                Some(format!(
                    "The remembered choice could not be read ({error}); showing the automatic answer. Nothing was changed."
                )),
            ),
        },
    };
    let adapter = PpssppAdapter::new(inputs.roots.clone(), inputs.desktop_directories.clone());
    let outcome = resolve_emulator_profile(&adapter, &mode);
    ResolutionView {
        mode,
        selection_note,
        outcome,
    }
}

/// Starts the worker. The result is tagged with `generation`.
pub(crate) fn spawn_ppsspp_resolution(
    generation: u64,
    inputs: PpssppResolutionInputs,
    context: Option<egui::Context>,
) -> Receiver<ResolutionMessage> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let view = compute_ppsspp_resolution(&inputs);
        let _ = sender.send((generation, view));
        if let Some(context) = context {
            context.request_repaint();
        }
    });
    receiver
}

pub(crate) enum PpssppResolutionState {
    NotScanned,
    Scanning {
        receiver: Receiver<ResolutionMessage>,
    },
    Ready {
        generation: u64,
        view: Box<ResolutionView>,
    },
    Error(String),
}

#[cfg(test)]
mod tests;
