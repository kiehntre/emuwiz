//! Native first-run and environment Doctor presentation.
//!
//! The page is a calm projection over EnvironmentSnapshot and the
//! already-loaded library. It does not scan, install, migrate, or repair.

use super::{
    environment::EnvironmentSnapshot,
    imagery::{EmptyArt, Imagery, empty_state},
    library::Library,
    routes::Section,
};
use crate::ui::theme;
use eframe::egui;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Action {
    DismissWelcome,
    DismissAndGoHome,
    Refresh,
    Open(Section),
    OpenPlatform(String),
    OpenGame(i64),
    /// Ask to create something; nothing is written until it is confirmed.
    RequestCreate(SetupConfirm),
    /// Copy the read-only Doctor report as text.
    CopySummary,
}

/// A setup change waiting for the person's confirmation, naming the exact
/// path that will be created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SetupConfirm {
    StarterConfig(std::path::PathBuf),
    MountRoot(std::path::PathBuf),
}

impl SetupConfirm {
    pub(super) fn path(&self) -> &std::path::Path {
        match self {
            Self::StarterConfig(path) | Self::MountRoot(path) => path,
        }
    }
}

/// Setup & Doctor's own action state: one confirmation, one running job,
/// its plain result, and the copied Doctor summary.
#[derive(Debug, Default)]
pub(super) struct SetupActions {
    pub confirm: Option<SetupConfirm>,
    pub job: Option<u64>,
    pub feedback: Option<(bool, String)>,
    pub summary_job: Option<u64>,
    /// The summary waiting to reach the clipboard on the next frame.
    pub summary: Option<String>,
    pub summary_copied: bool,
}

/// What a confirmation says will and will not change.
pub(super) fn confirm_text(confirm: &SetupConfirm) -> (&'static str, String, &'static str) {
    match confirm {
        SetupConfirm::StarterConfig(path) => (
            "Create a starter configuration?",
            format!("EmuWiz will create one new file: {}", path.display()),
            "No game files are read, moved or changed. An existing configuration is never replaced.",
        ),
        SetupConfirm::MountRoot(path) => (
            "Create the mount folder?",
            format!("EmuWiz will create one empty folder: {}", path.display()),
            "Nothing else is created or changed. An existing folder is never touched.",
        ),
    }
}

/// The confirmation dialog: Cancel is focused first. Returns `Some(true)` to
/// go ahead and `Some(false)` to cancel.
pub(super) fn confirm_dialog(context: &egui::Context, confirm: &SetupConfirm) -> Option<bool> {
    let (title, what, unchanged) = confirm_text(confirm);
    let mut answer = None;
    crate::ui::components::centered_window(title)
        .resizable(false)
        .show(context, |ui| {
            ui.set_max_width(560.0);
            ui.label(what);
            ui.label(egui::RichText::new(unchanged).color(theme::muted(ui)));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let cancel = ui.button("Cancel");
                // Cancel is focused first, once per opening: a stray Enter
                // never creates anything, and Tab still reaches Create.
                let opened = egui::Id::new(("v2_setup_confirm_opened", confirm.path()));
                if !ui
                    .ctx()
                    .data(|data| data.get_temp::<bool>(opened).unwrap_or(false))
                {
                    cancel.request_focus();
                    ui.ctx().data_mut(|data| data.insert_temp(opened, true));
                }
                if cancel.clicked() {
                    answer = Some(false);
                }
                if crate::ui::components::action_button(
                    ui,
                    "Create",
                    crate::ui::components::ActionStyle::Primary,
                    true,
                )
                .clicked()
                {
                    answer = Some(true);
                }
            });
        });
    if answer.is_some() {
        let opened = egui::Id::new(("v2_setup_confirm_opened", confirm.path()));
        context.data_mut(|data| data.remove::<bool>(opened));
    }
    answer
}

/// Configuration: create what is missing (each asks first) and copy the
/// read-only Doctor summary.
fn configuration_card(
    ui: &mut egui::Ui,
    snapshot: &EnvironmentSnapshot,
    setup: &SetupActions,
    action: &mut Option<Action>,
) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.heading("Configuration");
        let running = setup.job.is_some();
        if let Some(path) = &snapshot.starter_config_path {
            ui.label(
                "EmuWiz is not configured yet. That is expected on a fresh install, not an error.",
            );
            ui.monospace(path.display().to_string());
            if ui
                .add_enabled(!running, egui::Button::new("Create Starter Config…"))
                .on_hover_text("Shows exactly what will be created and asks first.")
                .clicked()
            {
                *action = Some(Action::RequestCreate(SetupConfirm::StarterConfig(
                    path.clone(),
                )));
            }
        }
        if let Some(path) = &snapshot.creatable_mount_root {
            ui.label("The configured mount folder does not exist yet.");
            ui.monospace(path.display().to_string());
            if ui
                .add_enabled(!running, egui::Button::new("Create Mount Root…"))
                .on_hover_text("Shows exactly what will be created and asks first.")
                .clicked()
            {
                *action = Some(Action::RequestCreate(SetupConfirm::MountRoot(path.clone())));
            }
        }
        if snapshot.starter_config_path.is_none() && snapshot.creatable_mount_root.is_none() {
            ui.label("The configuration file and mount folder are in place.");
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    setup.summary_job.is_none(),
                    egui::Button::new("Copy summary"),
                )
                .on_hover_text("Copies the read-only Doctor report as text. Nothing is changed.")
                .clicked()
            {
                *action = Some(Action::CopySummary);
            }
            if setup.summary_job.is_some() || running {
                ui.spinner();
            }
            if setup.summary_copied {
                ui.label("Copied the Doctor summary.");
            }
        });
        if let Some((succeeded, message)) = &setup.feedback {
            let colour = if *succeeded {
                egui::Color32::from_rgb(90, 205, 150)
            } else {
                egui::Color32::from_rgb(235, 178, 76)
            };
            ui.colored_label(colour, message);
        }
    });
}

