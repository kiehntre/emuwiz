//! The Launch page's emulator choice controls over the shared core facility
//! ([`archivefs_core::launch::launch_choice`]).
//!
//! Each choosable candidate card offers "Use once", "Default for this game"
//! and "Default for <system>". Only a candidate that would produce a real
//! launch request once chosen is offered; choosing never bypasses readiness,
//! and launch preflight revalidates everything again. A one-time choice is
//! kept in memory only; defaults are saved to the versioned preferences file.
use std::path::PathBuf;

use archivefs_core::launch::launch_choice::{
    self as core_choice, LaunchChoiceInputs, LaunchChoiceSource, LaunchOnceChoice,
    LaunchPreference, LaunchPreferenceScope,
};
use archivefs_core::launch::{LaunchCandidate, LaunchPlan};
use eframe::egui;

/// Saved preferences (or why they could not be read) plus the unsaved
/// one-time choice. `Default` reads nothing from disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaunchChoiceState {
    pub(crate) preferences: Result<Vec<LaunchPreference>, String>,
    pub(crate) once: Option<LaunchOnceChoice>,
    /// The last save failure, shown until the next successful change.
    pub(crate) save_error: Option<String>,
    path: Option<PathBuf>,
    /// Whether a RetroArch or Dolphin launch was active when last observed.
    launch_was_active: bool,
}

impl Default for LaunchChoiceState {
    fn default() -> Self {
        Self {
            preferences: Ok(Vec::new()),
            once: None,
            save_error: None,
            path: None,
            launch_was_active: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LaunchChoiceAction {
    UseOnce(LaunchOnceChoice),
    SetDefault(LaunchPreference),
    ClearDefault {
        scope: LaunchPreferenceScope,
        platform_id: String,
        game_key: Option<String>,
    },
}

impl LaunchChoiceState {
    /// Reads the user's saved preferences from the EmuWiz config directory.
    pub(crate) fn load_default() -> Self {
        match core_choice::default_launch_preferences_path() {
            Ok(path) => Self::load_at(path),
            Err(error) => Self {
                preferences: Err(error),
                ..Self::default()
            },
        }
    }

    pub(crate) fn load_at(path: PathBuf) -> Self {
        Self {
            preferences: core_choice::load_launch_preferences_at(&path),
            once: None,
            save_error: None,
            path: Some(path),
            launch_was_active: false,
        }
    }

    pub(crate) fn inputs(&self) -> LaunchChoiceInputs<'_> {
        LaunchChoiceInputs {
            once: self.once.as_ref(),
            preferences: self.preferences.as_deref().map_err(String::as_str),
        }
    }

    /// Call once per frame, before the launch states are polled, with whether
    /// any launch that a choice can apply to (RetroArch or Dolphin) is
    /// starting or running. A one-time choice is used up by the launch that
    /// starts after it was made, from any Play or Launch button.
    pub(crate) fn observe_launches(&mut self, launch_active: bool) {
        if launch_active && !self.launch_was_active {
            self.once = None;
        }
        self.launch_was_active = launch_active;
    }

    /// Applies one explicit user action. Saved changes are written atomically
    /// and then reloaded from disk, so the page shows exactly what was stored.
    pub(crate) fn handle(&mut self, action: LaunchChoiceAction) {
        let game = match &action {
            LaunchChoiceAction::UseOnce(once) => {
                self.once = Some(once.clone());
                self.save_error = None;
                return;
            }
            LaunchChoiceAction::SetDefault(preference) => {
                (preference.platform_id.clone(), preference.game_key.clone())
            }
            LaunchChoiceAction::ClearDefault {
                platform_id,
                game_key,
                ..
            } => (platform_id.clone(), game_key.clone()),
        };
        let Some(path) = self.path.clone() else {
            self.save_error = Some("Emulator choices cannot be saved in this session.".into());
            return;
        };
        let result = match &action {
            LaunchChoiceAction::SetDefault(preference) => {
                core_choice::set_launch_preference_at(&path, preference)
            }
            LaunchChoiceAction::ClearDefault {
                scope,
                platform_id,
                game_key,
            } => core_choice::clear_launch_preference_at(
                &path,
                *scope,
                platform_id,
                game_key.as_deref(),
            ),
            LaunchChoiceAction::UseOnce(_) => unreachable!("handled above"),
        };
        // A saved change for this game (or its system) supersedes a one-time
        // choice that would otherwise keep outranking it.
        if result.is_ok()
            && self.once.as_ref().is_some_and(|once| {
                once.platform_id == game.0
                    && game.1.as_deref().is_none_or(|key| key == once.game_key)
            })
        {
            self.once = None;
        }
        self.save_error = result.err();
        self.preferences = core_choice::load_launch_preferences_at(&path);
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }
}

fn source_label(source: LaunchChoiceSource, platform_id: &str) -> String {
    match source {
        LaunchChoiceSource::Once => "Chosen for the next launch of this game (not saved)".into(),
        LaunchChoiceSource::Game => "Default for this game".into(),
        LaunchChoiceSource::System => format!("Default for {platform_id} games"),
    }
}

/// The choice controls for one candidate card. `launchable_if_chosen` must
/// come from the same launch-request gate the Launch buttons use.
pub(super) fn show_controls(
    ui: &mut egui::Ui,
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
    state: &LaunchChoiceState,
    launchable_if_chosen: bool,
) -> Option<LaunchChoiceAction> {
    let (Some(platform_id), Some(game_key)) = (&plan.platform_id, &plan.game_key) else {
        return None;
    };
    if !core_choice::is_choosable(plan, candidate) {
        return None;
    }
    let option_id = core_choice::launch_option_id(&candidate.target);
    let chosen = core_choice::chosen_source(plan, candidate, state.inputs());
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        if let Some(source) = chosen {
            ui.label(egui::RichText::new(source_label(source, platform_id)).strong());
            let clear = match source {
                LaunchChoiceSource::Once => None,
                LaunchChoiceSource::Game => {
                    Some((LaunchPreferenceScope::Game, "Forget game default"))
                }
                LaunchChoiceSource::System => {
                    Some((LaunchPreferenceScope::System, "Forget system default"))
                }
            };
            if let Some((scope, label)) = clear
                && ui.button(label).clicked()
            {
                action = Some(LaunchChoiceAction::ClearDefault {
                    scope,
                    platform_id: platform_id.clone(),
                    game_key: (scope == LaunchPreferenceScope::Game).then(|| game_key.clone()),
                });
            }
        }
        if !launchable_if_chosen {
            ui.label(
                egui::RichText::new(
                    "EmuWiz cannot launch this option here, so it cannot be chosen.",
                )
                .small(),
            );
            return;
        }
        if chosen != Some(LaunchChoiceSource::Once) && ui.button("Use once").clicked() {
            action = Some(LaunchChoiceAction::UseOnce(LaunchOnceChoice {
                platform_id: platform_id.clone(),
                game_key: game_key.clone(),
                option_id: option_id.clone(),
            }));
        }
        if chosen != Some(LaunchChoiceSource::Game) && ui.button("Default for this game").clicked()
        {
            action = Some(LaunchChoiceAction::SetDefault(LaunchPreference::game(
                platform_id,
                game_key,
                &option_id,
            )));
        }
        if chosen != Some(LaunchChoiceSource::System)
            && ui.button(format!("Default for {platform_id}")).clicked()
        {
            action = Some(LaunchChoiceAction::SetDefault(LaunchPreference::system(
                platform_id,
                &option_id,
            )));
        }
    });
    action
}

