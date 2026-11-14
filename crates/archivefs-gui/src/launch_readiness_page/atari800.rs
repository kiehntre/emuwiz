//! Atari 8-bit `.atr` launch through Core's native Atari800 disposable session.
//!
//! Core owns the setup file, every check and the sandboxed process
//! (`archivefs_core::launch::atari800_setup`). This module only offers the
//! request, shows the setup the user must complete, and reports what the
//! process and its temporary workspace did.

use super::*;
use archivefs_core::launch::LaunchWarningKind;
use archivefs_core::launch::atari800::{
    Atari800Machine, Atari800RomKind, Atari800Video, DISPOSABLE_SESSION_WARNING,
};
use archivefs_core::launch::atari800_setup::{
    Atari800Paths, Atari800Resolution, Atari800RomBinding, Atari800RomState, Atari800Setup,
    LaunchedAtari800, bind_rom, launch_selected_atr, load_setup, os_kind_for, required_rom_size,
    resolve_setup, save_setup,
};
use archivefs_core::launch::process_spawn::CapturedFileIdentity;
use archivefs_core::launch::safe_launch_sandbox::{CleanupOutcome, SandboxManager};
use std::time::Duration;

pub(crate) const ADAPTER_KEY: &str = "atari800";
const PLATFORM_ID: &str = "Atari 8-bit";

/// Shown wherever Atari800 Play is offered.
pub(crate) const LIMITATION: &str = "Local launch identity (this disk file's own SHA-256 and ATR header), not a DAT check. Atari800 is only started; EmuWiz cannot confirm the game runs.";
pub(crate) const FIRMWARE_CAVEAT: &str =
    "Your Atari system ROMs have the right role and size, but EmuWiz cannot verify their contents.";

mod resolver;
#[cfg(not(test))]
use resolver::AsyncResolver;
use resolver::Snapshot;

type Resolved = Result<(Atari800Setup, Atari800Resolution), String>;

/// One answer is reused for this long while a page redraws.
const RESOLUTION_TTL: Duration = Duration::from_secs(2);
/// A check silent for this long is reported as unavailable, not as checking.
#[cfg(not(test))]
const RESOLUTION_STALL: Duration = Duration::from_secs(3);
/// A silent check is retried this often, within the worker cap.
#[cfg(not(test))]
const RESOLUTION_RETRY: Duration = Duration::from_secs(15);
#[cfg(not(test))]
const RESOLUTION_WORKERS: usize = 3;

/// The background check of the user's setup. Reading the ROMs and the
/// program never happens on the drawing thread: see [`resolver`].
#[cfg(not(test))]
static RESOLVER: std::sync::LazyLock<AsyncResolver<Resolved>> = std::sync::LazyLock::new(|| {
    AsyncResolver::new(
        RESOLUTION_TTL,
        RESOLUTION_STALL,
        RESOLUTION_RETRY,
        RESOLUTION_WORKERS,
    )
});

pub(crate) const CHECKING: &str = "Checking your Atari800 setup…";
const UNAVAILABLE: &str = "Your Atari800 setup cannot be checked right now: the folder holding its ROMs or program is not responding. EmuWiz keeps trying and starts nothing until the check completes.";

fn resolve_from(paths: &Atari800Paths) -> Result<(Atari800Setup, Atari800Resolution), String> {
    let setup = load_setup(&paths.setup_file)?;
    #[cfg(not(test))]
    let path_env = std::env::var_os("PATH");
    #[cfg(test)]
    let path_env: Option<std::ffi::OsString> = None;
    let resolution = resolve_setup(&setup, path_env.as_deref());
    Ok((setup, resolution))
}

/// The user's setup as last checked in the background. Never blocks.
#[cfg(not(test))]
pub(crate) fn current() -> Snapshot<Resolved> {
    RESOLVER.snapshot(|| environment_paths().and_then(|paths| resolve_from(&paths)))
}

/// Tests resolve their own scratch setup directly, never a shared one.
#[cfg(test)]
pub(crate) fn current() -> Snapshot<Resolved> {
    Snapshot::Current(environment_paths().and_then(|paths| resolve_from(&paths)))
}

#[cfg(not(test))]
fn environment_paths() -> Result<Atari800Paths, String> {
    Atari800Paths::from_environment()
}