fn button(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add(
        egui::Button::new(egui::RichText::new(label).strong())
            .fill(theme::primary_action_fill(ui))
            .min_size(egui::vec2(180.0, 42.0)),
    )
    .clicked()
}

fn status(ui: &mut egui::Ui, label: &str, tone: egui::Color32) {
    ui.colored_label(tone, label);
}

fn ready(ui: &mut egui::Ui, label: &str) {
    status(
        ui,
        &format!("Ready · {label}"),
        egui::Color32::from_rgb(90, 205, 150),
    );
}

fn attention(ui: &mut egui::Ui, label: &str) {
    status(
        ui,
        &format!("Needs attention · {label}"),
        egui::Color32::from_rgb(235, 178, 76),
    );
}

fn optional(ui: &mut egui::Ui, label: &str) {
    status(
        ui,
        &format!("Optional · {label}"),
        egui::Color32::from_rgb(150, 180, 220),
    );
}

pub(super) fn show(
    ui: &mut egui::Ui,
    snapshot: Option<&EnvironmentSnapshot>,
    library: &Library,
    welcome_dismissed: bool,
    selected_platform: Option<&str>,
    imagery: &mut Imagery,
    setup: &SetupActions,
) -> Option<Action> {
    let Some(snapshot) = snapshot else {
        ui.label("Checking this computer. You can keep browsing while this finishes.");
        ui.spinner();
        return None;
    };

    if snapshot.is_fresh() && !welcome_dismissed {
        return welcome(ui);
    }

    let mut action = None;
    egui::ScrollArea::vertical()
        .id_salt("v2_setup_doctor")
        .auto_shrink([false, false])
        .show(ui, |ui| {

            if snapshot.both_roots_conflict {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("Two existing setups were found");
                    ui.label("EmuWiz found both an EmuWiz setup and an older ArchiveFS setup.");
                    ui.label("Nothing will be merged automatically.");
                    ui.label("Run Upgrade Preflight before choosing which setup should remain active.");
                    if button(ui, "Review upgrade and recovery tools") { action = Some(Action::Open(Section::Advanced)); }
                    ui.collapsing("Advanced details", |ui| {
                        if let Some(path) = &snapshot.active_config_root {
                            ui.label(format!("Active configuration folder: {}", path.display()));
                        }
                        if let Some(path) = &snapshot.active_data_root {
                            ui.label(format!("Active data folder: {}", path.display()));
                        }
                    });
                });
            }

            let blue = theme::blue_profile(ui);
            if blue {
                blue_overview(ui, snapshot, library, imagery, &mut action);
            }
            let ready_count = snapshot.setup_ready_count(library);
            let attention_count = snapshot.setup_attention_count(library);
            if !blue {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading("System setup");
                ui.label(format!("{ready_count} ready · {attention_count} need attention"));
                let updates = snapshot
                    .lifecycle
                    .iter()
                    .filter(|projection| matches!(projection.state, archivefs_core::emulator_lifecycle::LifecycleState::InstalledUpdateAvailable))
                    .count();
                let multiple = snapshot
                    .lifecycle
                    .iter()
                    .filter(|projection| matches!(projection.state, archivefs_core::emulator_lifecycle::LifecycleState::MultipleInstallations))
                    .count();
                let broken = snapshot
                    .lifecycle
                    .iter()
                    .filter(|projection| matches!(projection.state, archivefs_core::emulator_lifecycle::LifecycleState::Broken))
                    .count();
                let externally_managed = snapshot
                    .lifecycle
                    .iter()
                    .filter(|projection| matches!(projection.state, archivefs_core::emulator_lifecycle::LifecycleState::ManagedExternally))
                    .count();
                ui.label(format!(
                    "{updates} update(s) available · {multiple} multiple-installation case(s) · {broken} selection(s) need attention · {externally_managed} externally managed"
                ));
                if attention_count == 0 {
                    ready(ui, "EmuWiz can guide you through the next step");
                }
                if button(ui, "Check again") {
                    action = Some(Action::Refresh);
                }
            });
            }

            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading("Saves & States");
                ui.label("Inspect game saves, memory cards, savestates and system storage without changing them.");
                if button(ui, "Open Saves & States") {
                    action = Some(Action::Open(Section::Saves));
                }
            });

            if snapshot.database_upgrade_required {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("Your library needs an upgrade");
                    ui.label("The library database is from an older EmuWiz version and must be upgraded before it can be opened.");
                    ui.label(format!(
                        "Current database schema: {} · required: {}",
                        snapshot.database_schema.map_or_else(|| "unknown".to_string(), |schema| schema.to_string()),
                        snapshot.required_schema
                    ));
                    ui.label("Open the existing database upgrade workflow from Advanced when you are ready.");
                    if button(ui, "Open upgrade tools") {
                        action = Some(Action::Open(Section::Dat));
                    }
                });
            }

            if !blue {
                checklist(ui, snapshot, library, &mut action);
            }

            configuration_card(ui, snapshot, setup, &mut action);

            ui.heading("Emulators");
            ui.label("EmuWiz only checks installed programs here. It never installs or replaces one silently.");
            if snapshot.lifecycle.is_empty() {
                if empty_state(
                    ui,
                    imagery,
                    EmptyArt::Mascot,
                    "No emulator checks yet",
                    "Run setup checking to see which supported emulators are installed and ready to configure.",
                    Some("Check again"),
                ) {
                    action = Some(Action::Refresh);
                }
            } else {
                let (attention, others): (Vec<_>, Vec<_>) = snapshot
                    .lifecycle
                    .iter()
                    .partition(|emulator| lifecycle_needs_attention(emulator));
                let (missing, working): (Vec<_>, Vec<_>) = others.into_iter().partition(|emulator| {
                    matches!(
                        emulator.state,
                        archivefs_core::emulator_lifecycle::LifecycleState::Missing
                    )
                });
                if attention.is_empty() {
                    ui.label("No emulator needs your attention right now.");
                } else {
                    ui.strong(format!("Needs your attention ({})", attention.len()));
                    for emulator in &attention {
                        lifecycle_emulator_card(ui, emulator, &mut action);
                    }
                }
                if !missing.is_empty() {
                    ui.add_space(4.0);
                    ui.strong(format!("Not installed ({})", missing.len()));
                    ui.label("These supported emulators were not found automatically. You only need the ones for systems you play.");
                    for emulator in &missing {
                        ui.push_id(("missing-emulator", &emulator.emulator_id), |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(&emulator.emulator_id);
                                status(
                                    ui,
                                    lifecycle_state_label(emulator.state),
                                    lifecycle_state_tone(emulator.state),
                                );
                            });
                        });
                    }
                    if button(ui, "Open Emulator Setup") {
                        action = Some(Action::Open(Section::Emulators));
                    }
                }
                if !working.is_empty() {
                    ui.add_space(4.0);
                    egui::CollapsingHeader::new(format!("Working emulators ({})", working.len()))
                        .id_salt("v2_setup_working_emulators")
                        .default_open(false)
                        .show(ui, |ui| {
                            for emulator in &working {
                                lifecycle_emulator_card(ui, emulator, &mut action);
                            }
                        });
                }
            }

            if snapshot.source_needs_attention() {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("Game drive unavailable");
                    for path in &snapshot.unavailable_sources {
                        ui.label(format!("{} may not be mounted.", path.display()));
                    }
                    if let Some(error) = &snapshot.source_error {
                        ui.label("EmuWiz could not read the saved game-folder settings.");
                        ui.collapsing("Advanced details", |ui| {
                            ui.label(error);
                        });
                    } else {
                        ui.label("EmuWiz will not rescan or replace this source, and will not treat it as an empty library.");
                    }
                    if button(ui, "Open Games folders") {
                        action = Some(Action::Open(Section::Sources));
                    }
                });
            }

            if let Some(error) = &snapshot.database_error {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("The library needs attention");
                    ui.label("EmuWiz could not safely inspect the library database.");
                    ui.collapsing("Advanced details", |ui| {
                        ui.label(error);
                    });
                    if button(ui, "Open Problems & Repair") {
                        action = Some(Action::Open(Section::Problems));
                    }
                });
            }

            ui.add_space(8.0);
            ui.heading("Environment Doctor");
            ui.label("Why can’t this game or system launch? Select a system for the checks EmuWiz already knows about.");

            for (platform, count) in &library.platforms {
                let selected = selected_platform == Some(platform.as_str());
                ui.horizontal(|ui| {
                    // A small hardware cue only; the label still names the system.
                    imagery.platform_icon(ui, platform, 28.0);
                    if ui
                        .selectable_label(selected, format!("{platform} · {count} game(s)"))
                        .clicked()
                    {
                        action = Some(Action::OpenPlatform(platform.clone()));
                    }
                });
            }
            if library.platforms.is_empty() {
                ui.label("No games are available yet. Add a Games folder to begin.");
            }
            if let Some(platform) = selected_platform
                && library.platforms.contains_key(platform)
            {
                platform_details(ui, snapshot, library, platform, &mut action, imagery);
            }
        });
    action
}

