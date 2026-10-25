//! RetroArch's installation picker. Discovery and preflight own eligibility;
//! this control only saves their validated choice beside the core override.
use archivefs_core::emulator_environment::HostReadOnlyFilesystem;
use archivefs_core::emulator_environment::retroarch::installation_selection::{
    EXECUTABLE_OVERRIDE_FILE, InstallationExecutable, classify_installation_executable,
    load_executable_override_at,
};
use archivefs_core::emulator_environment::retroarch::{
    AppImageIdentificationConfidence, ConfigAssociation, DiscoveryEnvironment, ExecutableState,
    ProfileKind, RetroArchEnvironmentReport, discover_retroarch_environment,
};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// Only the EmuWiz sidecar is written; emulator installations are untouched.
fn save_executable_override_at(path: &Path, selected: Option<&Path>) -> Result<(), String> {
    if let Some(selected) = selected {
        let text = selected
            .to_str()
            .filter(|text| !text.contains(['\n', '\r']) && selected.is_absolute())
            .ok_or("Choose an absolute UTF-8 program path")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        // Reject an existing link or non-file rather than overwriting its target.
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if !metadata.is_file() => {
                return Err("RetroArch selection file is not a regular file".into());
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(error.to_string());
            }
            _ => {}
        }
        use std::io::Write;
        // Publish one complete sidecar atomically: an interrupted save must
        // not expose a transient blank file (which means automatic).
        let parent = path
            .parent()
            .ok_or("Selection file has no parent directory")?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        temporary
            .write_all(format!("{text}\n").as_bytes())
            .map_err(|e| e.to_string())?;
        temporary.as_file().sync_all().map_err(|e| e.to_string())?;
        temporary.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    } else {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Installation {
    program: PathBuf,
    settings: String,
    kind: &'static str,
}

enum Update {
    Available(Vec<Installation>, Vec<String>),
    Saved,
}

#[derive(Default)]
pub(crate) struct InstallationPicker {
    pending: Option<Receiver<Result<Update, String>>>,
    saving: bool,
    loaded: bool,
    choices: Vec<Installation>,
    unsupported: Vec<String>,
    message: Option<String>,
}

fn supported_choices(report: &RetroArchEnvironmentReport) -> Vec<Installation> {
    let mut choices = Vec::new();
    for profile in &report.profiles {
        if profile.config_file.path.lossy {
            continue;
        }
        if profile.profile_kind == ProfileKind::Native {
            for program in &profile.evidence.executables {
                if !program.lossy {
                    choices.push(Installation {
                        program: PathBuf::from(&program.display),
                        settings: profile.config_file.path.display.clone(),
                        kind: "Native",
                    });
                }
            }
        }
        for image in &profile.app_images {
            if image.confidence == AppImageIdentificationConfidence::Exact
                && image.executable == Some(ExecutableState::Executable)
                && !image.path.lossy
                && !matches!(
                    image.config_association,
                    ConfigAssociation::Unknown | ConfigAssociation::Ambiguous
                )
                && classify_installation_executable(
                    &HostReadOnlyFilesystem,
                    image.path.display.as_ref(),
                ) == InstallationExecutable::Native
            {
                choices.push(Installation {
                    program: PathBuf::from(&image.path.display),
                    settings: profile.config_file.path.display.clone(),
                    kind: "AppImage",
                });
            }
        }
    }
    choices.sort_by(|left, right| left.program.cmp(&right.program));
    choices.dedup_by(|left, right| left.program == right.program);
    choices
}

impl InstallationPicker {
    /// Called by the regular controller update, even when Setup is hidden.
    /// Every completed save attempt requires fresh discovery: a failure must
    /// not leave cached authority for a stale or unreadable on-disk choice.
    pub(crate) fn poll(&mut self, context: &egui::Context) -> bool {
        let mut refresh = false;
        if let Some(receiver) = &self.pending {
            match receiver.try_recv() {
                Ok(result) => {
                    self.pending = None;
                    refresh = std::mem::take(&mut self.saving);
                    match result {
                        Ok(Update::Saved) => {
                            self.message = None;
                        }
                        Ok(Update::Available(choices, unsupported)) => {
                            self.choices = choices;
                            self.unsupported = unsupported;
                            self.message = None;
                        }
                        Err(message) => self.message = Some(message),
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.pending = None;
                    refresh = std::mem::take(&mut self.saving);
                    self.message =
                        Some("The installation check stopped. Try checking again.".into());
                }
                Err(TryRecvError::Empty) => {
                    context.request_repaint();
                }
            }
        }
        refresh
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        if !self.loaded {
            self.scan(context.clone());
        }
        ui.label("RetroArch installation");
        let override_file = archivefs_core::app_dirs::config_path(EXECUTABLE_OVERRIDE_FILE);
        let current = override_file
            .as_ref()
            .map_err(ToString::to_string)
            .and_then(|path| load_executable_override_at(path));
        match &current {
            Ok(Some(path)) => {
                ui.label(format!("Selected: {}", path.display()));
            }
            Ok(None) => {
                ui.label(
                    "Automatic detection. Multiple native programs require an explicit choice.",
                );
            }
            Err(message) => {
                ui.colored_label(ui.visuals().error_fg_color, message);
            }
        }
        ui.label("Choose a discovered native program or verified AppImage. The program and settings shown below are used together. Emulator installations and settings are not changed.");
        if self.choices.is_empty() && self.pending.is_none() {
            ui.label("No supported RetroArch installation was found. Install a native RetroArch program or a verified AppImage, then check again.");
        }
        let mut picked = None;
        for choice in &self.choices {
            let selected = current.as_ref().ok().and_then(Option::as_ref) == Some(&choice.program);
            if ui
                .add_enabled(
                    self.pending.is_none(),
                    egui::Button::new(format!("{}: {}", choice.kind, choice.program.display()))
                        .selected(selected),
                )
                .clicked()
            {
                picked = Some(choice.program.clone());
            }
            ui.small(format!("Settings: {}", choice.settings));
        }
        if let Some(picked) = picked {
            self.start(Some(picked), context.clone());
        }
        for unsupported in &self.unsupported {
            ui.label(format!("Read-only: {unsupported}"));
        }
        ui.small(
            "Flatpak and unresolved launcher scripts cannot be selected for Play or bezel Apply.",
        );
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    self.pending.is_none(),
                    egui::Button::new("Check installations again"),
                )
                .clicked()
            {
                self.scan(context.clone());
            }
            if !matches!(current, Ok(None))
                && ui
                    .add_enabled(
                        self.pending.is_none(),
                        egui::Button::new("Use automatic installation detection"),
                    )
                    .clicked()
            {
                self.start(None, context.clone());
            }
        });
        if self.pending.is_some() {
            ui.spinner();
            ui.label("Checking RetroArch installations…");
        }
        if let Some(message) = &self.message {
            ui.colored_label(ui.visuals().error_fg_color, message);
        }
    }

    fn scan(&mut self, context: egui::Context) {
        self.loaded = true;
        let (sender, receiver) = mpsc::channel();
        self.pending = Some(receiver);
        std::thread::spawn(move || {
            let mut environment = DiscoveryEnvironment::from_process_environment();
            // Inventory choices using existing discovery before applying the
            // saved selection; no second installation finder exists here.
            environment.explicit_executables.clear();
            let result = discover_retroarch_environment(&HostReadOnlyFilesystem, &environment)
                .map(|report| {
                    let choices = supported_choices(&report);
                    let mut unsupported: Vec<_> = report
                        .profiles
                        .iter()
                        .filter(|profile| {
                            profile.profile_kind == ProfileKind::Flatpak
                                && (profile.evidence.flatpak_metadata_found
                                    || !profile.evidence.executables.is_empty())
                        })
                        .map(|profile| {
                            format!(
                                "Flatpak ({:?}); {}",
                                profile.scope, profile.config_file.path.display
                            )
                        })
                        .collect();
                    unsupported.extend(
                        report
                            .diagnostics
                            .iter()
                            .filter(|diagnostic| {
                                diagnostic.code == "retroarch_launcher_script_unresolved"
                            })
                            .filter_map(|diagnostic| diagnostic.path.as_ref())
                            .map(|path| format!("Unresolved launcher: {}", path.display)),
                    );
                    Update::Available(choices, unsupported)
                })
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    pub(crate) fn start(&mut self, selected: Option<PathBuf>, context: egui::Context) {
        self.loaded = true;
        self.saving = true;
        let (sender, receiver) = mpsc::channel();
        self.pending = Some(receiver);
        self.message = None;
        std::thread::spawn(move || {
            let result = (|| {
                let override_file = archivefs_core::app_dirs::config_path(EXECUTABLE_OVERRIDE_FILE)
                    .map_err(|e| e.to_string())?;
                if let Some(selected) = &selected {
                    let mut environment = DiscoveryEnvironment::from_process_environment();
                    environment.explicit_executables = vec![selected.clone()];
                    discover_retroarch_environment(&HostReadOnlyFilesystem, &environment)
                        .map_err(|e| e.to_string())?;
                }
                save_executable_override_at(&override_file, selected.as_deref())?;
                Ok(Update::Saved)
            })();
            let _ = sender.send(result);
            context.request_repaint();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn selection_sidecar_round_trips_atomically_and_rejects_a_link() {
        let root = tempfile::tempdir().unwrap();
        let sidecar = root.path().join("emuwiz/retroarch_executable_override.txt");
        let selected = root.path().join("bin/retroarch");
        save_executable_override_at(&sidecar, Some(&selected)).unwrap();
        assert_eq!(
            load_executable_override_at(&sidecar).unwrap(),
            Some(selected)
        );
        save_executable_override_at(&sidecar, None).unwrap();
        assert_eq!(load_executable_override_at(&sidecar).unwrap(), None);
        let target = root.path().join("untouched");
        fs::write(&target, "untouched").unwrap();
        std::os::unix::fs::symlink(&target, &sidecar).unwrap();
        assert!(save_executable_override_at(&sidecar, Some(Path::new("/another"))).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "untouched");
    }

    #[test]
    fn setup_offers_native_and_verified_appimage_with_their_own_settings_only() {
        let root = tempfile::tempdir().unwrap();
        for directory in [
            "bin",
            "wrapper",
            "Applications",
            "desktop",
            "Applications/RetroArch.AppImage.config/retroarch",
        ] {
            fs::create_dir_all(root.path().join(directory)).unwrap();
        }
        let native = root.path().join("bin/retroarch");
        let image = root.path().join("Applications/RetroArch.AppImage");
        let wrapper = root.path().join("wrapper/retroarch");
        fs::write(&native, b"\x7fELFnative").unwrap();
        fs::write(&image, b"\x7fELFimage").unwrap();
        fs::write(
            &wrapper,
            "#!/bin/sh\nexec flatpak run org.libretro.RetroArch \"$@\"\n",
        )
        .unwrap();
        for executable in [&native, &image, &wrapper] {
            fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(
            root.path().join("desktop/retroarch.desktop"),
            format!(
                "[Desktop Entry]\nType=Application\nName=RetroArch\nExec={}\n",
                image.display()
            ),
        )
        .unwrap();
        let environment = DiscoveryEnvironment {
            home: Some(root.path().into()),
            xdg_config_home: None,
            path: Some(
                std::env::join_paths([root.path().join("bin"), root.path().join("wrapper")])
                    .unwrap(),
            ),
            explicit_executables: Vec::new(),
            user_flatpak_root: root.path().join("user-flatpak"),
            system_flatpak_root: root.path().join("system-flatpak"),
            app_image_search_roots: vec![root.path().join("Applications")],
            desktop_file_roots: vec![root.path().join("desktop")],
        };
        let report = discover_retroarch_environment(&HostReadOnlyFilesystem, &environment).unwrap();
        let choices = supported_choices(&report);
        assert_eq!(choices.len(), 2);
        let native = choices
            .iter()
            .find(|choice| choice.program == native)
            .unwrap();
        let image = choices
            .iter()
            .find(|choice| choice.program == image)
            .unwrap();
        assert_eq!(native.kind, "Native");
        assert_eq!(image.kind, "AppImage");
        assert!(native.settings.ends_with(".config/retroarch/retroarch.cfg"));
        assert!(
            image
                .settings
                .ends_with("RetroArch.AppImage.config/retroarch/retroarch.cfg")
        );
        assert!(!choices.iter().any(|choice| choice.program == wrapper));
    }
}
