//! Read-only view of which DuckStation installation + configuration profile
//! EmuWiz would use, and why.
//!
//! Everything decided here comes from the core
//! [`archivefs_core::emulator_profile_resolver`] (typed outcomes and the
//! remembered AUTO / PREFERRED / FORCED reader). This module only gathers the
//! inputs, runs the resolver off the GUI thread, tags the result with the
//! request it answers, and turns the typed outcome into words. It never writes
//! a preference, creates a folder, repairs a path or launches anything.

use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use archivefs_core::emulator_inventory::InventoryEmulator;
use archivefs_core::emulator_profile_resolver::desktop::default_desktop_directories;
use archivefs_core::emulator_profile_resolver::{
    CandidateAssessment, CandidateIdentity, ConfigurationWarning, DuckStationAdapter,
    EmulatorSelectionMode, ForcedRefusal, PreferenceBypassReason, PreferenceState, ProfileLayout,
    RankFactor, ResolutionOutcome, ResolutionWarning, SelectionModeKind, SelectionReason, Standing,
    UnusableReason, load_selection_from, resolve_emulator_profile,
};
use archivefs_core::patch_manager::DuckStationProfileDiscoveryRoots;

use crate::emulator_setup_overrides::EmulatorPathOverrides;

/// Everything the background resolution needs, captured on the GUI thread so
/// the worker never reads mutable app state.
#[derive(Debug, Clone)]
pub(crate) struct DuckStationResolutionInputs {
    pub(crate) roots: DuckStationProfileDiscoveryRoots,
    pub(crate) desktop_directories: Vec<PathBuf>,
    /// The existing remembered-selection file. `None` when the config
    /// directory is unavailable (treated as AUTO, and said so).
    pub(crate) selection_path: Option<PathBuf>,
}

impl DuckStationResolutionInputs {
    pub(crate) fn from_environment(
        overrides: &EmulatorPathOverrides,
        managed_executables: &[PathBuf],
    ) -> Result<Self, String> {
        let mut roots =
            DuckStationProfileDiscoveryRoots::from_environment().map_err(|e| e.to_string())?;
        // Validated EmuWiz-managed AppImages are caller-confirmed evidence.
        roots
            .explicit_executables
            .extend(managed_executables.iter().cloned());
        // An old override is evidence for the resolver, never an order.
        if let Some(executable) = &overrides.duckstation_executable {
            roots.explicit_executables.push(executable.clone());
        }
        if let Some(folder) = &overrides.duckstation_configuration_folder {
            roots.explicit_configuration_roots.push(folder.clone());
        }
        let desktop_directories = default_desktop_directories(&roots.home, &roots.xdg_data_home);
        Ok(Self {
            roots,
            desktop_directories,
            selection_path:
                archivefs_core::emulator_profile_resolver::persist::default_selection_path().ok(),
        })
    }
}

/// One finished resolution: the policy that was read and what it produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DuckStationResolutionView {
    pub(crate) mode: EmulatorSelectionMode,
    /// Set when the remembered policy could not be read; the outcome is then
    /// the AUTO answer and the page says so instead of pretending otherwise.
    pub(crate) selection_note: Option<String>,
    pub(crate) outcome: ResolutionOutcome,
}

/// Pure and read-only: reads the policy file and the filesystem facts.
pub(crate) fn compute_duckstation_resolution(
    inputs: &DuckStationResolutionInputs,
) -> DuckStationResolutionView {
    let (mode, selection_note) = match &inputs.selection_path {
        None => (
            EmulatorSelectionMode::Auto,
            Some("The settings folder is unavailable, so no remembered choice was read.".into()),
        ),
        Some(path) => match load_selection_from(path, InventoryEmulator::DuckStation) {
            Ok(mode) => (mode, None),
            Err(error) => (
                EmulatorSelectionMode::Auto,
                Some(format!(
                    "The remembered choice could not be read ({error}); showing the automatic answer. Nothing was changed."
                )),
            ),
        },
    };
    let adapter = DuckStationAdapter::new(inputs.roots.clone(), inputs.desktop_directories.clone());
    let outcome = resolve_emulator_profile(&adapter, &mode);
    DuckStationResolutionView {
        mode,
        selection_note,
        outcome,
    }
}

/// Which emulator a resolution view is about. Only wording differs; the
/// outcome types and the presentation rules are shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolverSubject {
    DuckStation,
    Ppsspp,
}