/// Only states that genuinely need the user, not merely incomplete evidence.
fn lifecycle_needs_attention(
    emulator: &archivefs_core::emulator_lifecycle::EmulatorLifecycleProjection,
) -> bool {
    use archivefs_core::emulator_lifecycle::LifecycleState::*;
    emulator.stale_selected
        || matches!(
            emulator.state,
            Broken | MultipleInstallations | InstalledUnsupportedVersion
        )
}

fn lifecycle_emulator_card(
    ui: &mut egui::Ui,
    emulator: &archivefs_core::emulator_lifecycle::EmulatorLifecycleProjection,
    action: &mut Option<Action>,
) {
    ui.push_id(("emulator", &emulator.emulator_id), |ui| {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(&emulator.emulator_id);
                status(
                    ui,
                    lifecycle_state_label(emulator.state),
                    lifecycle_state_tone(emulator.state),
                );
            });
            if emulator.stale_selected {
                ui.label("Your selected installation can no longer be found.");
            }
            if emulator.installations.is_empty() {
                ui.label(lifecycle_empty_explanation(emulator.state));
                if matches!(
                    emulator.state,
                    archivefs_core::emulator_lifecycle::LifecycleState::Missing
                        | archivefs_core::emulator_lifecycle::LifecycleState::Broken
                ) && button(ui, "Open Emulator Setup")
                {
                    *action = Some(Action::Open(Section::Emulators));
                }
                ui.collapsing("Advanced details", |ui| {
                    ui.label("Checked PATH, configured executable paths, known user AppImage locations and bounded Flatpak metadata.");
                    ui.label("No executable or package binding was proven automatically for this emulator.");
                });
            } else {
                for installation in &emulator.installations {
                    ui.push_id(
                        ("installation", format!("{:?}", installation.exact_binding)),
                        |ui| lifecycle_installation_card(ui, installation),
                    );
                }
            }
        });
    });
}

fn lifecycle_empty_explanation(
    state: archivefs_core::emulator_lifecycle::LifecycleState,
) -> &'static str {
    use archivefs_core::emulator_lifecycle::LifecycleState::*;
    match state {
        Missing => "Not found automatically · choose an executable or review setup",
        Broken => "Found evidence, but the selected profile needs attention",
        MultipleInstallations => "Multiple installations need a selection",
        ManagedExternally => "Managed externally · review the detected installation",
        InstalledCurrent
        | InstalledUpdateAvailable
        | InstalledUnknownVersion
        | InstalledUnsupportedVersion => "Installation evidence is incomplete · review setup",
    }
}