/// Tests never read the developer's own setup, ROMs or PATH: they see an
/// empty setup unless a test installs its own scratch paths.
#[cfg(test)]
fn environment_paths() -> Result<Atari800Paths, String> {
    Ok(TEST_PATHS
        .with(|paths| paths.borrow().clone())
        .unwrap_or(Atari800Paths {
            setup_file: "/nonexistent-emuwiz-test/atari800_setup.json".into(),
            seed_directory: "/nonexistent-emuwiz-test/atari800".into(),
        }))
}

#[cfg(test)]
thread_local! {
    static TEST_PATHS: std::cell::RefCell<Option<Atari800Paths>> =
        const { std::cell::RefCell::new(None) };
}

fn invalidate() {
    #[cfg(not(test))]
    RESOLVER.invalidate();
}

/// Whether the last completed check, of any configuration, had a candidate.
fn previously_offered() -> bool {
    #[cfg(not(test))]
    {
        matches!(RESOLVER.previous(), Some(Ok((_, resolution))) if resolution.standalone_input().is_some())
    }
    #[cfg(test)]
    {
        false
    }
}

/// What background tile checks key their cached verdicts on: the setup's
/// configuration generation and the last completed check. Reading it starts
/// no check and touches no file, so it is safe on every frame.
pub(crate) fn plan_signature() -> (u64, Option<archivefs_core::launch::StandaloneProfileInput>) {
    #[cfg(not(test))]
    {
        (
            RESOLVER.generation(),
            match RESOLVER.previous() {
                Some(Ok((_, resolution))) => resolution.standalone_input(),
                Some(Err(_)) => Some(unknown_profile_input()),
                None => None,
            },
        )
    }
    #[cfg(test)]
    {
        (0, None)
    }
}

/// The planner candidate for a setup that is not known right now: blocked,
/// with a profile id that can never authorise a launch.
fn unknown_profile_input() -> archivefs_core::launch::StandaloneProfileInput {
    archivefs_core::launch::StandaloneProfileInput {
        adapter_id: ADAPTER_KEY,
        profile_id: archivefs_core::launch::atari800_setup::UNSELECTED_PROFILE_ID.into(),
        profile_path: None,
        eligible: false,
        firmware: archivefs_core::launch::FirmwareReadiness::Unknown,
    }
}

/// The planner candidate for Blue's standalone scan: present when an Atari800
/// program exists or a setup was started, eligible only when the whole setup
/// is complete and was checked recently. While a check is running or not
/// answering, an existing candidate is kept but blocked: a setup that is not
/// known to be ready right now is never offered as ready.
pub(crate) fn standalone_profile_input() -> Option<archivefs_core::launch::StandaloneProfileInput> {
    match current() {
        Snapshot::Current(Ok((_, resolution))) => resolution.standalone_input(),
        // An unreadable setup file is the user's to repair, in the panel.
        Snapshot::Current(Err(_)) | Snapshot::Unavailable => Some(unknown_profile_input()),
        Snapshot::Checking => previously_offered().then(unknown_profile_input),
    }
}

#[derive(Clone)]
pub(crate) struct Request {
    path: PathBuf,
    game_key: String,
    profile_id: String,
}

fn is_atari800(candidate: &LaunchCandidate) -> bool {
    matches!(
        candidate.target,
        LaunchTarget::Standalone {
            adapter_id: ADAPTER_KEY,
            ..
        }
    )
}

