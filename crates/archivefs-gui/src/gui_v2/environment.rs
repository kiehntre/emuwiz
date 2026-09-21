//! Read-only environment projection for GUI v2 onboarding and Doctor.
//!
//! This module deliberately owns no setup or migration logic. It gathers the
//! existing core diagnostics and bounded emulator inventory on the GUI-v2
//! worker, then exposes plain-language state for the onboarding surface.

use std::{env, fs, path::PathBuf};

use crate::gui_v2::library::Library;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct EmulatorSummary {
    pub label: String,
    pub installations: usize,
    pub paths: Vec<PathBuf>,
    pub installation_types: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct EnvironmentSnapshot {
    pub config_path: Option<PathBuf>,
    pub data_path: Option<PathBuf>,
    pub config_present: bool,
    pub database_present: bool,
    pub database_schema: Option<i64>,
    pub required_schema: i64,
    pub database_upgrade_required: bool,
    pub database_error: Option<String>,
    pub source_count: usize,
    pub available_sources: usize,
    pub unavailable_sources: Vec<PathBuf>,
    pub source_error: Option<String>,
    pub identification_data_count: usize,
    pub identification_data_ready: bool,
    pub emulator_summaries: Vec<EmulatorSummary>,
    pub legacy_config_present: bool,
    pub legacy_data_present: bool,
    pub both_roots_conflict: bool,
    pub active_config_root: Option<PathBuf>,
    pub active_data_root: Option<PathBuf>,
}

impl EnvironmentSnapshot {
    pub fn is_fresh(&self) -> bool {
        !self.config_present
            && !self.database_present
            && self.source_count == 0
            && self.identification_data_count == 0
            && !self.legacy_config_present
            && !self.legacy_data_present
    }

    pub fn installed_emulator_count(&self) -> usize {
        self.emulator_summaries
            .iter()
            .map(|summary| summary.installations)
            .sum()
    }

    pub fn source_needs_attention(&self) -> bool {
        !self.unavailable_sources.is_empty() || self.source_error.is_some()
    }

    pub fn setup_ready_count(&self, library: &Library) -> usize {
        usize::from(self.source_count > 0 && !self.source_needs_attention())
            + usize::from(self.installed_emulator_count() > 0)
            + usize::from(self.identification_data_ready)
            + usize::from(library.games.iter().any(|game| game.identified))
    }

    pub fn setup_attention_count(&self, library: &Library) -> usize {
        usize::from(self.both_roots_conflict)
            + usize::from(self.source_needs_attention())
            + usize::from(self.database_upgrade_required)
            + usize::from(!library.games.is_empty() && self.installed_emulator_count() == 0)
    }
}

impl Default for EnvironmentSnapshot {
    fn default() -> Self {
        Self {
            config_path: None,
            data_path: None,
            config_present: false,
            database_present: false,
            database_schema: None,
            required_schema: archivefs_core::latest_schema_version(),
            database_upgrade_required: false,
            database_error: None,
            source_count: 0,
            available_sources: 0,
            unavailable_sources: Vec::new(),
            source_error: None,
            identification_data_count: 0,
            identification_data_ready: false,
            emulator_summaries: Vec::new(),
            legacy_config_present: false,
            legacy_data_present: false,
            both_roots_conflict: false,
            active_config_root: None,
            active_data_root: None,
        }
    }
}

/// Gather all filesystem/process probes used by the v2 onboarding and Doctor.
/// This is called only from the existing GUI-v2 background worker.
pub(super) fn gather() -> EnvironmentSnapshot {
    let config_path = archivefs_core::default_config_path().ok();
    let data_path = archivefs_core::default_database_path().ok();
    let config_present = config_path.as_deref().is_some_and(path_present);
    let database_present = data_path.as_deref().is_some_and(path_present);

    let diagnostics = config_path
        .as_deref()
        .map(archivefs_core::run_setup_diagnostics_read_only);
    let active_config_root = diagnostics.as_ref().and_then(|report| {
        report
            .config_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(PathBuf::from)
    });
    let active_data_root = data_path
        .as_ref()
        .and_then(|path| path.parent().map(PathBuf::from));

    let (source_count, available_sources, unavailable_sources, source_error) =
        match archivefs_core::load_source_folder_configs_default() {
            Ok(sources) => {
                let unavailable = sources
                    .iter()
                    .filter(|source| source.enabled && !path_present(&source.path))
                    .map(|source| source.path.clone())
                    .collect::<Vec<_>>();
                let available = sources
                    .iter()
                    .filter(|source| source.enabled && path_present(&source.path))
                    .count();
                (sources.len(), available, unavailable, None)
            }
            Err(_error) if !config_present => (0, 0, Vec::new(), None),
            Err(error) => (0, 0, Vec::new(), Some(error.to_string())),
        };

    let identification_data_count = local_dat_count() + managed_dat_count();
    let identification_data_ready = identification_data_count > 0;

    let emulator_summaries = emulator_summaries();
    let (database_schema, database_error) = if database_present {
        let report = data_path.as_deref().map(archivefs_core::diagnose_database);
        let schema = report.as_ref().and_then(|report| report.schema_version);
        let error = report.as_ref().and_then(|report| {
            (!report.diagnostics.is_empty() && schema.is_none())
                .then(|| "The library database could not be opened safely.".to_string())
        });
        (schema, error)
    } else {
        (None, None)
    };
    let required_schema = archivefs_core::latest_schema_version();

    let (legacy_config_present, legacy_data_present, both_roots_conflict) = legacy_roots();

    EnvironmentSnapshot {
        config_path,
        data_path,
        config_present,
        database_present,
        database_schema,
        required_schema,
        database_upgrade_required: database_schema.is_some_and(|schema| schema < required_schema),
        database_error,
        source_count,
        available_sources,
        unavailable_sources,
        source_error,
        identification_data_count,
        identification_data_ready,
        emulator_summaries,
        legacy_config_present,
        legacy_data_present,
        both_roots_conflict,
        active_config_root,
        active_data_root,
    }
}

fn path_present(path: &std::path::Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

fn local_dat_count() -> usize {
    archivefs_core::dat::sources::default_dat_sources_config_path()
        .ok()
        .and_then(|path| archivefs_core::dat::sources::load_dat_sources_config_from(path).ok())
        .and_then(|config| config.sources)
        .map_or(0, |sources| sources.len())
}

fn managed_dat_count() -> usize {
    archivefs_core::dat::managed_sources::default_managed_dat_sources_config_path()
        .ok()
        .and_then(|path| {
            archivefs_core::dat::managed_sources::load_managed_dat_sources_from(path).ok()
        })
        .map(|sources| {
            let config = sources.to_config();
            config.mame_software_lists.len()
                + config.redump_bios.len()
                + config.redump_games.len()
                + usize::from(config.fbneo.is_some())
        })
        .unwrap_or_default()
}

fn emulator_summaries() -> Vec<EmulatorSummary> {
    let inventory = archivefs_core::emulator_inventory::discover_installed_emulators();
    [
        archivefs_core::emulator_inventory::InventoryEmulator::Dolphin,
        archivefs_core::emulator_inventory::InventoryEmulator::Rpcs3,
        archivefs_core::emulator_inventory::InventoryEmulator::Pcsx2,
        archivefs_core::emulator_inventory::InventoryEmulator::Ppsspp,
        archivefs_core::emulator_inventory::InventoryEmulator::DuckStation,
        archivefs_core::emulator_inventory::InventoryEmulator::Xemu,
    ]
    .into_iter()
    .map(|emulator| {
        let installations = inventory
            .installations
            .iter()
            .filter(|installation| installation.emulator == emulator)
            .collect::<Vec<_>>();
        EmulatorSummary {
            label: emulator.label().to_string(),
            installations: installations.len(),
            paths: installations
                .iter()
                .map(|installation| installation.executable_path.clone())
                .collect(),
            installation_types: installations
                .iter()
                .map(|installation| format!("{:?}", installation.installation_type))
                .collect(),
        }
    })
    .collect()
}

fn legacy_roots() -> (bool, bool, bool) {
    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        return (false, false, false);
    };
    let primary_config = home.join(".config/emuwiz");
    let legacy_config = home.join(".config/archivefs");
    let primary_data = home.join(".local/share/emuwiz");
    let legacy_data = home.join(".local/share/archivefs");
    let legacy_config_present = path_present(&legacy_config);
    let legacy_data_present = path_present(&legacy_data);
    let both = (path_present(&primary_config) && legacy_config_present)
        || (path_present(&primary_data) && legacy_data_present);
    (legacy_config_present, legacy_data_present, both)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_snapshot_is_conservative_and_not_fresh_when_legacy_exists() {
        let mut snapshot = EnvironmentSnapshot::default();
        assert!(snapshot.is_fresh());
        snapshot.legacy_data_present = true;
        assert!(!snapshot.is_fresh());
    }

    #[test]
    fn missing_sources_are_attention_not_empty_library() {
        let mut snapshot = EnvironmentSnapshot {
            source_count: 1,
            ..EnvironmentSnapshot::default()
        };
        snapshot
            .unavailable_sources
            .push(PathBuf::from("/mnt/games"));
        assert!(snapshot.source_needs_attention());
        assert!(!snapshot.is_fresh());
    }
}