fn lifecycle_state_label(
    state: archivefs_core::emulator_lifecycle::LifecycleState,
) -> &'static str {
    use archivefs_core::emulator_lifecycle::LifecycleState::*;
    match state {
        InstalledCurrent => "Installed",
        InstalledUpdateAvailable => "Update available",
        InstalledUnknownVersion => "Installed · version unknown",
        InstalledUnsupportedVersion => "Installed · unsupported version",
        Missing => "Not found automatically",
        Broken => "Needs attention",
        MultipleInstallations => "Multiple installations found",
        ManagedExternally => "Managed by another installer",
    }
}

fn lifecycle_state_tone(
    state: archivefs_core::emulator_lifecycle::LifecycleState,
) -> egui::Color32 {
    use archivefs_core::emulator_lifecycle::LifecycleState::*;
    match state {
        InstalledCurrent => egui::Color32::from_rgb(90, 205, 150),
        InstalledUpdateAvailable | ManagedExternally | MultipleInstallations => {
            egui::Color32::from_rgb(235, 178, 76)
        }
        Missing => egui::Color32::from_rgb(150, 180, 220),
        InstalledUnknownVersion | InstalledUnsupportedVersion | Broken => {
            egui::Color32::from_rgb(235, 120, 100)
        }
    }
}

fn lifecycle_installation_card(
    ui: &mut egui::Ui,
    installation: &archivefs_core::emulator_lifecycle::EmulatorLifecycleInstallation,
) {
    use archivefs_core::emulator_inventory::InstallationType;
    use archivefs_core::emulator_lifecycle::{ExactBinding, UpdateAuthority};
    ui.group(|ui| {
        // Lead with what is known. "Unknown" values are not repeated here;
        // they are listed once under Advanced details.
        let mut summary = Vec::new();
        if installation.installation_type != InstallationType::Unknown {
            summary.push(installation_type_label(installation.installation_type).to_string());
        }
        summary.push(format!(
            "Version {}",
            installation
                .version
                .version
                .as_deref()
                .unwrap_or("not detected")
        ));
        if installation.selected {
            summary.push("Currently selected".into());
        }
        ui.label(summary.join(" · "));
        ui.label(format!(
            "Health: {}",
            local_health_label(installation.local_health)
        ));
        if let Some(readiness) = installation.launch_readiness {
            ui.label(format!("Launch: {}", launch_readiness_label(readiness)));
        }
        if !matches!(
            installation.launch_support,
            archivefs_core::launch::installation_support::LaunchSupport::NotAssessed
        ) {
            ui.label(installation.launch_support.label());
        }
        if installation.update_authority != UpdateAuthority::Unknown
            && let Some(status) = installation.update_status
        {
            ui.label(format!(
                "Updates: {} · {}",
                update_authority_label(installation.update_authority),
                update_status_label(status)
            ));
        }
        ui.collapsing("Advanced details", |ui| {
            ui.label(installation_type_label(installation.installation_type));
            ui.label(format!(
                "Updates: {}",
                update_authority_label(installation.update_authority)
            ));
            ui.label(format!(
                "Update status: {}",
                installation
                    .update_status
                    .map(update_status_label)
                    .unwrap_or("Latest version unknown")
            ));
            match &installation.exact_binding {
                ExactBinding::FlatpakApp { app_id } => {
                    ui.label(format!("Flatpak app ID: {app_id}"))
                }
                ExactBinding::ManagedInstall {
                    manifest_path,
                    executable_path,
                } => {
                    ui.label(format!("Manifest: {}", manifest_path.display()));
                    ui.label(format!("Executable: {}", executable_path.display()))
                }
                ExactBinding::NativeExecutable { path }
                | ExactBinding::PortableExecutable { path }
                | ExactBinding::UnknownExternal { path } => {
                    ui.label(format!("Path: {}", path.display()))
                }
            };
            ui.label(format!(
                "Ownership evidence: {}",
                ownership_label(installation.ownership_category)
            ));
            ui.label(format!("Version source: {:?}", installation.version.source));
            ui.label(format!("Channel: {:?}", installation.channel));
        });
    });
}

fn installation_type_label(
    kind: archivefs_core::emulator_inventory::InstallationType,
) -> &'static str {
    use archivefs_core::emulator_inventory::InstallationType::*;
    match kind {
        Flatpak => "Flatpak",
        SystemPackage => "System package",
        AppImage => "AppImage",
        Portable => "Portable installation",
        Managed => "EmuWiz-managed installation",
        Manual => "Installed manually",
        Unknown => "Installation type unknown",
    }
}

fn ownership_label(
    ownership: archivefs_core::emulator_lifecycle::OwnershipCategory,
) -> &'static str {
    use archivefs_core::emulator_lifecycle::OwnershipCategory::*;
    match ownership {
        OfficialManaged => "Managed by EmuWiz",
        OfficialBrowserHandoff => "Official download handoff",
        FlatpakManaged => "Managed by Flatpak",
        SystemPackageManaged => "Managed by your system",
        PortableUserManaged => "Installed manually",
        Unknown => "Installation source unknown",
        DoNotAutomate => "Manual updates only",
    }
}

fn update_authority_label(
    authority: archivefs_core::emulator_lifecycle::UpdateAuthority,
) -> &'static str {
    use archivefs_core::emulator_lifecycle::UpdateAuthority::*;
    match authority {
        EmuWizManaged => "EmuWiz",
        Flatpak => "Flatpak",
        SystemPackageManager => "your system package manager",
        OfficialBrowser => "official download page",
        UserManaged => "manual updates",
        None => "no automatic updater",
        Unknown => "unknown",
    }
}

