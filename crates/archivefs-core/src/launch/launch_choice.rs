//! Shared emulator launch preferences: the user's explicit choice of one
//! launch option for one game or one system.
//!
//! # Precedence
//!
//! 1. A one-time choice for this game (not saved).
//! 2. A saved default for this game.
//! 3. A saved default for this system.
//! 4. The planner's own unambiguous result (a sole eligible or otherwise
//!    already-determined candidate).
//! 5. Otherwise every eligible candidate stays tied
//!    ([`LaunchWarningKind::MultipleEligibleProfiles`]) and the user is asked.
//!
//! # Safety rules
//!
//! - A choice only clears the tie from the one candidate it names: the tie
//!   warning, or the RetroArch core tie ([`LaunchBlockerKind::AmbiguousCore`],
//!   2026-11-10; never for Saturn, which has its own validated selection).
//!   Every other blocker and warning survives, a candidate blocked for any
//!   other reason is never made usable, and launch preflight still
//!   revalidates from scratch.
//! - A choice that names a missing, changed or blocked option never falls
//!   back to another one: every remaining candidate receives
//!   [`LaunchWarningKind::PreferredLaunchChoiceUnavailable`] (so strict
//!   launch gates refuse) until the user chooses again. Unreadable saved
//!   choices are treated the same way.
//! - Every platform uses this facility (2026-11-09; first adopted for
//!   GameCube and Wii). Before that, any other system with more than one
//!   usable emulator showed "Choice needed" with no way to choose, so Play
//!   was never offered (owner screenshot: PS2 with native, Flatpak and
//!   portable PCSX2). A plan without a platform is returned unchanged.
//!
//! # Identity
//!
//! Options are stored by [`launch_option_id`], built only from facts the
//! planner and preflight already treat as the option's identity - never a
//! display name or a list position. Game scope uses the plan's verified
//! canonical `(platform_id, game_key)`, never a filename.
//!
//! Choices live in their own versioned file. They are deliberately separate
//! from the Cheats & Mods remembered profiles (`emulator_profiles.toml`),
//! whose ids belong to a different discovery namespace; that file is never
//! read, rewritten or migrated here.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::planning::{CandidatePreference, LaunchCandidate, LaunchPlan, LaunchTarget};
use super::readiness::{LaunchBlockerKind, LaunchReadiness, LaunchWarning, LaunchWarningKind};
use crate::emulator_environment::retroarch::{ProfileKind, ProfileScope};

/// File name inside the EmuWiz configuration directory.
pub const LAUNCH_PREFERENCES_FILE: &str = "launch_preferences.json";
/// The platforms that first used this facility; kept for reference only.
/// Every platform now uses it (see [`is_adopted`]).
pub const ADOPTED_PLATFORMS: &[&str] = &["GameCube", "Wii"];
const MAX_FILE_BYTES: u64 = 64 * 1024;
const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchPreferenceScope {
    /// Default for every game of `platform_id`.
    System,
    /// Override for the one game `(platform_id, game_key)`.
    Game,
}

/// One saved preference. `game_key` is present exactly for game scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchPreference {
    pub scope: LaunchPreferenceScope,
    pub platform_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_key: Option<String>,
    /// See [`launch_option_id`].
    pub option_id: String,
}

impl LaunchPreference {
    pub fn system(platform_id: &str, option_id: &str) -> Self {
        Self {
            scope: LaunchPreferenceScope::System,
            platform_id: platform_id.into(),
            game_key: None,
            option_id: option_id.into(),
        }
    }

    pub fn game(platform_id: &str, game_key: &str, option_id: &str) -> Self {
        Self {
            scope: LaunchPreferenceScope::Game,
            platform_id: platform_id.into(),
            game_key: Some(game_key.into()),
            option_id: option_id.into(),
        }
    }

    fn same_slot(&self, other: &Self) -> bool {
        self.scope == other.scope
            && self.platform_id == other.platform_id
            && self.game_key == other.game_key
    }

    fn validate(&self) -> Result<(), String> {
        let shape_ok = match self.scope {
            LaunchPreferenceScope::System => self.game_key.is_none(),
            LaunchPreferenceScope::Game => self.game_key.as_deref().is_some_and(|k| !k.is_empty()),
        };
        if !shape_ok || self.platform_id.is_empty() || self.option_id.is_empty() {
            return Err("An emulator preference needs a platform, an option and, for one game, that game's identity.".into());
        }
        Ok(())
    }
}

/// A choice for one game that is used but never saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchOnceChoice {
    pub platform_id: String,
    pub game_key: String,
    pub option_id: String,
}

/// Where the effective choice for a plan came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchChoiceSource {
    Once,
    Game,
    System,
}