impl ResolverSubject {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::DuckStation => "DuckStation",
            Self::Ppsspp => "PPSSPP",
        }
    }

    fn main_config_file(self) -> &'static str {
        match self {
            Self::DuckStation => "settings.ini",
            Self::Ppsspp => "ppsspp.ini",
        }
    }

    fn details_id(self) -> &'static str {
        match self {
            Self::DuckStation => "duckstation-resolution-candidates",
            Self::Ppsspp => "ppsspp-resolution-candidates",
        }
    }
}

/// A resolution view is emulator-neutral; PPSSPP reuses it unchanged.
pub(crate) type ResolutionView = DuckStationResolutionView;

pub(crate) type ResolutionMessage = (u64, DuckStationResolutionView);

/// Starts the worker. The result is tagged with `generation`.
pub(crate) fn spawn_duckstation_resolution(
    generation: u64,
    inputs: DuckStationResolutionInputs,
    context: Option<egui::Context>,
) -> Receiver<ResolutionMessage> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let view = compute_duckstation_resolution(&inputs);
        let _ = sender.send((generation, view));
        if let Some(context) = context {
            context.request_repaint();
        }
    });
    receiver
}

pub(crate) enum DuckStationResolutionState {
    NotScanned,
    Scanning {
        receiver: Receiver<ResolutionMessage>,
    },
    Ready {
        generation: u64,
        view: Box<DuckStationResolutionView>,
    },
    Error(String),
}

// ---- Launch authority -----------------------------------------------------

/// What the shared resolver authorises Launch to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DuckStationLaunchAuthority {
    /// Exactly this executable + configuration profile (possibly after a
    /// permitted PREFERRED fallback, which `notes` records).
    Authorised {
        executable: PathBuf,
        profile_root: PathBuf,
        layout: ProfileLayout,
        mode: &'static str,
        /// A PREFERRED choice was unavailable and the resolver authorised
        /// this fallback instead.
        fallback: bool,
        /// Why the resolver selected this pair.
        why: Vec<String>,
        /// Every warning the resolver attached (including the bypassed
        /// preference and an unreadable remembered choice).
        notes: Vec<String>,
    },
    /// The resolver refused, found nothing usable, or cannot choose. Launch
    /// must not fall back to any other discovery.
    Refused { reason: String },
}

/// The legacy discovery profiles Launch may use, narrowed to the resolver's
/// authoritative pair, plus the authority that produced them.
#[derive(Debug, Clone)]
pub(crate) struct DuckStationLaunchSelection {
    pub(crate) roots: DuckStationProfileDiscoveryRoots,
    pub(crate) discovery: archivefs_core::patch_manager::DuckStationProfileDiscovery,
    pub(crate) authority: DuckStationLaunchAuthority,
}

pub(crate) fn launch_authority(view: &DuckStationResolutionView) -> DuckStationLaunchAuthority {
    let presentation = present_resolution(view);
    match &view.outcome {
        ResolutionOutcome::Resolved(resolution) => DuckStationLaunchAuthority::Authorised {
            executable: resolution.profile.executable.clone(),
            profile_root: resolution.profile.profile_root.clone(),
            layout: resolution.profile.layout,
            mode: presentation.mode_label,
            fallback: matches!(
                resolution.profile.preference,
                PreferenceState::PreferredBypassed(_)
            ),
            why: presentation.why,
            notes: presentation.warnings,
        },
        _ => {
            let mut reason = format!(
                "{} ({} mode).",
                presentation.headline, presentation.mode_label
            );
            for line in presentation.why.iter().chain(presentation.warnings.iter()) {
                reason.push(' ');
                reason.push_str(line);
            }
            DuckStationLaunchAuthority::Refused { reason }
        }
    }
}

/// Asks the shared resolver and narrows the legacy profiles to its answer.
/// A resolver pair that the legacy launch binding cannot start exactly is a
/// refusal - never a different profile.
pub(crate) fn select_for_launch(
    inputs: &DuckStationResolutionInputs,
) -> DuckStationLaunchSelection {
    select_for_launch_from_view(inputs, &compute_duckstation_resolution(inputs))
}