fn local_health_label(health: archivefs_core::emulator_lifecycle::LocalHealth) -> &'static str {
    use archivefs_core::emulator_lifecycle::LocalHealth::*;
    match health {
        Healthy => "Healthy",
        MissingExecutable => "Selected executable missing",
        ChangedExecutable => "Executable changed",
        ManifestMismatch => "Managed manifest mismatch",
        PermissionsProblem => "Permissions need attention",
        UnknownVersion => "Version unknown",
        StaleConfiguredPath => "Selected path is stale",
        MultipleCandidates => "Multiple candidates",
        BrokenProfile => "Profile needs attention",
        Unknown => "Unknown",
    }
}

fn launch_readiness_label(
    readiness: archivefs_core::launch::readiness::LaunchReadiness,
) -> &'static str {
    use archivefs_core::launch::readiness::LaunchReadiness::*;
    match readiness {
        Ready => "Ready",
        ReadyWithWarnings => "Ready with warnings",
        Blocked => "Blocked",
    }
}

fn update_status_label(status: archivefs_core::emulator_update::UpdateStatus) -> &'static str {
    use archivefs_core::emulator_update::UpdateStatus::*;
    match status {
        UpToDate => "Up to date",
        UpdateAvailable => "Update available",
        InstalledNewer => "Installed version is newer",
        VersionUnknown => "Version unknown",
        LatestUnknown => "Latest version unknown",
        ChannelMismatch => "Channel mismatch",
        ComparisonUnsupported => "Cannot compare versions",
        Offline => "Offline",
    }
}

fn welcome(ui: &mut egui::Ui) -> Option<Action> {
    let mut action = None;
    egui::ScrollArea::vertical()
        .id_salt("v2_first_run")
        .show(ui, |ui| {
            ui.heading("Welcome to EmuWiz");
            ui.label("EmuWiz helps you understand, preserve, and play a game collection without changing original files unexpectedly.");
            // "›", not "→": egui's default fonts have no arrow glyph and drew a box.
            ui.label("The usual path is: add a source › scan and inspect › identify releases › review problems › prepare an emulator or a playing library.");
            ui.label("Metadata, artwork, cheats, and mods are optional. Preservation keeps what you own; a playing library is a reviewed view arranged for a frontend.");
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading("Get started");
                ui.label("Set up the basics so EmuWiz can find and check your games. You can dismiss this immediately; nothing is locked or hidden.");
                if button(ui, "Get started") {
                    action = Some(Action::DismissWelcome);
                }
            });
            ui.add_space(10.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading("What EmuWiz can do");
                for (title, explanation) in [
                    ("Scan / inspect", "Read configured folders and inspect archives, media, and structure. Browsing does not rename or delete files."),
                    ("Identify", "Compare local evidence with trusted catalogues and show why a release is or is not verified."),
                    ("Repair / organise", "Preview safe repairs or a reviewed organisation plan before anything changes."),
                    ("Metadata / artwork", "Optionally add descriptions, covers, screenshots, and provider context without replacing stronger identity evidence."),
                    ("Emulators", "Check installed emulators, firmware, and launch readiness; EmuWiz does not silently install software."),
                    ("Cheats / mods", "Find optional improvements for a selected game and preview changes separately from identity."),
                ] {
                    ui.collapsing(title, |ui| { ui.label(explanation); });
                }
            });
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading("Suggested first steps");
                ui.label("You can take these in any order, skip optional steps, or use the sidebar directly.");
                if ui.button("1 · Add a library / source").clicked() { action = Some(Action::Open(Section::Sources)); }
                if ui.button("2 · Inspect my collection").clicked() { action = Some(Action::Open(Section::Games)); }
                if ui.button("3 · Review problems").clicked() { action = Some(Action::Open(Section::Problems)); }
                if ui.button("4 · Configure an emulator").clicked() { action = Some(Action::Open(Section::Emulators)); }
                if ui.button("Optional · Metadata and providers").clicked() { action = Some(Action::Open(Section::Artwork)); }
            });
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading("What is this?");
                for (term, explanation) in [
                    ("DAT", "A versioned catalogue of known releases and their identity details; it is evidence, not a copy of your game."),
                    ("BIOS / firmware", "System software an emulator may require before a game can start. EmuWiz reports requirements and evidence; it does not provide copyrighted firmware."),
                    ("Parent / clone", "A MAME preservation relationship: a clone may depend on shared parent data. It is not automatically the same release."),
                    ("CHD", "A compressed-disk container used by some systems. Inspecting it explains its contents; conversion is a separate, reviewed action."),
                    ("Provenance", "Where a fact came from and when it was observed, so local evidence and provider metadata stay distinguishable."),
                    ("Verified vs external evidence", "Verified means EmuWiz found matching local evidence. External means a provider reported a match; it never silently overrides stronger local identity."),
                    ("MAME preservation vs playing library", "Preservation keeps the original set and relationships. A playing library is a reviewed projection that selects or links usable games for a frontend."),
                ] {
                    ui.collapsing(term, |ui| { ui.label(explanation); });
                }
            });
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading("I already know what I’m doing");
                ui.label("Go straight to the normal interface. You can always return to Setup & Doctor.");
                if ui.button("Open normal interface").clicked() {
                    action = Some(Action::DismissAndGoHome);
                }
            });
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading("Recover an existing install");
                ui.label("Check for an older EmuWiz or ArchiveFS setup before changing anything.");
                if ui.button("Check existing setup").clicked() {
                    action = Some(Action::DismissWelcome);
                }
            });
        });
    action
}

/// How one setup step stands. Wording and tone come from the step itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StepState {
    Ready,
    Attention,
    Optional,
    NotStarted,
}

/// One of the five setup steps, as plain data, so the Classic grid and the
/// Blue cards say exactly the same thing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SetupStep {
    pub title: &'static str,
    pub state: StepState,
    pub label: &'static str,
    pub detail: String,
    /// The existing page that moves this step on, when it needs one.
    pub action: Option<(&'static str, Section)>,
}