impl Request {
    /// `setup_reason` supplies the specific setup blocker; it is only asked
    /// when the candidate itself is blocked.
    fn offer(
        plan: &LaunchPlan,
        candidate: &LaunchCandidate,
        setup_reason: impl FnOnce() -> Option<String>,
    ) -> Result<Self, String> {
        let LaunchTarget::Standalone {
            adapter_id: ADAPTER_KEY,
            profile_id,
            ..
        } = &candidate.target
        else {
            return Err("The selected candidate is not native Atari800.".into());
        };
        if plan.platform_id.as_deref() != Some(PLATFORM_ID) {
            return Err("A resolved Atari 8-bit identity is required. Recheck this game.".into());
        }
        let game_key = plan
            .game_key
            .clone()
            .ok_or("A resolved Atari 8-bit identity is required. Recheck this game.")?;
        let path = candidate
            .content
            .resolved_path
            .as_deref()
            .filter(|path| path.is_absolute())
            .ok_or("No direct Atari 8-bit disk file is available.")?;
        if candidate.content.requires_mount
            || candidate.content.container != Some(LaunchContainerKind::PlainFile)
            || !path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("atr"))
        {
            return Err("Atari800 launches only a direct .atr disk image for now. XFD, ATX, cassettes, cartridges, programs and archives are not launched.".into());
        }
        if candidate.readiness == LaunchReadiness::Blocked || !candidate.blockers.is_empty() {
            return Err(setup_reason().unwrap_or_else(|| {
                candidate
                    .blockers
                    .first()
                    .map(|blocker| blocker.detail.clone())
                    .unwrap_or_else(|| "Atari800 is not ready. Complete its setup.".into())
            }));
        }
        // The only caveat Play may carry is the firmware one the user accepted.
        // Anything else (such as a saved emulator choice that is no longer
        // available) holds the launch and says why.
        if let Some(other) = candidate
            .warnings
            .iter()
            .find(|warning| warning.kind != LaunchWarningKind::FirmwarePresentUnverified)
        {
            return Err(other.detail.clone());
        }
        // The game file itself is not touched here: this runs on every
        // redraw, and the disk may be on a sleeping drive. The launch worker
        // opens, identifies and hashes it at click time.
        Ok(Self {
            path: path.to_owned(),
            game_key,
            profile_id: profile_id.clone(),
        })
    }

    pub(crate) fn key(&self) -> (PathBuf, String) {
        (self.path.clone(), ADAPTER_KEY.into())
    }

    pub(crate) fn execute(self) -> Result<Box<LaunchedAtari800>, String> {
        let paths = Atari800Paths::from_environment()?;
        self.execute_with(&paths, std::env::var_os("PATH").as_deref(), || {
            SandboxManager::from_environment()
        })
    }

    fn execute_with(
        self,
        paths: &Atari800Paths,
        path_env: Option<&std::ffi::OsStr>,
        open_manager: impl FnOnce() -> Result<
            (
                SandboxManager,
                archivefs_core::launch::safe_launch_sandbox::CleanupReport,
            ),
            archivefs_core::launch::safe_launch_sandbox::SafeLaunchSandboxError,
        >,
    ) -> Result<Box<LaunchedAtari800>, String> {
        // On the launch worker, never the drawing thread. The file must be a
        // regular file now and must not change again before it is hashed
        // against the library's own SHA-256 for this game.
        let metadata = std::fs::symlink_metadata(&self.path).map_err(|error| {
            format!("Atari800 was not started: the game file cannot be read ({error}).")
        })?;
        if !metadata.is_file() {
            return Err(
                "Atari800 was not started: the game is not a regular, non-symlinked file.".into(),
            );
        }
        launch_selected_atr(
            paths,
            path_env,
            &self.profile_id,
            &self.path,
            &self.game_key,
            &CapturedFileIdentity::capture(&metadata),
            open_manager,
        )
        .map(Box::new)
        .map_err(|refusal| refusal.to_string())
    }
}

fn environment_reason() -> Option<String> {
    match current() {
        Snapshot::Current(Ok((_, resolution))) => resolution.first_blocker(),
        Snapshot::Current(Err(error)) => Some(error),
        Snapshot::Checking => Some(CHECKING.into()),
        Snapshot::Unavailable => Some(UNAVAILABLE.into()),
    }
}

/// Whether `reason` only says the background check has not answered yet.
pub(super) fn is_checking(reason: &str) -> bool {
    reason == CHECKING
}

pub(super) fn offer_for(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
) -> Option<Result<Request, String>> {
    is_atari800(candidate).then(|| Request::offer(plan, candidate, environment_reason))
}

fn machine_label(machine: Atari800Machine) -> &'static str {
    match machine {
        Atari800Machine::Atari400800 => "Atari 400/800",
        Atari800Machine::Atari800Xl => "Atari 800XL",
        Atari800Machine::Atari130Xe => "Atari 130XE",
    }
}

fn video_label(video: Atari800Video) -> &'static str {
    match video {
        Atari800Video::Pal => "PAL",
        Atari800Video::Ntsc => "NTSC",
    }
}

fn rom_role_label(kind: Atari800RomKind) -> &'static str {
    match kind {
        Atari800RomKind::Os400800 => "400/800 OS ROM",
        Atari800RomKind::OsXlXe => "XL/XE OS ROM",
        Atari800RomKind::Basic => "BASIC ROM",
    }
}