/// Page-level notices: unreadable saved choices or a failed save.
pub(super) fn show_notices(ui: &mut egui::Ui, plan: &LaunchPlan, state: &LaunchChoiceState) {
    if !core_choice::is_adopted(plan) {
        return;
    }
    if let Err(error) = &state.preferences {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("Saved emulator choices cannot be read: {error}. Fix or remove the file, or use an emulator once."),
        );
    }
    if let Some(error) = &state.save_error {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("Your emulator choice was not saved: {error}"),
        );
    }
}

/// The plan as it would be if `candidate` were chosen once, so the caller can
/// ask the ordinary launch gate whether choosing it would offer Launch.
pub(super) fn plan_if_chosen(plan: &LaunchPlan, candidate: &LaunchCandidate) -> Option<LaunchPlan> {
    let (Some(platform_id), Some(game_key)) = (&plan.platform_id, &plan.game_key) else {
        return None;
    };
    let once = LaunchOnceChoice {
        platform_id: platform_id.clone(),
        game_key: game_key.clone(),
        option_id: core_choice::launch_option_id(&candidate.target),
    };
    let mut simulated = plan.clone();
    // Clear earlier holds so only this hypothetical choice decides.
    for other in &mut simulated.candidates {
        other.warnings.retain(|warning| {
            warning.kind
                != archivefs_core::launch::LaunchWarningKind::PreferredLaunchChoiceUnavailable
        });
    }
    matches!(
        core_choice::apply(
            &mut simulated,
            LaunchChoiceInputs {
                once: Some(&once),
                preferences: Ok(&[]),
            },
        ),
        core_choice::LaunchChoiceOutcome::Applied { .. }
    )
    .then_some(simulated)
}

#[cfg(test)]
mod tests;