pub(super) fn setup_steps(snapshot: &EnvironmentSnapshot, library: &Library) -> Vec<SetupStep> {
    let emulators = snapshot.installed_emulator_count();
    vec![
        SetupStep {
            title: "1. Games folder",
            state: if snapshot.source_count == 0 {
                StepState::NotStarted
            } else if snapshot.source_needs_attention() {
                StepState::Attention
            } else {
                StepState::Ready
            },
            label: if snapshot.source_count == 0 {
                "Not started"
            } else if snapshot.source_needs_attention() {
                "Game drive unavailable"
            } else {
                "Folder available"
            },
            detail: format!(
                "{} configured · {} available",
                snapshot.source_count, snapshot.available_sources
            ),
            action: (snapshot.source_count == 0)
                .then_some(("Add a Games folder", Section::Sources)),
        },
        SetupStep {
            title: "2. Emulators",
            state: if emulators > 0 {
                StepState::Ready
            } else if library.games.is_empty() {
                StepState::Optional
            } else {
                StepState::Attention
            },
            label: if emulators > 0 {
                "Installed emulators found"
            } else if library.games.is_empty() {
                "Choose when needed"
            } else {
                "No supported emulator found"
            },
            detail: format!("{emulators} installation(s) found"),
            action: (emulators == 0).then_some(("Open Emulator Setup", Section::Emulators)),
        },
        SetupStep {
            title: "3. Game identification data",
            state: if snapshot.identification_data_ready {
                StepState::Ready
            } else {
                StepState::Optional
            },
            label: snapshot.dat_setup.as_ref().map_or("Catalogue status unavailable", |status| status.label()),
            detail: snapshot.dat_setup.as_ref().map_or_else(
                || "The catalogue inventory has not been read successfully. Open identification data to inspect it; no game check was concluded.".into(),
                |status| status.detail(),
            ),
            action: Some(("Open identification data", Section::Dat)),
        },
        SetupStep {
            title: "4. System software",
            state: if !library.games.is_empty() && emulators == 0 {
                StepState::Attention
            } else {
                StepState::Optional
            },
            label: if library.games.is_empty() {
                "Check when you choose a system"
            } else if emulators == 0 {
                "Choose an emulator first"
            } else {
                "Checked when you prepare to play"
            },
            detail: "Some systems need original system software before games can start.".into(),
            action: None,
        },
        SetupStep {
            title: "5. Ready to play",
            state: if !library.games.is_empty() && emulators > 0 {
                StepState::Ready
            } else {
                StepState::Optional
            },
            label: if !library.games.is_empty() && emulators > 0 {
                "Open a game to check it"
            } else {
                "Choose a Games folder first"
            },
            detail: "Setup is resumable; it does not need to be completed all at once.".into(),
            action: None,
        },
    ]
}

/// The step the Next-step bar points at: the first that needs attention,
/// else the first with an action (an optional step), else none.
pub(super) fn next_step(steps: &[SetupStep]) -> Option<(usize, &SetupStep)> {
    steps
        .iter()
        .enumerate()
        .find(|(_, step)| step.state == StepState::Attention && step.action.is_some())
        .or_else(|| {
            steps
                .iter()
                .enumerate()
                .find(|(_, step)| step.action.is_some())
        })
}

fn checklist(
    ui: &mut egui::Ui,
    snapshot: &EnvironmentSnapshot,
    library: &Library,
    action: &mut Option<Action>,
) {
    let steps = setup_steps(snapshot, library);
    ui.heading("Setup checklist");
    egui::Grid::new("v2_setup_checklist")
        .num_columns(3)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            for step in &steps {
                ui.strong(step.title);
                match step.state {
                    StepState::Ready => ready(ui, step.label),
                    StepState::Attention => attention(ui, step.label),
                    StepState::Optional => optional(ui, step.label),
                    StepState::NotStarted => {
                        status(ui, step.label, egui::Color32::from_rgb(150, 180, 220))
                    }
                }
                ui.label(&step.detail);
                ui.end_row();
            }
            for (label, section) in steps.iter().filter_map(|step| step.action) {
                if ui.button(label).clicked() {
                    *action = Some(Action::Open(section));
                }
            }
            ui.end_row();
        });
}