/// What the setup panel is editing: the typed-in paths and the last message.
#[derive(Default)]
pub(super) struct Panel {
    executable: String,
    os_400_800: String,
    os_xl_xe: String,
    basic: String,
    message: Option<String>,
    /// The one setup change being applied in the background, if any.
    saving: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
}

impl Panel {
    fn rom_field(&mut self, kind: Atari800RomKind) -> &mut String {
        match kind {
            Atari800RomKind::Os400800 => &mut self.os_400_800,
            Atari800RomKind::OsXlXe => &mut self.os_xl_xe,
            Atari800RomKind::Basic => &mut self.basic,
        }
    }
}

fn rom_slot(setup: &mut Atari800Setup, kind: Atari800RomKind) -> &mut Option<Atari800RomBinding> {
    match kind {
        Atari800RomKind::Os400800 => &mut setup.os_400_800,
        Atari800RomKind::OsXlXe => &mut setup.os_xl_xe,
        Atari800RomKind::Basic => &mut setup.basic,
    }
}

/// One explicit change to the setup, applied and saved at once.
pub(super) enum Edit {
    Executable(Option<PathBuf>),
    Machine(Atari800Machine),
    Video(Atari800Video),
    BindRom(Atari800RomKind, PathBuf),
    ClearRom(Atari800RomKind),
    BasicEnabled(bool),
    AcceptUnverified(bool),
    AcknowledgeDisposable(bool),
}

/// Applies `edit` to the stored setup. A ROM is bound only if it passes the
/// role and size check; a failed edit leaves the stored setup unchanged.
pub(super) fn apply_edit(paths: &Atari800Paths, edit: Edit) -> Result<String, String> {
    let mut setup = load_setup(&paths.setup_file)?;
    let done = match edit {
        Edit::Executable(path) => {
            if let Some(path) = &path
                && (!path.is_absolute()
                    || std::fs::canonicalize(path).ok().as_deref() != Some(path.as_path())
                    || !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file()))
            {
                return Err(format!(
                    "{} is not a real program file (missing, or reached through a symbolic link). Nothing was changed.",
                    path.display()
                ));
            }
            setup.executable = path;
            "Atari800 program selection saved."
        }
        Edit::Machine(machine) => {
            setup.machine = Some(machine);
            "Machine saved."
        }
        Edit::Video(video) => {
            setup.video = Some(video);
            "Video standard saved."
        }
        Edit::BindRom(kind, path) => {
            let binding = bind_rom(&path, kind)?;
            // A different file is a new, unreviewed ROM: consent starts over.
            if rom_slot(&mut setup, kind).as_ref() != Some(&binding) {
                setup.accept_present_unverified_roms = false;
            }
            *rom_slot(&mut setup, kind) = Some(binding);
            "ROM selected. Confirm below that you accept using unverified ROMs."
        }
        Edit::ClearRom(kind) => {
            *rom_slot(&mut setup, kind) = None;
            "ROM selection removed. The file itself was not touched."
        }
        Edit::BasicEnabled(enabled) => {
            setup.basic_enabled = enabled;
            "BASIC choice saved."
        }
        Edit::AcceptUnverified(accepted) => {
            setup.accept_present_unverified_roms = accepted;
            "Choice saved."
        }
        Edit::AcknowledgeDisposable(acknowledged) => {
            setup.disposable_session_acknowledged = acknowledged;
            "Choice saved."
        }
    };
    save_setup(&paths.setup_file, &setup)?;
    invalidate();
    Ok(done.into())
}