/// [`select_for_launch`] for a resolution the caller already holds, so a
/// consumer that also needs the resolved profile (Cheats & Mods) acts on the
/// exact same answer Launch would, not a second read of the policy.
pub(crate) fn select_for_launch_from_view(
    inputs: &DuckStationResolutionInputs,
    view: &DuckStationResolutionView,
) -> DuckStationLaunchSelection {
    let mut authority = launch_authority(view);
    let mut roots = inputs.roots.clone();
    if let DuckStationLaunchAuthority::Authorised {
        profile_root,
        layout,
        ..
    } = &authority
    {
        // The resolver has already proven this exact pair. Make the legacy
        // discovery materialise that profile (it may live outside the
        // locations legacy discovery would search on its own) so that the
        // launch binding and the preflight, which re-run discovery with these
        // same roots, describe the same pair.
        let list = if *layout == ProfileLayout::Portable {
            &mut roots.portable_configuration_roots
        } else {
            &mut roots.explicit_configuration_roots
        };
        if !list.contains(profile_root) {
            list.push(profile_root.clone());
        }
    }
    if let DuckStationLaunchAuthority::Authorised { executable, .. } = &authority
        && roots.explicit_executables.contains(executable)
    {
        // Legacy discovery attaches every explicit executable to the default
        // profile and then calls the result ambiguous. The resolver has
        // already chosen between them, so offer only its executable (the
        // preflight re-runs discovery with these same roots) and do not let
        // PATH add another.
        roots.explicit_executables = vec![executable.clone()];
        roots.path_override = Some(Vec::new());
    }
    let mut discovery = archivefs_core::patch_manager::discover_duckstation_profiles(&roots);
    match &authority {
        DuckStationLaunchAuthority::Authorised {
            executable,
            profile_root,
            ..
        } => {
            discovery.profiles.retain(|profile| {
                profile.configuration_path == *profile_root
                    && archivefs_core::patch_manager::resolve_duckstation_native_launch_binding(
                        profile, &roots,
                    )
                    .is_ok_and(|binding| binding.executable == *executable)
            });
            if discovery.profiles.is_empty() {
                authority = DuckStationLaunchAuthority::Refused {
                    reason: format!(
                        "The shared resolver selected {} with settings in {}, but Launch cannot start exactly that pair, so nothing was substituted.",
                        executable.display(),
                        profile_root.display()
                    ),
                };
            }
        }
        DuckStationLaunchAuthority::Refused { .. } => discovery.profiles.clear(),
    }
    DuckStationLaunchSelection {
        roots,
        discovery,
        authority,
    }
}

impl DuckStationLaunchAuthority {
    /// What Launch must say out loud about the resolver's decision. Empty
    /// when the selection is exactly what the user chose or what AUTO found.
    pub(crate) fn launch_notices(&self) -> Vec<String> {
        match self {
            Self::Refused { reason } => vec![format!(
                "DuckStation is blocked by the shared resolver: {reason} Launch will not use any other installation."
            )],
            Self::Authorised {
                executable,
                profile_root,
                fallback,
                why,
                notes,
                ..
            } => {
                let mut out = Vec::new();
                if *fallback {
                    out.push(format!(
                        "Your preferred DuckStation is unavailable, so Launch is using the shared resolver's fallback: {} with settings in {}. {} Your saved preference was not changed.",
                        executable.display(),
                        profile_root.display(),
                        why.join(" ")
                    ));
                }
                out.extend(
                    notes
                        .iter()
                        .filter(|note| {
                            note.contains("preferred") || note.contains("could not be read")
                        })
                        .cloned(),
                );
                out
            }
        }
    }
}

// ---- presentation -------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolutionHealth {
    /// A usable pair was selected and nothing needs attention.
    Healthy,
    /// A usable pair was selected, with notes (e.g. a preferred pair bypassed).
    HealthyWithNotes,
    /// Several equally good pairs; EmuWiz will not guess.
    Ambiguous,
    /// A pinned (FORCED) pair cannot be used and nothing is substituted.
    Unavailable,
    /// Nothing usable was found.
    NothingUsable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProfileResolutionPresentation {
    pub(crate) health: ResolutionHealth,
    pub(crate) headline: String,
    pub(crate) mode_label: &'static str,
    pub(crate) executable: Option<PathBuf>,
    pub(crate) profile_directory: Option<PathBuf>,
    pub(crate) layout: Option<&'static str>,
    pub(crate) why: Vec<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) next_steps: Vec<String>,
    /// One line per candidate the resolver considered.
    pub(crate) candidates: Vec<String>,
    /// Stable id of the collapsible "technical details" block.
    pub(crate) details_id: &'static str,
}

fn mode_label(kind: SelectionModeKind) -> &'static str {
    match kind {
        SelectionModeKind::Auto => "AUTO",
        SelectionModeKind::Preferred => "PREFERRED",
        SelectionModeKind::Forced => "FORCED",
    }
}