/// Blue: the five steps as cards, Check again, the systems found with their
/// real hardware, and the Next-step bar (board `D10-SD-Main`).
fn blue_overview(
    ui: &mut egui::Ui,
    snapshot: &EnvironmentSnapshot,
    library: &Library,
    imagery: &mut Imagery,
    action: &mut Option<Action>,
) {
    use crate::ui::components::{StatusTone, status_badge};
    use crate::ui::theme::blue;
    let steps = setup_steps(snapshot, library);
    let next = next_step(&steps).map(|(index, _)| index);
    let ready_count = snapshot.setup_ready_count(library);
    let attention_count = snapshot.setup_attention_count(library);
    let eyebrow = |ui: &mut egui::Ui, text: String| {
        ui.label(
            egui::RichText::new(text.to_uppercase())
                .size(13.0)
                .strong()
                .color(blue::SECONDARY_TEXT),
        );
    };
    // The Next-step bar leads, so the page's one primary action is on
    // screen without scrolling at every size (the board pins it lower).
    if let Some(index) = next
        && let Some((label, section)) = steps[index].action
    {
        let step = &steps[index];
        egui::Frame::new()
            .fill(blue::CARD_SURFACE_TOP)
            .stroke(egui::Stroke::new(1.0_f32, blue::BORDER_SUBTLE))
            .corner_radius(14)
            .inner_margin(egui::Margin::symmetric(18, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("NEXT STEP")
                                .size(13.0)
                                .strong()
                                .color(blue::ACCENT),
                        );
                        ui.label(theme::card_title(format!(
                            "{} · {}",
                            step.title, step.label
                        )));
                        ui.label(
                            egui::RichText::new(format!(
                                "Step {} of {}. Setup is resumable.",
                                index + 1,
                                steps.len()
                            ))
                            .size(13.5)
                            .color(blue::SECONDARY_TEXT),
                        );
                    });
                    if button(ui, label) {
                        *action = Some(Action::Open(section));
                    }
                });
            });
        ui.add_space(12.0);
    }
    let width = ui.available_width();
    let wide = width >= 980.0;
    let steps_width = if wide {
        (width * 0.5).max(480.0)
    } else {
        width
    };
    let steps_ui = |ui: &mut egui::Ui, action: &mut Option<Action>| {
        ui.set_width(steps_width);
        eyebrow(
            ui,
            format!("System setup · {ready_count} ready · {attention_count} need attention"),
        );
        for (index, step) in steps.iter().enumerate() {
            let selected = Some(index) == next;
            egui::Frame::new()
                .fill(if selected {
                    blue::SELECTED_FILL
                } else {
                    blue::CARD_SURFACE_TOP
                })
                .stroke(egui::Stroke::new(
                    1.0_f32,
                    if selected {
                        blue::SELECTED_OUTLINE
                    } else {
                        blue::BORDER_SUBTLE
                    },
                ))
                .corner_radius(12)
                .inner_margin(egui::Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.label(theme::card_title(step.title));
                        let tone = match step.state {
                            StepState::Ready => StatusTone::Success,
                            StepState::Attention => StatusTone::Warning,
                            StepState::Optional | StepState::NotStarted => StatusTone::Pending,
                        };
                        status_badge(ui, step.label, tone);
                    });
                    ui.label(
                        egui::RichText::new(&step.detail)
                            .size(13.5)
                            .color(blue::SECONDARY_TEXT),
                    );
                });
            ui.add_space(6.0);
        }
        ui.label(
            egui::RichText::new(
                "Setup is resumable. It does not need to be completed all at once.",
            )
            .size(13.5)
            .color(blue::SECONDARY_TEXT),
        );
        ui.add_space(4.0);
        if button(ui, "Check again") {
            *action = Some(Action::Refresh);
        }
    };
    if wide {
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(steps_width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| steps_ui(ui, action),
            );
            ui.add_space(16.0);
            // A sized column, so the systems wrap instead of running off.
            let rest = ui.available_width();
            ui.allocate_ui_with_layout(
                egui::vec2(rest, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(rest);
                    eyebrow(ui, "Systems found".into());
                    detected_systems(ui, library, imagery, action);
                },
            );
        });
    } else {
        steps_ui(ui, action);
        ui.add_space(8.0);
        eyebrow(ui, "Systems found".into());
        detected_systems(ui, library, imagery, action);
    }
    ui.add_space(12.0);
}

/// The largest systems found, each with its own hardware picture; choosing
/// one opens its Doctor checks below.
fn detected_systems(
    ui: &mut egui::Ui,
    library: &Library,
    imagery: &mut Imagery,
    action: &mut Option<Action>,
) {
    use crate::ui::theme::blue;
    let mut systems: Vec<_> = library.platforms.iter().collect();
    systems.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    if systems.is_empty() {
        ui.label(
            egui::RichText::new("No systems yet. Add a Games folder to begin.")
                .color(blue::SECONDARY_TEXT),
        );
        return;
    }
    // Measured rows: a framed tile cannot wrap by itself.
    const TILE: f32 = 150.0 + 20.0;
    const GAP: f32 = 10.0;
    let columns = (((ui.available_width() + GAP) / (TILE + GAP)).floor() as usize).max(1);
    let systems: Vec<_> = systems.into_iter().take(6).collect();
    for row in systems.chunks(columns) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for (platform, count) in row.iter().copied() {
                ui.push_id(("v2_setup_system", platform), |ui| {
                    egui::Frame::new()
                        .fill(blue::CARD_SURFACE_TOP)
                        .stroke(egui::Stroke::new(1.0_f32, blue::BORDER_SUBTLE))
                        .corner_radius(12)
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(150.0);
                            ui.vertical_centered(|ui| {
                                imagery.platform_icon(ui, platform, 96.0);
                                ui.label(egui::RichText::new(platform).strong());
                                ui.label(
                                    egui::RichText::new(format!("{count} games"))
                                        .size(13.0)
                                        .color(blue::SECONDARY_TEXT),
                                );
                                if ui.small_button("Doctor checks").clicked() {
                                    *action = Some(Action::OpenPlatform(platform.clone()));
                                }
                            });
                        });
                });
            }
        });
        ui.add_space(GAP);
    }
}

fn platform_details(
    ui: &mut egui::Ui,
    snapshot: &EnvironmentSnapshot,
    library: &Library,
    platform: &str,
    action: &mut Option<Action>,
    imagery: &mut Imagery,
) {
    let games = library
        .games
        .iter()
        .filter(|game| game.platform == platform)
        .collect::<Vec<_>>();
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            imagery.platform_icon(ui, platform, 64.0);
            ui.vertical(|ui| {
                ui.heading(platform);
                ui.label(format!("{} game(s) in this system", games.len()));
            });
        });
        if let Some(projection) = lifecycle_for_platform(snapshot, platform) {
            ui.label(format!(
                "Emulator: {} · {}",
                projection
                    .installations
                    .iter()
                    .find(|installation| installation.selected)
                    .and_then(|installation| installation.version.version.as_deref())
                    .unwrap_or("version unknown"),
                lifecycle_state_label(projection.state)
            ));
            if let Some(selected) = projection.installations.iter().find(|item| item.selected) {
                ui.label(format!(
                    "Installation: {} · Updates: {}",
                    installation_type_label(selected.installation_type),
                    update_authority_label(selected.update_authority)
                ));
            }
        }
        if snapshot.installed_emulator_count() == 0 {
            attention(ui, "Choose an emulator");
            ui.label("No supported emulator was found by the existing inventory check.");
            if ui.button("Open Emulator Setup").clicked() {
                *action = Some(Action::Open(Section::Emulators));
            }
        } else {
            ready(ui, "An emulator installation was found");
            ui.label("Open a game to run the existing launch-readiness checks for its emulator and system software.");
            if ui.button("Open Emulator Setup").clicked() {
                *action = Some(Action::Open(Section::Emulators));
            }
        }
        let unresolved = games.iter().filter(|game| !game.identified).count();
        let attention_games = games.iter().filter(|game| game.attention).count();
        if unresolved > 0 {
            attention(ui, &format!("{unresolved} game(s) need identification review"));
            if ui.button("Open game checks").clicked() {
                *action = Some(Action::Open(Section::Check));
            }
        } else if attention_games > 0 {
            attention(ui, &format!("{attention_games} game(s) need attention"));
            if ui.button("Open Problems & Repair").clicked() {
                *action = Some(Action::Open(Section::Problems));
            }
        } else {
            ready(ui, "No library blocker is recorded for this system");
        }
        for game in games.iter().filter(|game| !game.identified || game.attention).take(8) {
            ui.push_id(("readiness-game", game.archive.id), |ui| {
                if ui
                    .button(format!("Why won’t {} start?", game.title))
                    .clicked()
                {
                    *action = Some(Action::OpenGame(game.archive.id));
                }
            });
        }
        ui.collapsing("Advanced details", |ui| {
            ui.label("EmuWiz uses the existing launch planner and identity evidence. This page does not recalculate readiness.");
            ui.label(format!("Platform key: {platform}"));
        });
    });
}