fn show_rom_row(
    ui: &mut egui::Ui,
    panel: &mut Panel,
    setup: &Atari800Setup,
    kind: Atari800RomKind,
    state: Atari800RomState,
    edits: &mut Vec<Edit>,
) {
    let binding = match kind {
        Atari800RomKind::Os400800 => setup.os_400_800.as_ref(),
        Atari800RomKind::OsXlXe => setup.os_xl_xe.as_ref(),
        Atari800RomKind::Basic => setup.basic.as_ref(),
    };
    let role = rom_role_label(kind);
    ui.label(egui::RichText::new(format!("{role} ({} bytes)", required_rom_size(kind))).strong());
    match (binding, state) {
        (Some(binding), Atari800RomState::Bound) => {
            ui.small(format!(
                "Selected: {} · SHA-256 {}… · role and size match, contents not verified",
                binding.path.display(),
                &binding.sha256[..binding.sha256.len().min(16)]
            ));
        }
        (Some(binding), Atari800RomState::Missing) => {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!("Missing: {}", binding.path.display()),
            );
        }
        (Some(binding), _) => {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!(
                    "{} is no longer the file you selected.",
                    binding.path.display()
                ),
            );
        }
        (None, _) => {
            ui.small("Not selected. EmuWiz never supplies or downloads firmware.");
        }
    }
    ui.horizontal_wrapped(|ui| {
        let field = panel.rom_field(kind);
        ui.add(
            egui::TextEdit::singleline(field)
                .hint_text("Full path to your ROM file")
                .desired_width(280.0),
        )
        .on_hover_text(format!("Path of the {role} to use"));
        if ui.button(format!("Browse for {role}…")).clicked()
            && let Some(path) = rfd::FileDialog::new().pick_file()
        {
            *field = path.display().to_string();
        }
        let typed = field.trim().to_owned();
        if ui
            .add_enabled(
                !typed.is_empty(),
                egui::Button::new(format!("Use as {role}")),
            )
            .clicked()
        {
            edits.push(Edit::BindRom(kind, PathBuf::from(typed)));
        }
        if binding.is_some() && ui.button(format!("Remove {role} selection")).clicked() {
            edits.push(Edit::ClearRom(kind));
        }
    });
}

