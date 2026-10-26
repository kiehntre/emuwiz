//! Blue controls for an explicit, one-launch PCSX2 PNACH approval.
//! Core owns parsing, identity, readiness, staging and sandbox lifetime.
use super::*;
use archivefs_core::launch::cheat_runtime::{
    CheatRuntimeRoots, CleanupOutcome, MaterialisationOutcome,
};
use archivefs_core::launch::cheat_runtime_pcsx2::{
    Pcsx2CheatLaunch, Pcsx2CheatPreview, launch_pcsx2_cheats, preview_pcsx2_cheats,
};

const APPROVE: &str = "Approve this temporary profile and PNACH for one launch";
const LAUNCH: &str = "Launch PCSX2 with temporary cheats";

enum Launch {
    Starting(Receiver<Result<Pcsx2CheatLaunch, String>>),
    Running(Box<Pcsx2CheatLaunch>),
    Failed(String),
}

#[derive(Default)]
pub(super) struct State {
    recovery: crate::cheat_workspace_recovery::State,
    choice: Option<(Pcsx2LaunchRequest, PathBuf)>,
    selected: bool,
    approved: bool,
    preview: Option<Pcsx2CheatPreview>,
    checking: Option<Receiver<Result<Pcsx2CheatPreview, String>>>,
    message: Option<String>,
    launch: Option<(Pcsx2LaunchRequest, Launch)>,
}

impl State {
    pub(super) fn poll(&mut self) -> bool {
        let mut changed = false;
        if let Some(checking) = self.checking.take() {
            match checking.try_recv() {
                Ok(Ok(preview)) => {
                    self.preview = Some(preview);
                    self.approved = false;
                    changed = true;
                }
                Ok(Err(message)) => {
                    self.message = Some(message);
                    changed = true;
                }
                Err(TryRecvError::Empty) => self.checking = Some(checking),
                Err(TryRecvError::Disconnected) => {
                    self.message = Some(
                        "The readiness worker stopped; preview again. Nothing was installed."
                            .into(),
                    );
                    changed = true;
                }
            }
        }
        if let Some((request, stage)) = self.launch.take() {
            let stage = match stage {
                Launch::Starting(receiver) => match receiver.try_recv() {
                    Ok(Ok(launch)) => {
                        changed = true;
                        Launch::Running(Box::new(launch))
                    }
                    Ok(Err(message)) => {
                        changed = true;
                        Launch::Failed(message)
                    }
                    Err(TryRecvError::Empty) => Launch::Starting(receiver),
                    Err(TryRecvError::Disconnected) => {
                        changed = true;
                        Launch::Failed("The launch worker stopped. Review temporary-profile recovery before retrying.".into())
                    }
                },
                Launch::Running(mut launch) => {
                    if !launch.session.is_finished() {
                        launch.poll();
                        changed = true;
                    }
                    Launch::Running(launch)
                }
                failed => failed,
            };
            self.launch = Some((request, stage));
        }
        changed
    }

    pub(super) fn is_active(&self) -> bool {
        match &self.launch {
            Some((_, Launch::Starting(_))) => true,
            Some((_, Launch::Running(launch))) => !launch.session.is_finished(),
            _ => false,
        }
    }

    pub(super) fn failure_detail(&self) -> Option<String> {
        match &self.launch {
            Some((_, Launch::Failed(message))) => Some(message.clone()),
            Some((_, Launch::Running(launch))) => launch.observation().detail.or_else(|| {
                launch.session.receipt().process.as_ref().and_then(|p| p.exit.as_ref())
                    .filter(|exit| !exit.success).map(|_| "PCSX2 launch failed or exited unexpectedly; review temporary-profile cleanup.".into())
            }),
            _ => None,
        }
    }

    pub(super) fn plain_start(&mut self) {
        if !self.is_active() {
            self.launch = None;
        }
    }

    fn clear_approval(&mut self) {
        self.preview = None;
        self.approved = false;
        self.checking = None;
    }

    fn choose(&mut self, request: Pcsx2LaunchRequest, path: PathBuf) {
        self.clear_approval();
        self.choice = Some((request, path));
        self.selected = false;
        self.message = None;
    }

    fn reconcile(&mut self, request: &Pcsx2LaunchRequest) {
        // Several installation cards can be rendered in one frame. Only a
        // changed game or changed binding of the same profile invalidates a
        // choice; another installation never inherits it.
        let stale = self.choice.as_ref().is_some_and(|(old, _)| {
            old.selected_content_path != request.selected_content_path
                || (old.profile_id == request.profile_id && old != request)
        });
        if stale {
            self.clear_approval();
            self.choice = None;
            self.selected = false;
            self.message = Some(
                "Game or PCSX2 installation changed. Select and preview the PNACH again.".into(),
            );
        }
    }