fn lifecycle_for_platform<'a>(
    snapshot: &'a EnvironmentSnapshot,
    platform: &str,
) -> Option<&'a archivefs_core::emulator_lifecycle::EmulatorLifecycleProjection> {
    let emulator = match platform.to_ascii_lowercase().as_str() {
        value if value.contains("playstation 2") || value == "ps2" => "PCSX2",
        value if value.contains("playstation") || value == "ps1" => "DuckStation",
        value if value.contains("dreamcast") => "Flycast",
        value if value.contains("gamecube") || value.contains("wii") => "Dolphin",
        value if value.contains("saturn") => "DuckStation",
        value if value.contains("psp") => "PPSSPP",
        value if value.contains("xbox") => "xemu",
        value if value.contains("arcade") || value.contains("mame") => "MAME",
        _ => return None,
    };
    snapshot
        .lifecycle
        .iter()
        .find(|projection| projection.emulator_id == emulator)
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::emulator_lifecycle::{LifecycleState, OwnershipCategory, UpdateAuthority};

    #[test]
    fn lifecycle_state_wording_is_normal_user_facing() {
        assert_eq!(
            lifecycle_state_label(LifecycleState::InstalledCurrent),
            "Installed"
        );
        assert_eq!(
            lifecycle_state_label(LifecycleState::InstalledUpdateAvailable),
            "Update available"
        );
        assert_eq!(
            lifecycle_state_label(LifecycleState::InstalledUnknownVersion),
            "Installed · version unknown"
        );
        assert_eq!(
            lifecycle_state_label(LifecycleState::MultipleInstallations),
            "Multiple installations found"
        );
        assert_eq!(
            lifecycle_state_label(LifecycleState::Broken),
            "Needs attention"
        );
    }

    #[test]
    fn empty_lifecycle_does_not_call_a_missing_binding_not_installed() {
        assert!(
            lifecycle_empty_explanation(LifecycleState::Missing)
                .contains("Not found automatically")
        );
        assert!(!lifecycle_empty_explanation(LifecycleState::Missing).contains("Not installed"));
        assert!(
            lifecycle_empty_explanation(LifecycleState::Broken).contains("profile needs attention")
        );
    }

    #[test]
    fn ownership_and_update_wording_preserve_authority() {
        assert_eq!(
            ownership_label(OwnershipCategory::FlatpakManaged),
            "Managed by Flatpak"
        );
        assert_eq!(
            ownership_label(OwnershipCategory::SystemPackageManaged),
            "Managed by your system"
        );
        assert_eq!(
            ownership_label(OwnershipCategory::PortableUserManaged),
            "Installed manually"
        );
        assert_eq!(
            update_authority_label(UpdateAuthority::SystemPackageManager),
            "your system package manager"
        );
        assert_eq!(
            update_authority_label(UpdateAuthority::None),
            "no automatic updater"
        );
    }
}

#[cfg(test)]
mod dat_setup_tests {
    use super::*;
    use archivefs_core::dat::catalogue_selection::setup_status::DatSetupStatus;

    #[test]
    fn setup_reports_inventory_status_and_keeps_explicit_data_navigation_available() {
        let library = Library::default();
        let mut snapshot = EnvironmentSnapshot::default();
        assert_eq!(
            setup_steps(&snapshot, &library)[2].label,
            "Catalogue status unavailable"
        );
        for (status, label) in [
            (DatSetupStatus::default(), "No trusted DAT configured"),
            (
                DatSetupStatus {
                    configured: 1,
                    enabled: 1,
                    needs_validation: 1,
                    ..Default::default()
                },
                "Catalogue needs validation",
            ),
            (
                DatSetupStatus {
                    configured: 1,
                    enabled: 1,
                    ready: 1,
                    ..Default::default()
                },
                "Game catalogue available",
            ),
            (
                DatSetupStatus {
                    configured: 1,
                    enabled: 1,
                    ..Default::default()
                },
                "No game catalogue available",
            ),
        ] {
            snapshot.identification_data_count = status.configured;
            snapshot.identification_data_ready = status.ready > 0;
            snapshot.dat_setup = Some(status);
            let step = setup_steps(&snapshot, &library).remove(2);
            assert_eq!(step.label, label);
            assert_eq!(
                step.action,
                Some(("Open identification data", Section::Dat))
            );
            assert!(step.detail.contains("does not verify any game"));
            assert_eq!(
                step.state == StepState::Ready,
                snapshot.identification_data_ready
            );
            assert!(library.games.is_empty());
        }
    }
}