impl LaunchChoiceSource {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Once => "one-time emulator choice for this game",
            Self::Game => "saved emulator default for this game",
            Self::System => "saved emulator default for this system",
        }
    }
}

/// Everything [`apply`] needs: the unsaved one-time choice, if any, and the
/// saved preferences (or why they could not be read).
#[derive(Debug, Clone, Copy)]
pub struct LaunchChoiceInputs<'a> {
    pub once: Option<&'a LaunchOnceChoice>,
    pub preferences: Result<&'a [LaunchPreference], &'a str>,
}

/// What [`apply`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchChoiceOutcome {
    /// This platform does not use the shared facility yet.
    NotAdopted,
    /// No choice applies; the planner's own result stands.
    NoChoice,
    /// The choice named a usable candidate; it is now the chosen one.
    Applied {
        source: LaunchChoiceSource,
        option_id: String,
    },
    /// The choice names a missing, changed or blocked option. Nothing else
    /// was substituted; every candidate now waits for a new choice.
    PreferredUnavailable {
        source: LaunchChoiceSource,
        option_id: String,
    },
    /// Saved preferences could not be read; nothing is launched until fixed
    /// or until the user makes a one-time choice.
    PreferencesUnreadable(String),
}

/// A stable identity for one launch option, built only from facts the
/// planner and launch preflight already treat as that option's identity.
/// Standalone: adapter and discovered profile id. RetroArch: profile kind,
/// scope and core stem (the same triple preflight matches).
pub fn launch_option_id(target: &LaunchTarget) -> String {
    match target {
        LaunchTarget::Standalone {
            adapter_id,
            profile_id,
            ..
        } => format!("standalone/{adapter_id}/{profile_id}"),
        LaunchTarget::RetroArchCore {
            profile, core_stem, ..
        } => retroarch_option_id(*profile, core_stem),
    }
}

/// The [`launch_option_id`] of the RetroArch candidate `profile` + `core_stem`.
pub fn retroarch_option_id(
    profile: crate::emulator_environment::retroarch::ProfileRef,
    core_stem: &str,
) -> String {
    let kind = match profile.profile_kind {
        ProfileKind::Native => "native",
        ProfileKind::AppImage => "appimage",
        ProfileKind::Flatpak => "flatpak",
    };
    let scope = match profile.scope {
        ProfileScope::User => "user",
        ProfileScope::System => "system",
    };
    format!("retroarch/{kind}/{scope}/{core_stem}")
}

/// A readable name for a saved [`launch_option_id`], so a choice that can no
/// longer be used says which emulator it named. Built only from the stored
/// id (the option itself may be gone); an unrecognised id is shown as stored.
pub fn describe_option_id(option_id: &str) -> String {
    if let Some(rest) = option_id.strip_prefix("standalone/")
        && let Some((adapter, profile)) = rest.split_once('/')
    {
        return match profile.find('/') {
            Some(at) => format!("{adapter} at {}", &profile[at..]),
            None => format!("{adapter} ({profile})"),
        };
    }
    if let Some(rest) = option_id.strip_prefix("retroarch/") {
        let parts: Vec<&str> = rest.splitn(3, '/').collect();
        if let [kind, scope, core] = parts.as_slice() {
            return format!("RetroArch core {core} ({kind}, {scope})");
        }
    }
    option_id.to_owned()
}

pub fn is_adopted(plan: &LaunchPlan) -> bool {
    plan.platform_id.is_some()
}

/// Whether the user may choose `candidate` for `plan`: an adopted platform
/// with a game key, and a candidate that is not blocked - or is blocked only
/// because several installed RetroArch cores fit and none is
/// reviewed-preferred ([`LaunchBlockerKind::AmbiguousCore`]).
pub fn is_choosable(plan: &LaunchPlan, candidate: &LaunchCandidate) -> bool {
    is_adopted(plan) && plan.game_key.is_some() && usable_if_chosen(plan, candidate)
}

/// A candidate a choice may name: not blocked, or blocked by nothing but the
/// core tie. That tie means "do not pick a core automatically"; the person's
/// explicit choice is exactly the answer it waits for (as the reviewed native
/// Saturn core selection already does). Saturn keeps its own validated core
/// authority, and every other blocker - firmware, content, executable,
/// a core installed twice under one name - still refuses.
fn usable_if_chosen(plan: &LaunchPlan, candidate: &LaunchCandidate) -> bool {
    candidate.readiness != LaunchReadiness::Blocked
        || (plan.platform_id.as_deref() != Some("Saturn")
            && !candidate.blockers.is_empty()
            && candidate
                .blockers
                .iter()
                .all(|blocker| blocker.kind == LaunchBlockerKind::AmbiguousCore))
}