/// Applies one change on a worker thread: binding a ROM reads the file,
/// which may sit on a sleeping drive. One change at a time.
fn start_edit(panel: &mut Panel, ui: &egui::Ui, edit: Edit) {
    if panel.saving.is_some() {
        return;
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    panel.saving = Some(receiver);
    panel.message = None;
    // Tests install their scratch paths per thread, so they are read here.
    #[cfg(test)]
    let paths = environment_paths();
    std::thread::spawn(move || {
        #[cfg(not(test))]
        let paths = environment_paths();
        let _ = sender.send(paths.and_then(|paths| apply_edit(&paths, edit)));
    });
    ui.ctx().request_repaint_after(Duration::from_millis(50));
}

/// The whole setup, editable in place. Every control saves immediately and
/// nothing here starts the emulator.
pub(super) fn show_setup(ui: &mut egui::Ui, state: &mut StandaloneLaunchState, open: bool) {
    let active = state.is_active();
    egui::CollapsingHeader::new("Atari800 setup")
        .id_salt("atari800-setup")
        .default_open(open)
        .show(ui, |ui| {
            let panel = &mut state.atari800;
            // A change being saved (it reads the ROM) reports back here.
            if let Some(saving) = &panel.saving {
                match saving.try_recv() {
                    Ok(result) => {
                        panel.message = Some(result.unwrap_or_else(|error| error));
                        panel.saving = None;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        ui.small("Saving… (reading the selected file)");
                        ui.ctx().request_repaint_after(Duration::from_millis(100));
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        panel.message =
                            Some("The change was not saved. Nothing was changed; try again.".into());
                        panel.saving = None;
                    }
                }
            }
            let (setup, resolution) = match current() {
                Snapshot::Current(Ok(value)) => value,
                Snapshot::Current(Err(error)) => {
                    ui.colored_label(ui.visuals().warn_fg_color, error);
                    if let Ok(paths) = environment_paths() {
                        ui.small(format!(
                            "The setup file is {}. EmuWiz will not overwrite a file it cannot read; move it aside to start a new setup.",
                            paths.setup_file.display()
                        ));
                    }
                    return;
                }
                Snapshot::Checking => {
                    ui.small(CHECKING);
                    ui.ctx().request_repaint_after(Duration::from_millis(100));
                    return;
                }
                Snapshot::Unavailable => {
                    ui.colored_label(ui.visuals().warn_fg_color, UNAVAILABLE);
                    ui.ctx().request_repaint_after(Duration::from_secs(1));
                    // The way out when a selected ROM sits on a drive that no
                    // longer answers: drop the selection (the file is never
                    // touched), then select one somewhere that responds.
                    ui.small("If a selected ROM is on a drive that is no longer connected, remove that selection and choose the file again.");
                    let mut edit = None;
                    ui.add_enabled_ui(!active && panel.saving.is_none(), |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for kind in [
                                Atari800RomKind::Os400800,
                                Atari800RomKind::OsXlXe,
                                Atari800RomKind::Basic,
                            ] {
                                if ui
                                    .button(format!("Remove {} selection", rom_role_label(kind)))
                                    .clicked()
                                {
                                    edit = Some(Edit::ClearRom(kind));
                                }
                            }
                            if ui.button("Find Atari800 automatically").clicked() {
                                edit = Some(Edit::Executable(None));
                            }
                        });
                    });
                    if let Some(edit) = edit {
                        start_edit(panel, ui, edit);
                    }
                    if let Some(message) = &panel.message {
                        ui.label(message.as_str());
                    }
                    return;
                }
            };
            // Re-check while the panel is on screen, so a changed or missing
            // ROM shows up without the user doing anything.
            ui.ctx().request_repaint_after(RESOLUTION_TTL);
            let mut edits = Vec::new();
            let editable = !active && panel.saving.is_none();
            ui.add_enabled_ui(editable, |ui| {
                ui.label(egui::RichText::new("Atari800 program").strong());
                match &resolution.executable {
                    Some(path) => {
                        ui.small(format!("Using {}", path.display()));
                    }
                    None => {
                        ui.small("No single Atari800 program is selected.");
                    }
                }
                for found in &resolution.discovered {
                    if setup.executable.as_ref() != Some(found)
                        && ui
                            .button(format!("Always use {}", found.display()))
                            .clicked()
                    {
                        edits.push(Edit::Executable(Some(found.clone())));
                    }
                }
                for refused in &resolution.symlink_refusals {
                    ui.small(match &refused.eligible_target {
                        Some(target) => format!(
                            "{} is a symbolic link and is not used; its target {} can be selected.",
                            refused.link.display(),
                            target.display()
                        ),
                        None => format!(
                            "{} is a symbolic link and is not used.",
                            refused.link.display()
                        ),
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut panel.executable)
                            .hint_text("Full path to the atari800 program")
                            .desired_width(280.0),
                    );
                    let typed = panel.executable.trim().to_owned();
                    if ui
                        .add_enabled(!typed.is_empty(), egui::Button::new("Use this program"))
                        .clicked()
                    {
                        edits.push(Edit::Executable(Some(PathBuf::from(typed))));
                    }
                    if setup.executable.is_some()
                        && ui.button("Find Atari800 automatically").clicked()
                    {
                        edits.push(Edit::Executable(None));
                    }
                });
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("Machine").strong());
                    for machine in [
                        Atari800Machine::Atari400800,
                        Atari800Machine::Atari800Xl,
                        Atari800Machine::Atari130Xe,
                    ] {
                        if ui
                            .selectable_label(setup.machine == Some(machine), machine_label(machine))
                            .clicked()
                        {
                            edits.push(Edit::Machine(machine));
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("Video").strong());
                    for video in [Atari800Video::Pal, Atari800Video::Ntsc] {
                        if ui
                            .selectable_label(setup.video == Some(video), video_label(video))
                            .clicked()
                        {
                            edits.push(Edit::Video(video));
                        }
                    }
                });
                ui.add_space(4.0);
                match setup.machine {
                    Some(machine) => {
                        show_rom_row(
                            ui,
                            panel,
                            &setup,
                            os_kind_for(machine),
                            resolution.os_rom,
                            &mut edits,
                        );
                    }
                    None => {
                        ui.small("Choose a machine to see which OS ROM it needs.");
                    }
                }
                let mut basic = setup.basic_enabled;
                if ui
                    .checkbox(&mut basic, "Start with a BASIC ROM (most games need this off)")
                    .changed()
                {
                    edits.push(Edit::BasicEnabled(basic));
                }
                if setup.basic_enabled {
                    show_rom_row(
                        ui,
                        panel,
                        &setup,
                        Atari800RomKind::Basic,
                        resolution.basic_rom,
                        &mut edits,
                    );
                }
                ui.add_space(4.0);
                let mut accepted = setup.accept_present_unverified_roms;
                if ui
                    .checkbox(
                        &mut accepted,
                        "I accept using these ROMs although EmuWiz cannot verify their contents",
                    )
                    .changed()
                {
                    edits.push(Edit::AcceptUnverified(accepted));
                }
                let mut acknowledged = setup.disposable_session_acknowledged;
                if ui
                    .checkbox(
                        &mut acknowledged,
                        "I understand each session is temporary: disk changes and saves are discarded",
                    )
                    .changed()
                {
                    edits.push(Edit::AcknowledgeDisposable(acknowledged));
                }
                ui.small(DISPOSABLE_SESSION_WARNING);
            });
            if active {
                ui.small("Setup cannot be changed while a game is running.");
            }
            if let Some(edit) = edits.into_iter().next() {
                start_edit(panel, ui, edit);
            }
            if let Some(message) = &panel.message {
                ui.label(message.as_str());
            }
            match resolution.first_blocker() {
                Some(blocker) => {
                    ui.colored_label(ui.visuals().warn_fg_color, format!("Next: {blocker}"));
                }
                None => {
                    ui.small("Setup complete. Your own Atari800 settings are never read or changed.");
                }
            }
        });
}