pub(crate) fn layout_label(layout: ProfileLayout) -> &'static str {
    match layout {
        ProfileLayout::Portable => "Portable (settings beside the program)",
        ProfileLayout::DefaultUser => "User configuration folder",
        ProfileLayout::FlatpakSandbox => "Flatpak sandbox",
        ProfileLayout::Explicit => "Folder you chose",
    }
}

fn factor_label(factor: RankFactor) -> &'static str {
    match factor {
        RankFactor::SetupState => "its first-run setup state",
        RankFactor::UseHistory => "play history, memory cards or save states",
        RankFactor::GameSpecificConfig => "per-game settings",
        RankFactor::PortableMarker => "a portable marker",
        RankFactor::DesktopLauncher => "a desktop launcher that starts it",
        RankFactor::ValidBiosConfiguration => "a valid BIOS location",
        RankFactor::UserOverride => "a path you set earlier",
    }
}

fn unusable_text(reason: UnusableReason, subject: ResolverSubject) -> String {
    match reason {
        UnusableReason::MainConfigMissing => {
            return format!("{} is missing", subject.main_config_file());
        }
        UnusableReason::MainConfigUnreadable => {
            return format!("{} cannot be read", subject.main_config_file());
        }
        _ => {}
    }
    unusable_fixed_text(reason).to_string()
}

fn unusable_fixed_text(reason: UnusableReason) -> &'static str {
    match reason {
        UnusableReason::ExecutableMissing => "the program file is missing",
        UnusableReason::ExecutableNotARegularFile => "the program is not a plain file",
        UnusableReason::ExecutableNotExecutable => "the program is not marked executable",
        UnusableReason::ExecutableNotAbsolute => "the program path is not absolute",
        UnusableReason::FlatpakNotInstalled => "the Flatpak is not installed",
        UnusableReason::ProfileDirectoryMissing => "the configuration folder is missing",
        UnusableReason::ProfileDirectoryNotADirectory => "the configuration path is not a folder",
        UnusableReason::MainConfigMissing => "settings.ini is missing",
        UnusableReason::MainConfigUnreadable => "settings.ini cannot be read",
        UnusableReason::IncompatiblePairing => {
            "that program would use a different configuration folder"
        }
        UnusableReason::ExecutableIdentityUnconfirmed => {
            "the program's identity is not confirmed; choose it explicitly to use it"
        }
    }
}

fn reasons_text(reasons: &[UnusableReason], subject: ResolverSubject) -> String {
    if reasons.is_empty() {
        return "it is not installed any more".to_string();
    }
    reasons
        .iter()
        .map(|reason| unusable_text(*reason, subject))
        .collect::<Vec<_>>()
        .join("; ")
}

fn pair_text(identity: &CandidateIdentity) -> String {
    match &identity.executable {
        Some(executable) => format!(
            "{} with settings in {}",
            executable.display(),
            identity.profile_root.display()
        ),
        None => format!(
            "settings in {} (no program installed)",
            identity.profile_root.display()
        ),
    }
}

fn reason_text(reason: &SelectionReason) -> String {
    match reason {
        SelectionReason::OnlyViableCandidate => {
            "It is the only installation with a usable configuration.".to_string()
        }
        SelectionReason::StrongestEvidence(factor) => format!(
            "It was the strongest of several usable installations, decided by {}.",
            factor_label(*factor)
        ),
        SelectionReason::PreferredAndUsable => {
            "It is the installation you preferred, and it is usable.".to_string()
        }
        SelectionReason::ForcedByUser => {
            "You pinned exactly this program and configuration.".to_string()
        }
        SelectionReason::EquivalentExecutableOfSameProfile => {
            "Several programs share this configuration; the first by path is shown.".to_string()
        }
    }
}

fn configuration_warning_text(warning: &ConfigurationWarning) -> String {
    match warning {
        ConfigurationWarning::BiosDirectoryMissing {
            configured,
            resolved,
        } => format!(
            "The BIOS folder '{configured}' does not exist (looked in {}). Nothing was created.",
            resolved.display()
        ),
        ConfigurationWarning::SetupWizardIncomplete => {
            "DuckStation has not finished its first-run setup.".to_string()
        }
        ConfigurationWarning::CustomFolderMissing { folder, path } => format!(
            "A custom {folder:?} folder does not exist: {}.",
            path.display()
        ),
        ConfigurationWarning::FolderUnsafe { folder, reason } => {
            format!("A configured {folder:?} folder was refused: {reason}.")
        }
        ConfigurationWarning::SettingsUnreadable { reason } => {
            format!("settings.ini could not be read: {reason}.")
        }
    }
}