/// The choice that applies to `(platform_id, game_key)` by precedence, or
/// the reason saved preferences cannot be consulted.
pub fn effective_choice<'a>(
    platform_id: &str,
    game_key: Option<&str>,
    inputs: LaunchChoiceInputs<'a>,
) -> Result<Option<(LaunchChoiceSource, &'a str)>, String> {
    if let (Some(once), Some(game_key)) = (inputs.once, game_key)
        && once.platform_id == platform_id
        && once.game_key == game_key
    {
        return Ok(Some((LaunchChoiceSource::Once, once.option_id.as_str())));
    }
    let preferences = inputs.preferences.map_err(str::to_owned)?;
    let find = |scope: LaunchPreferenceScope, key: Option<&str>| {
        preferences.iter().find(|preference| {
            preference.scope == scope
                && preference.platform_id == platform_id
                && preference.game_key.as_deref() == key
        })
    };
    if let Some(game_key) = game_key
        && let Some(preference) = find(LaunchPreferenceScope::Game, Some(game_key))
    {
        return Ok(Some((
            LaunchChoiceSource::Game,
            preference.option_id.as_str(),
        )));
    }
    Ok(find(LaunchPreferenceScope::System, None)
        .map(|preference| (LaunchChoiceSource::System, preference.option_id.as_str())))
}

/// Applies the effective choice to `plan`. See the module documentation.
pub fn apply(plan: &mut LaunchPlan, inputs: LaunchChoiceInputs<'_>) -> LaunchChoiceOutcome {
    if !is_adopted(plan) {
        return LaunchChoiceOutcome::NotAdopted;
    }
    let platform_id = plan.platform_id.clone().unwrap_or_default();
    let outcome = match effective_choice(&platform_id, plan.game_key.as_deref(), inputs) {
        Err(error) => {
            hold_for_new_choice(
                plan,
                &format!("Saved emulator choices cannot be read, so none is used: {error}"),
            );
            LaunchChoiceOutcome::PreferencesUnreadable(error)
        }
        Ok(None) => LaunchChoiceOutcome::NoChoice,
        Ok(Some((source, option_id))) => {
            let option_id = option_id.to_owned();
            let usable = plan.candidates.iter().position(|candidate| {
                usable_if_chosen(plan, candidate)
                    && launch_option_id(&candidate.target) == option_id
            });
            match usable {
                Some(index) => {
                    choose(plan, index);
                    LaunchChoiceOutcome::Applied { source, option_id }
                }
                None => {
                    let state = if plan
                        .candidates
                        .iter()
                        .any(|candidate| launch_option_id(&candidate.target) == option_id)
                    {
                        "is blocked"
                    } else {
                        "is no longer available"
                    };
                    hold_for_new_choice(
                        plan,
                        &format!(
                            "The emulator in your {} ({}) {state}. No other emulator is used instead; choose one to continue.",
                            source.describe(),
                            describe_option_id(&option_id)
                        ),
                    );
                    LaunchChoiceOutcome::PreferredUnavailable { source, option_id }
                }
            }
        }
    };
    plan.summary = super::planning::summarize(&plan.candidates);
    outcome
}

/// Whether `candidate` is the one the effective choice made usable.
pub fn chosen_source(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
    inputs: LaunchChoiceInputs<'_>,
) -> Option<LaunchChoiceSource> {
    if !is_adopted(plan) || candidate.preference != CandidatePreference::Remembered {
        return None;
    }
    let platform_id = plan.platform_id.as_deref()?;
    match effective_choice(platform_id, plan.game_key.as_deref(), inputs) {
        Ok(Some((source, option_id))) if option_id == launch_option_id(&candidate.target) => {
            Some(source)
        }
        _ => None,
    }
}

fn choose(plan: &mut LaunchPlan, index: usize) {
    for (position, candidate) in plan.candidates.iter_mut().enumerate() {
        if position == index {
            // Only the tie the person just answered; see `usable_if_chosen`.
            candidate
                .blockers
                .retain(|blocker| blocker.kind != LaunchBlockerKind::AmbiguousCore);
            candidate.warnings.retain(|warning| {
                !matches!(
                    warning.kind,
                    LaunchWarningKind::MultipleEligibleProfiles
                        | LaunchWarningKind::PreferredLaunchChoiceUnavailable
                )
            });
            candidate.preference = CandidatePreference::Remembered;
        } else if matches!(
            candidate.preference,
            CandidatePreference::Remembered | CandidatePreference::SoleEligible
        ) {
            // An explicit launch choice outranks any other remembered or
            // reviewed-hint preference; that option stays choosable.
            candidate.preference = CandidatePreference::Undetermined;
            candidate.warnings.push(LaunchWarning::new(
                LaunchWarningKind::MultipleEligibleProfiles,
                "a different emulator is chosen for this game",
            ));
        }
        recompute(candidate);
    }
}

