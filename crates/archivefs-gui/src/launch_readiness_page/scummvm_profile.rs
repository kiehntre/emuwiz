//! Optional, explicit use of a saved ScummVM trainer profile when launching a
//! ScummVM game from the Playing Library.
//!
//! The user checks the saved profile for the game on screen and must then tick
//! "use it" (never on by default, and cleared after every launch attempt).
//! Everything that matters is decided in core
//! (`archivefs_core::launch::preflight_and_launch_with_trainer_profile`): the
//! profile bytes are re-read and compared with the fingerprint the user saw,
//! detection and the executable are re-checked, and ScummVM runs on a private
//! copy so neither the saved profile nor the user's own ScummVM configuration
//! or saves are rewritten. ScummVM reports nothing about which options it
//! applied, so a launch never claims a trainer effect.
use super::{StandaloneLaunchRequest, StandaloneLaunchState, StandaloneProcess};
use crate::cheats_mods::scummvm_trainer::{PROFILE_TARGET, owned_profile_path};
use archivefs_core::launch::{
    ScummVmLaunchRequest, TrainerLaunchRequest, TrainerProfileState, inspect_trainer_profile,
    preflight_and_launch_with_trainer_profile,
};
use eframe::egui;
use std::path::{Path, PathBuf};

pub(super) const START_NOTE: &str = "ScummVM runs from a private copy of the saved profile that is removed after it exits. Your own ScummVM configuration, the saved profile, your saves and the game files are not modified by EmuWiz. ScummVM does not say which options it applied, so EmuWiz cannot confirm any trainer effect. This launch uses the saved profile, your save location and a short list of your own ScummVM settings (audio and display values, a theme name, and a soundfont, theme or extra path only when it is a real, unlinked location). Anything else in your configuration is not carried over, and after launch EmuWiz lists what was left out and why.";

/// Where a launch reads from; injectable so tests never touch real homes.
#[derive(Clone)]
pub(super) struct Environment {
    pub(super) profile: PathBuf,
    pub(super) original_configuration: Option<PathBuf>,
    pub(super) default_savepath: Option<PathBuf>,
    pub(super) scratch_root: PathBuf,
}

impl Environment {
    fn managed(folder: &Path, id: &str) -> Result<Self, String> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let dir = |var: &str, fallback: &str| {
            std::env::var_os(var)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .or_else(|| home.as_ref().map(|h| h.join(fallback)))
        };
        let config = dir("XDG_CONFIG_HOME", ".config").map(|d| d.join("scummvm/scummvm.ini"));
        Ok(Self {
            profile: owned_profile_path(folder, id)?,
            original_configuration: config
                .into_iter()
                .chain(home.as_ref().map(|h| h.join(".scummvmrc")))
                .find(|p| std::fs::symlink_metadata(p).is_ok()),
            default_savepath: dir("XDG_DATA_HOME", ".local/share").map(|d| d.join("scummvm/saves")),
            scratch_root: archivefs_core::app_dirs::data_path("scummvm-trainer-launch")
                .map_err(|e| e.to_string())?,
        })
    }
}

#[derive(Clone)]
struct Checked {
    folder: PathBuf,
    id: String,
    env: Environment,
    state: TrainerProfileState,
}

#[derive(Default)]
pub(super) struct Choice {
    checked: Option<Checked>,
    /// The explicit per-launch tick; false until the user sets it.
    use_profile: bool,
    note: Option<String>,
    #[cfg(test)]
    pub(super) env_override: Option<Environment>,
}

impl Choice {
    fn environment(&self, request: &ScummVmLaunchRequest) -> Result<Environment, String> {
        #[cfg(test)]
        if let Some(env) = &self.env_override {
            return Ok(env.clone());
        }
        Environment::managed(&request.selected_game_folder, &request.expected_game_key)
    }

    pub(super) fn check(&mut self, request: &ScummVmLaunchRequest) {
        self.use_profile = false;
        self.note = None;
        self.checked = match self.environment(request) {
            Ok(env) => Some(Checked {
                folder: request.selected_game_folder.clone(),
                id: request.expected_game_key.clone(),
                state: inspect_trainer_profile(
                    &env.profile,
                    PROFILE_TARGET,
                    &request.expected_game_key,
                    &request.selected_game_folder,
                ),
                env,
            }),
            Err(e) => {
                self.note = Some(format!("The saved profile could not be located: {e}"));
                None
            }
        };
    }

    /// A check for another game is stale and never carried over.
    fn for_request(&self, request: &ScummVmLaunchRequest) -> Option<&Checked> {
        self.checked.as_ref().filter(|c| {
            c.folder == request.selected_game_folder && c.id == request.expected_game_key
        })
    }