fn bypass_text(why: &PreferenceBypassReason, subject: ResolverSubject) -> String {
    match why {
        PreferenceBypassReason::NoLongerPresent => "it is no longer installed".to_string(),
        PreferenceBypassReason::Unusable(reasons) => reasons_text(reasons, subject),
    }
}

fn standing_text(assessment: &CandidateAssessment, subject: ResolverSubject) -> String {
    let what = pair_text(&assessment.identity);
    let layout = layout_label(assessment.layout);
    match &assessment.standing {
        Standing::Selected => format!("In use — {what} [{layout}]"),
        Standing::Alternative { lost_on } => format!(
            "Not used — {what} [{layout}]: outranked on {}",
            factor_label(*lost_on)
        ),
        Standing::NotPreferred { stronger_on } => format!(
            "Not used — {what} [{layout}]: stronger on {}, but your choice decided",
            factor_label(*stronger_on)
        ),
        Standing::Tied => format!("Tied — {what} [{layout}]"),
        Standing::Unusable(reasons) => {
            format!(
                "Unusable — {what} [{layout}]: {}",
                reasons_text(reasons, subject)
            )
        }
    }
}

fn candidate_lines(assessments: &[CandidateAssessment], subject: ResolverSubject) -> Vec<String> {
    assessments
        .iter()
        .map(|assessment| standing_text(assessment, subject))
        .collect()
}

fn nothing_found_steps(subject: ResolverSubject) -> [String; 2] {
    let name = subject.name();
    [
        format!(
            "Install {name} from the Emulators section below, or point EmuWiz at an existing program with the {name} executable override."
        ),
        format!(
            "Start {name} once so it creates its settings, then re-check. EmuWiz will not create the folder for you."
        ),
    ]
}

pub(crate) fn present_resolution(
    view: &DuckStationResolutionView,
) -> ProfileResolutionPresentation {
    present_resolution_for(view, ResolverSubject::DuckStation)
}

