//! Saturn's explicit core preference, separate from RetroArch's own settings.
use std::io::Read;
use std::path::Path;

use archivefs_core::emulator_environment::retroarch::{
    CoreInfoFinding, ProfileKind, RetroArchEnvironmentReport,
};
use archivefs_core::launch::{
    LaunchPlan,
    saturn_core_selection::{self, SaturnCoreSelection},
};
use eframe::egui;

const FILE: &str = "saturn_core_selection.json";

pub(super) fn load_at(path: &Path) -> Result<Option<SaturnCoreSelection>, String> {
    let file = match archivefs_core::launch::process_spawn::open_regular_file_nonblocking(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Saturn core selection cannot be read: {e}")),
    };
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Err("Saturn core selection is too large.".into());
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| format!("Invalid Saturn core selection: {e}"))
}

pub(super) fn save_at(path: &Path, selected: &SaturnCoreSelection) -> Result<(), String> {
    use std::io::Write;
    match std::fs::symlink_metadata(path) {
        Ok(m) if !m.is_file() => return Err("Saturn selection is not a regular file.".into()),
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
        _ => {}
    }
    let parent = path
        .parent()
        .ok_or("Saturn selection has no parent directory.")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    temp.write_all(&serde_json::to_vec(selected).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn load() -> Result<Option<SaturnCoreSelection>, String> {
    load_at(&archivefs_core::app_dirs::config_path(FILE).map_err(|e| e.to_string())?)
}

pub(crate) fn apply(plan: &mut LaunchPlan, environment: &RetroArchEnvironmentReport) {
    if plan.platform_id.as_deref() != Some("Saturn") {
        return;
    }
    let selected = load();
    saturn_core_selection::apply(
        plan,
        environment,
        selected.as_ref().map(|s| s.as_ref()).map_err(Clone::clone),
    );
}

pub(super) fn show(ui: &mut egui::Ui, environment: &RetroArchEnvironmentReport) {
    match archivefs_core::app_dirs::config_path(FILE) {
        Ok(path) => show_at(ui, environment, &path),
        Err(error) => {
            ui.label(error.to_string());
        }
    }
}

fn show_at(ui: &mut egui::Ui, environment: &RetroArchEnvironmentReport, path: &Path) {
    ui.heading("Saturn core");
    ui.label("Choose the native core to use for Saturn games. No core is selected automatically.");
    let selected = load_at(path);
    match &selected {
        Err(error) => {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        Ok(Some(selection)) => {
            if let Err(error) = selection.validate(environment) {
                ui.colored_label(ui.visuals().error_fg_color, error);
            } else {
                ui.label(format!("Selected: {}", selection.core_stem));
            }
        }
        Ok(None) => {
            ui.label("No Saturn core selected.");
        }
    }
    let mut count = 0;
    for profile in &environment.profiles {
        for core in &profile.cores {
            if !archivefs_core::launch::platform_map::retroarch_platform_matches(
                &core.info, "Saturn",
            ) {
                continue;
            }
            count += 1;
            let name = match &core.info {
                CoreInfoFinding::Found {
                    display_name: Some(name),
                    ..
                } => name.as_str(),
                _ => core.core_stem.as_str(),
            };
            let choice = SaturnCoreSelection::capture(profile, core);
            ui.group(|ui| {
                ui.label(format!("{name} ({})", core.core_stem));
                ui.label(&core.full_path.display);
                let availability = if profile.profile_kind != ProfileKind::Native {
                    "Unavailable here: only native RetroArch is supported".to_owned()
                } else {
                    choice
                        .as_ref()
                        .err()
                        .cloned()
                        .unwrap_or_else(|| "Available — native core discovered".into())
                };
                ui.label(availability);
                if ui
                    .add_enabled(choice.is_ok(), egui::Button::new(format!("Use {name}")))
                    .clicked()
                {
                    let result = choice.and_then(|choice| {
                        choice.validate(environment)?;
                        save_at(path, &choice)
                    });
                    if let Err(error) = result {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                    }
                    ui.ctx().request_repaint();
                }
            });
        }
    }
    if count == 0 {
        ui.label("No Saturn core with readable platform metadata was discovered. Check RetroArch in Emulator Setup.");
    }
}

#[cfg(test)]
mod tests;