    fn can_launch(&self, request: &Pcsx2LaunchRequest) -> bool {
        self.selected
            && self.approved
            && !self.is_active()
            && self.preview.as_ref().is_some_and(|p| p.matches(request))
    }

    fn preview(&mut self, request: &Pcsx2LaunchRequest, firmware: &[FirmwareIdentityRecord]) {
        let Some((selected, path)) = self
            .choice
            .clone()
            .filter(|(selected, _)| selected == request)
        else {
            return;
        };
        if !self.selected || self.is_active() {
            return;
        }
        self.clear_approval();
        self.message = None;
        let firmware = firmware.to_vec();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = Pcsx2ProfileDiscoveryRoots::from_environment()
                .map_err(|e| e.to_string())
                .and_then(|roots| {
                    let id = format!(
                        "pcsx2-{}-{}",
                        std::process::id(),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos()
                    );
                    preview_pcsx2_cheats(
                        &selected,
                        &roots,
                        &firmware,
                        &path,
                        &id,
                        &CheatRuntimeRoots::default(),
                    )
                });
            let _ = sender.send(result);
        });
        self.checking = Some(receiver);
    }

    fn start(&mut self, request: &Pcsx2LaunchRequest, firmware: &[FirmwareIdentityRecord]) {
        if !self.can_launch(request) {
            return;
        }
        let Some(approved) = self.preview.take() else {
            return;
        };
        self.approved = false;
        self.selected = false;
        self.message = None;
        let firmware = firmware.to_vec();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = Pcsx2ProfileDiscoveryRoots::from_environment()
                .map_err(|e| e.to_string())
                .and_then(|roots| {
                    launch_pcsx2_cheats(approved, &roots, &firmware, &CheatRuntimeRoots::default())
                });
            let _ = sender.send(result);
        });
        self.launch = Some((request.clone(), Launch::Starting(receiver)));
    }

    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        request: &Pcsx2LaunchRequest,
        firmware: &[FirmwareIdentityRecord],
    ) {
        self.reconcile(request);
        ui.push_id(("pcsx2-temp-cheats", &request.profile_id), |ui| {
            egui::CollapsingHeader::new("Temporary PCSX2 cheats — explicit approval")
                .show(ui, |ui| {
                    ui.label("Choose a PNACH for this exact game and installation. A preview changes nothing. Existing production cheats and settings are never installed over.");
                    ui.label("The explicit launch below uses only this file's validated patch lines. Ordinary Play remains a normal PCSX2 launch.");
                    ui.small(format!("Installation profile: {}", request.profile_id));
                    if ui.add_enabled(!self.is_active(), egui::Button::new("Choose PNACH file for this installation")).clicked()
                        && let Some(path) = rfd::FileDialog::new().add_filter("PCSX2 PNACH", &["pnach"]).pick_file() {
                        self.choose(request.clone(), path);
                    }
                    if let Some((_, path)) = self.choice.as_ref().filter(|(selected, _)| selected == request) {
                        ui.label(format!("Selected file: {}", path.display()));
                        if ui.checkbox(&mut self.selected, "Select all validated patch lines in this PNACH for one launch").changed() {
                            self.clear_approval();
                        }
                        if ui.add_enabled(self.selected && self.checking.is_none() && !self.is_active(), egui::Button::new("Preview temporary PCSX2 cheats")).clicked() {
                            self.preview(request, firmware);
                        }
                    }
                    if self.checking.is_some() { ui.spinner(); ui.label("Checking exact game, source file, profile and installed deployment…"); }
                    if let Some(preview) = self.preview.as_ref().filter(|p| p.matches(request)) {
                        ui.label(format!("Planning ready: {} · {} patch line(s)", preview.identity(), preview.patch_count()));
                        ui.small(format!("Profile: {}", preview.profile_path().display()));
                        ui.small(format!("PNACH SHA-256: {}", preview.source_hash()));
                        ui.small(format!("Verified deployment: {}", preview.deployment()));
                        for (key, path) in preview.preserved_folders() {
                            if matches!(key.as_str(), "MemoryCards" | "Savestates" | "Bios" | "Patches") { ui.small(format!("Preserve {key}: {}", path.display())); }
                        }
                        ui.label("No cheat was installed or activated by this preview. Launch will revalidate everything and check a private profile before boot. BIOS and console state are copied temporarily; real memory cards and save states keep their existing paths. Cleanup waits for sandbox exit; interrupted launches are available in recovery below.");
                        ui.checkbox(&mut self.approved, APPROVE);
                    }
                    if ui.add_enabled(self.can_launch(request), egui::Button::new(LAUNCH)).clicked() { self.start(request, firmware); }
                    if let Some(message) = &self.message { ui.colored_label(ui.visuals().warn_fg_color, message); }
                    if let Some((_, stage)) = self.launch.as_ref().filter(|(selected, _)| selected == request) {
                        match stage {
                            Launch::Starting(_) => { ui.spinner(); ui.label("Revalidating approval and preparing the temporary profile. Game not started yet."); }
                            Launch::Failed(message) => { ui.colored_label(ui.visuals().warn_fg_color, message); }
                            Launch::Running(launch) => {
                                let observed = launch.observation();
                                ui.label(observed.label());
                                if let Some(detail) = observed.detail { ui.colored_label(ui.visuals().warn_fg_color, detail); }
                                if matches!(launch.session.receipt().materialisation, MaterialisationOutcome::Materialised { .. }) { ui.small("Temporary files were staged in an EmuWiz-owned profile."); }
                                match &launch.session.receipt().cleanup {
                                    CleanupOutcome::Completed => { ui.label("Temporary profile removed."); }
                                    CleanupOutcome::NotNeeded => { ui.label("No temporary profile remains."); }
                                    CleanupOutcome::Failed { detail } => { ui.colored_label(ui.visuals().warn_fg_color, format!("Cleanup could not finish: {detail}. Review recovery below.")); }
                                    _ => { ui.small("Temporary profile retained while the sandbox is active or its lifetime is unconfirmed."); }
                                }
                                if launch.session.receipt().has_violation() { ui.colored_label(ui.visuals().warn_fg_color, "A protected file changed during this launch. Review preservation results."); }
                                if launch.session.is_finished() {
                                    if let Some(exit) = launch.session.receipt().process.as_ref().and_then(|p| p.exit.as_ref()) {
                                        if exit.success { ui.label("PCSX2 closed normally."); }
                                        else { ui.colored_label(ui.visuals().warn_fg_color, "PCSX2 launch failed or exited unexpectedly."); }
                                    }
                                    ui.label("This one-launch approval is consumed. Select and preview again for another activation.");
                                }
                            }
                        }
                    }
                    crate::cheat_workspace_recovery::show(ui, &mut self.recovery);
                });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Pcsx2LaunchRequest {
        Pcsx2LaunchRequest {
            selected_content_path: "/private/game.iso".into(),
            expected_platform_id: "PS2".into(),
            expected_game_key: "SLUS-00001".into(),
            expected_ps2_serial: "SLUS-00001".into(),
            profile_id: "selected-profile".into(),
            expected_executable: "/usr/bin/flatpak".into(),
            expected_user_directory_mode:
                archivefs_core::patch_manager::Pcsx2UserDirectoryMode::FlatpakSandbox,
            expected_installation:
                archivefs_core::launch::installation::LaunchInstallation::Flatpak {
                    app_id: "net.pcsx2.PCSX2".into(),
                },
        }
    }
    #[test]
    fn choosing_a_file_never_selects_approves_or_starts_cheats() {
        let request = request();
        let mut state = State::default();
        state.choose(request.clone(), "/private/SLUS-00001_12345678.pnach".into());
        assert!(!state.selected && !state.approved && !state.can_launch(&request));
        state.start(&request, &[]);
        assert!(state.launch.is_none());
    }
    #[test]
    fn game_and_installation_drift_clear_selection_and_approval() {
        for change in [0, 1, 2] {
            let mut request = request();
            let mut state = State::default();
            state.choose(request.clone(), "/private/cheats.pnach".into());
            state.selected = true;
            state.approved = true;
            match change {
                0 => request.selected_content_path = "/private/other.iso".into(),
                1 => request.expected_executable = "/other/flatpak".into(),
                _ => request.expected_ps2_serial = "SLUS-00002".into(),
            }
            state.reconcile(&request);
            assert!(state.choice.is_none());
            assert!(!state.selected && !state.approved && !state.can_launch(&request));
        }
    }
    #[test]
    fn another_profile_never_inherits_the_choice() {
        let original = request();
        let mut other = original.clone();
        other.profile_id = "another-profile".into();
        let mut state = State::default();
        state.choose(original.clone(), "/private/cheats.pnach".into());
        state.selected = true;
        state.approved = true;
        state.reconcile(&other);
        assert!(
            state
                .choice
                .as_ref()
                .is_some_and(|(selected, _)| selected == &original)
        );
        assert!(!state.can_launch(&other));
    }
}