    #[cfg(test)]
    pub(super) fn checked_state(
        &self,
        request: &ScummVmLaunchRequest,
    ) -> Option<TrainerProfileState> {
        self.for_request(request).map(|c| c.state.clone())
    }

    pub(super) fn set_use_profile(&mut self, value: bool) {
        self.use_profile = value;
    }

    /// The reviewed profile fingerprint, only when the user ticked it and the
    /// check still belongs to this game and was ready.
    fn selected(&self, request: &ScummVmLaunchRequest) -> Option<(Checked, String)> {
        let checked = self.for_request(request)?;
        match (&checked.state, self.use_profile) {
            (TrainerProfileState::Ready(summary), true) => {
                Some((checked.clone(), summary.sha256.clone()))
            }
            _ => None,
        }
    }
}

/// Starts the launch for a ScummVM request. Returns false when the plain
/// launch applies (no profile ticked), leaving the choice untouched.
pub(super) fn start_with_profile(
    state: &mut StandaloneLaunchState,
    request: &StandaloneLaunchRequest,
) -> Option<std::sync::mpsc::Receiver<Result<StandaloneProcess, String>>> {
    let StandaloneLaunchRequest::ScummVm(plain) = request else {
        return None;
    };
    let (checked, sha256) = state.scummvm_profile.selected(plain)?;
    // One explicit choice, one launch attempt: whatever happens, the next
    // launch starts unticked and needs a fresh check.
    state.scummvm_profile.use_profile = false;
    state.scummvm_profile.checked = None;
    let launch = TrainerLaunchRequest {
        selected_game_folder: plain.selected_game_folder.clone(),
        expected_game_key: plain.expected_game_key.clone(),
        expected_executable: plain.expected_executable.clone(),
        profile: checked.env.profile.clone(),
        target_name: PROFILE_TARGET.into(),
        expected_profile_sha256: sha256,
        original_configuration: checked.env.original_configuration.clone(),
        default_savepath: checked.env.default_savepath.clone(),
        scratch_root: checked.env.scratch_root.clone(),
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = preflight_and_launch_with_trainer_profile(&launch)
            .map(|process| StandaloneProcess::ScummVmTrainer(Box::new(process)))
            .map_err(|error| error.to_string());
        let _ = sender.send(result);
    });
    Some(receiver)
}

/// Chooser shown with the ScummVM Launch button.
pub(super) fn show_chooser(
    ui: &mut egui::Ui,
    state: &mut StandaloneLaunchState,
    request: &StandaloneLaunchRequest,
) {
    let StandaloneLaunchRequest::ScummVm(plain) = request else {
        return;
    };
    if state.is_active() {
        return;
    }
    let choice = &mut state.scummvm_profile;
    ui.small("Saved trainer profile (optional)");
    if ui.button("Check saved trainer profile").clicked() {
        choice.check(plain);
    }
    if let Some(note) = &choice.note {
        ui.label(note);
    }
    let Some(checked) = choice.for_request(plain).cloned() else {
        return;
    };
    match &checked.state {
        TrainerProfileState::Missing if !trainer_options_exist(&checked.id) => {
            ui.label("Trainer options exist only for Hypno-engine ScummVM games (for example Wetlands). This game can be launched normally, but there is nothing to apply.");
        }
        TrainerProfileState::Missing => {
            ui.label("No saved trainer profile exists for this game, so only a plain launch is possible. Save one from Cheats & Mods first.");
        }
        TrainerProfileState::Unreadable(e) => {
            ui.label(format!("The saved profile could not be read safely: {e}"));
        }
        TrainerProfileState::Mismatch(detail) => {
            ui.label(format!(
                "Stale profile: {detail}. Save a profile for this game again."
            ));
        }
        TrainerProfileState::Unsupported(items) => {
            ui.label("The saved profile contains settings EmuWiz does not support, so it cannot be used:");
            for item in items {
                ui.label(format!("• {item}"));
            }
        }
        TrainerProfileState::Ready(summary) => {
            ui.monospace(checked.env.profile.display().to_string());
            ui.label(format!("Profile fingerprint: {}", &summary.sha256[..16]));
            for (key, on) in &summary.options {
                ui.label(format!("{key}: {}", if *on { "On" } else { "Off" }));
            }
            let mut ticked = choice.use_profile;
            if ui
                .checkbox(&mut ticked, "Use this saved profile for the next launch")
                .changed()
            {
                choice.set_use_profile(ticked);
            }
            ui.small(START_NOTE);
        }
    }
}

/// Whether EmuWiz documents any trainer options for this verified game ID.
fn trainer_options_exist(game_id: &str) -> bool {
    archivefs_core::patch_manager::scummvm_trainer_options(game_id, b"").is_ok()
}