pub(crate) fn present_resolution_for(
    view: &DuckStationResolutionView,
    subject: ResolverSubject,
) -> ProfileResolutionPresentation {
    let name = subject.name();
    let mode = view.mode.kind();
    let mut presentation = ProfileResolutionPresentation {
        health: ResolutionHealth::NothingUsable,
        headline: String::new(),
        mode_label: mode_label(mode),
        executable: None,
        profile_directory: None,
        layout: None,
        why: Vec::new(),
        warnings: Vec::new(),
        next_steps: Vec::new(),
        candidates: Vec::new(),
        details_id: subject.details_id(),
    };
    if let Some(note) = &view.selection_note {
        presentation.warnings.push(note.clone());
    }
    match &view.outcome {
        ResolutionOutcome::Resolved(resolution) => {
            let profile = &resolution.profile;
            presentation.executable = Some(profile.executable.clone());
            presentation.profile_directory = Some(profile.profile_root.clone());
            presentation.layout = Some(layout_label(profile.layout));
            presentation.why.push(reason_text(&profile.reason));
            presentation.candidates = candidate_lines(&resolution.assessments, subject);
            let mut notes = false;
            for warning in &profile.warnings {
                notes = true;
                presentation.warnings.push(match warning {
                    ResolutionWarning::Configuration(warning) => {
                        configuration_warning_text(warning)
                    }
                    ResolutionWarning::PreferredProfileBypassed { preferred, why } => format!(
                        "Your preferred choice ({}) was not used because {}. EmuWiz fell back to the installation above; your preference was kept, not erased.",
                        pair_text(preferred),
                        bypass_text(why, subject)
                    ),
                    ResolutionWarning::EquivalentExecutables { others } => format!(
                        "{} other program(s) share this configuration and are equivalent.",
                        others.len()
                    ),
                });
            }
            let bypassed = matches!(profile.preference, PreferenceState::PreferredBypassed(_));
            presentation.health = if bypassed || notes || view.selection_note.is_some() {
                ResolutionHealth::HealthyWithNotes
            } else {
                ResolutionHealth::Healthy
            };
            presentation.headline = if bypassed {
                match subject {
                    // Launch consumes DuckStation's result; PPSSPP is a preview.
                    ResolverSubject::DuckStation => {
                        "Using a fallback — your preferred DuckStation is unavailable".to_string()
                    }
                    ResolverSubject::Ppsspp => {
                        format!("Recommending a fallback — your preferred {name} is unavailable")
                    }
                }
            } else {
                match subject {
                    ResolverSubject::DuckStation => "Selected DuckStation installation".to_string(),
                    ResolverSubject::Ppsspp => format!("Recommended {name} installation"),
                }
            };
            if bypassed {
                presentation.next_steps.push(
                    "Restore the preferred installation, or choose a different preference.".into(),
                );
            }
        }
        ResolutionOutcome::Ambiguous {
            choices,
            assessments,
        } => {
            presentation.health = ResolutionHealth::Ambiguous;
            presentation.headline =
                format!("More than one {name} looks equally good — EmuWiz will not guess");
            presentation.why.push(
                "The installations below tie on every factor EmuWiz weighs, so no single pair is selected."
                    .to_string(),
            );
            presentation.candidates = candidate_lines(
                if choices.is_empty() {
                    assessments
                } else {
                    choices
                },
                subject,
            );
            presentation.next_steps.push(
                "Pin the installation you want (a preferred or forced choice), or remove the one you do not use.".into(),
            );
        }
        ResolutionOutcome::NoInstallation { assessments } => {
            presentation.headline = format!("No {name} installation was found");
            presentation.candidates = candidate_lines(assessments, subject);
            presentation.next_steps.extend(nothing_found_steps(subject));
        }
        ResolutionOutcome::NoUsableProfile { assessments } => {
            presentation.headline =
                format!("{name} was found, but no configuration profile is usable");
            presentation.candidates = candidate_lines(assessments, subject);
            presentation.next_steps.extend(nothing_found_steps(subject));
        }
        ResolutionOutcome::Refused(refusal) => {
            presentation.health = ResolutionHealth::Unavailable;
            let (pinned, reasons) = match refusal {
                ForcedRefusal::ForcedProfileUnavailable { pinned, reasons }
                | ForcedRefusal::ForcedProfileInvalid { pinned, reasons } => (pinned, reasons),
            };
            presentation.headline =
                format!("Your forced {name} choice is unavailable — nothing else was substituted");
            presentation.why.push(format!(
                "You pinned {}, but {}.",
                pair_text(pinned),
                reasons_text(reasons, subject)
            ));
            presentation.next_steps.push(
                "Restore that exact installation and configuration, or clear the forced choice to let EmuWiz choose automatically.".into(),
            );
        }
    }
    presentation
}

/// Renders the presentation. Read-only: there are no preference controls.
pub(crate) fn show_profile_resolution(
    ui: &mut egui::Ui,
    presentation: &ProfileResolutionPresentation,
) {
    use crate::ui::{components as widgets, theme};
    let (badge, tone) = match presentation.health {
        ResolutionHealth::Healthy => ("Healthy", widgets::StatusTone::Success),
        ResolutionHealth::HealthyWithNotes => ("Usable · notes", widgets::StatusTone::Warning),
        ResolutionHealth::Ambiguous => ("Choose one", widgets::StatusTone::Pending),
        ResolutionHealth::Unavailable => ("Unavailable", widgets::StatusTone::Blocked),
        ResolutionHealth::NothingUsable => ("Not usable", widgets::StatusTone::Pending),
    };
    ui.horizontal_wrapped(|ui| {
        widgets::status_badge(ui, badge, tone);
        ui.label(format!("Mode: {}", presentation.mode_label));
    });
    ui.label(&presentation.headline);
    if let (Some(executable), Some(profile)) =
        (&presentation.executable, &presentation.profile_directory)
    {
        ui.label(format!("Program: {}", executable.display()));
        ui.label(format!("Configuration folder: {}", profile.display()));
        if let Some(layout) = presentation.layout {
            ui.label(format!("Layout: {layout}"));
        }
    }
    for line in &presentation.why {
        ui.label(format!("Why: {line}"));
    }
    for warning in &presentation.warnings {
        ui.colored_label(ui.visuals().warn_fg_color, format!("• {warning}"));
    }
    for step in &presentation.next_steps {
        ui.label(format!("Next: {step}"));
    }
    if !presentation.candidates.is_empty() {
        widgets::technical_details(ui, presentation.details_id, |ui| {
            for line in &presentation.candidates {
                ui.label(egui::RichText::new(line).small().color(theme::muted(ui)));
            }
        });
    }
}

pub(crate) mod ppsspp;

#[cfg(test)]
mod tests;