pub(super) fn show_offer(
    ui: &mut egui::Ui,
    state: &mut StandaloneLaunchState,
    offer: Result<Request, String>,
) {
    ui.add_space(6.0);
    match offer {
        Ok(request) => {
            ui.small(LIMITATION);
            ui.small(FIRMWARE_CAVEAT);
            ui.small(DISPOSABLE_SESSION_WARNING);
            show_setup(ui, state, false);
            super::show_standalone_launch_action(
                ui,
                state,
                StandaloneLaunchRequest::Atari800(request),
            )
        }
        Err(reason) => {
            ui.colored_label(ui.visuals().warn_fg_color, reason);
            show_setup(ui, state, true);
        }
    }
}

fn cleanup_line(outcome: &CleanupOutcome) -> Option<String> {
    match outcome {
        CleanupOutcome::Running => None,
        CleanupOutcome::Removed => Some("The temporary workspace was removed.".into()),
        CleanupOutcome::Retained { path, .. } => Some(format!(
            "Atari800 did not exit cleanly, so its temporary workspace {} is kept for a few minutes for diagnosis and then removed. It holds only copies.",
            path.display()
        )),
        CleanupOutcome::Refused { path, reason } => Some(format!(
            "The temporary workspace {} could not be removed: {reason}. It holds only copies; your game and ROM files were not changed.",
            path.display()
        )),
    }
}

pub(super) fn show_running(ui: &mut egui::Ui, launched: &LaunchedAtari800) {
    ui.small(format!(
        "Atari800 running · PID {} · {} {} · BASIC {}",
        launched.process.process.pid,
        machine_label(launched.machine),
        video_label(launched.video),
        if launched.basic { "on" } else { "off" }
    ));
    ui.small("Temporary session on private copies: disk changes and saves are discarded when it closes. Starting the process does not prove the game ran.");
    show_skipped(ui, launched);
}

fn show_skipped(ui: &mut egui::Ui, launched: &LaunchedAtari800) {
    for (path, reason) in &launched.startup_cleanup_skipped {
        ui.small(format!(
            "Another temporary workspace is still held and was left alone: {} ({reason}). A session that did not exit cleanly is kept for a few minutes.",
            path.display()
        ));
    }
}

pub(super) fn show_exited(ui: &mut egui::Ui, launched: &mut LaunchedAtari800) {
    if let Some(report) = launched.process.process.poll() {
        ui.label(super::exit_line("Atari800", report));
    }
    if let Some(line) = cleanup_line(&launched.process.cleanup_outcome()) {
        ui.small(line);
    }
    show_skipped(ui, launched);
}

/// A failed exit, or a workspace that could not be removed, in words.
pub(super) fn exit_failure(launched: &LaunchedAtari800) -> Option<String> {
    let outcome = launched.process.cleanup_outcome();
    let refused = matches!(outcome, CleanupOutcome::Refused { .. })
        .then(|| cleanup_line(&outcome))
        .flatten();
    let report = launched.process.process.exit_report()?;
    match &report.status {
        Ok(status) if status.success() => refused,
        _ => Some(
            [
                Some(super::exit_line("Atari800", report)),
                cleanup_line(&outcome),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" "),
        ),
    }
}

#[cfg(test)]
#[path = "atari800_tests.rs"]
mod tests;