fn hold_for_new_choice(plan: &mut LaunchPlan, detail: &str) {
    for candidate in &mut plan.candidates {
        if candidate.readiness == LaunchReadiness::Blocked {
            continue;
        }
        if candidate.preference == CandidatePreference::Remembered
            || candidate.preference == CandidatePreference::SoleEligible
        {
            candidate.preference = CandidatePreference::Undetermined;
        }
        candidate.warnings.push(LaunchWarning::new(
            LaunchWarningKind::PreferredLaunchChoiceUnavailable,
            detail,
        ));
        recompute(candidate);
    }
}

fn recompute(candidate: &mut LaunchCandidate) {
    candidate.readiness = if !candidate.blockers.is_empty() {
        LaunchReadiness::Blocked
    } else if !candidate.warnings.is_empty() {
        LaunchReadiness::ReadyWithWarnings
    } else {
        LaunchReadiness::Ready
    };
}

pub fn default_launch_preferences_path() -> Result<PathBuf, String> {
    crate::app_dirs::config_path(LAUNCH_PREFERENCES_FILE).map_err(|error| error.to_string())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchPreferencesFile {
    version: u32,
    preferences: Vec<LaunchPreference>,
}

/// A missing file means nothing has been saved yet. Anything else that is not
/// a small, regular, well-formed preferences file is an error, never empty.
pub fn load_launch_preferences_at(path: &Path) -> Result<Vec<LaunchPreference>, String> {
    let file = match super::process_spawn::open_regular_file_nonblocking(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("{} is too large", path.display()));
    }
    let parsed: LaunchPreferencesFile = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is not valid: {error}", path.display()))?;
    if parsed.version != FORMAT_VERSION {
        return Err(format!(
            "{} uses unsupported version {}",
            path.display(),
            parsed.version
        ));
    }
    for (index, preference) in parsed.preferences.iter().enumerate() {
        preference.validate()?;
        if parsed.preferences[..index]
            .iter()
            .any(|earlier| earlier.same_slot(preference))
        {
            return Err(format!(
                "{} saves more than one choice for the same game or system",
                path.display()
            ));
        }
    }
    Ok(parsed.preferences)
}

/// Saves `preference`, replacing only the choice for the same scope, system
/// and game. Refuses to replace a file that cannot be read.
pub fn set_launch_preference_at(path: &Path, preference: &LaunchPreference) -> Result<(), String> {
    preference.validate()?;
    // A launch-only file-hash identity is not a stable game identity and is
    // never persisted as a per-game default (system defaults carry no key).
    if preference.game_key.as_deref().is_some_and(|key| {
        key.starts_with(crate::launch::AZAHAR_HOMEBREW_KEY_PREFIX)
            || key.starts_with(crate::launch::LOCAL_ADF_KEY_PREFIX)
    }) {
        return Err(
            "This file is a launch target only, so a per-game emulator default cannot be saved for it. Use the system default or choose once."
                .into(),
        );
    }
    let mut preferences = load_launch_preferences_at(path)?;
    preferences.retain(|existing| !existing.same_slot(preference));
    preferences.push(preference.clone());
    write_preferences(path, preferences)
}

/// Forgets the saved choice for exactly this scope, system and game.
pub fn clear_launch_preference_at(
    path: &Path,
    scope: LaunchPreferenceScope,
    platform_id: &str,
    game_key: Option<&str>,
) -> Result<(), String> {
    let mut preferences = load_launch_preferences_at(path)?;
    let before = preferences.len();
    preferences.retain(|existing| {
        !(existing.scope == scope
            && existing.platform_id == platform_id
            && existing.game_key.as_deref() == game_key)
    });
    if preferences.len() == before {
        return Ok(());
    }
    write_preferences(path, preferences)
}

fn write_preferences(path: &Path, mut preferences: Vec<LaunchPreference>) -> Result<(), String> {
    preferences.sort_by(|left, right| {
        (
            &left.platform_id,
            left.scope == LaunchPreferenceScope::Game,
            &left.game_key,
        )
            .cmp(&(
                &right.platform_id,
                right.scope == LaunchPreferenceScope::Game,
                &right.game_key,
            ))
    });
    let mut text = serde_json::to_string_pretty(&LaunchPreferencesFile {
        version: FORMAT_VERSION,
        preferences,
    })
    .map_err(|error| error.to_string())?;
    text.push('\n');
    if text.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "{} would exceed its size limit; forget some per-game emulator defaults first",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    crate::atomic_write_text(path, &text).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests;
