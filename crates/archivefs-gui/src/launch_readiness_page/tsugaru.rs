//! Blue's bounded Tsugaru ISO connection. Discovery and review are explicit;
//! Core owns command planning and process spawning.
use super::{
    LaunchPlan, StandaloneLaunchRequest, StandaloneLaunchState, preview_identity,
    show_standalone_launch_action,
};
use archivefs_core::launch::planning::CanonicalIdentityStatus;
use archivefs_core::launch::process_spawn::{
    CapturedFileIdentity, WatchedProcess, open_regular_file_nonblocking,
};
use archivefs_core::launch::tsugaru_execution::{
    TsugaruLaunchRequest, preflight_and_launch_tsugaru, preflight_tsugaru_launch,
};
use archivefs_core::patch_manager::{TsugaruProfileDiscoveryRoots, discover_tsugaru_profiles};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileBinding {
    path: PathBuf,
    identity: CapturedFileIdentity,
}
fn capture(path: &Path) -> Result<FileBinding, String> {
    if !path.is_absolute() || std::fs::canonicalize(path).ok().as_deref() != Some(path) {
        return Err(format!(
            "{} is missing, non-canonical or passes through a symbolic link. Select the real file, then check again.",
            path.display()
        ));
    }
    let file = open_regular_file_nonblocking(path)
        .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    Ok(FileBinding {
        path: path.into(),
        identity: CapturedFileIdentity::capture(&meta),
    })
}

// The upstream loader requires these four split ROMs with these lengths.
// https://github.com/captainys/TOWNSEMU/blob/master/src/towns/memory/physmem.cpp
// Presence/size is not authenticity. Combined FMT_ALL.ROM can override split
// ROMs and is deliberately outside this bounded integration.
fn firmware(directory: &Path) -> Result<Vec<FileBinding>, String> {
    if std::fs::canonicalize(directory).ok().as_deref() != Some(directory) {
        return Err("Tsugaru ROM directory is missing or passes through a symbolic link.".into());
    }
    if std::fs::symlink_metadata(directory.join("FMT_ALL.ROM")).is_ok() {
        return Err("Combined FMT_ALL.ROM is not supported by Blue's split-ROM review. Use a separately configured split-ROM profile; EmuWiz will not change it.".into());
    }
    let mut files = Vec::new();
    for (name, length) in [
        ("FMT_SYS.ROM", 262144),
        ("FMT_DOS.ROM", 524288),
        ("FMT_FNT.ROM", 262144),
        ("FMT_DIC.ROM", 524288),
    ] {
        let binding = capture(&directory.join(name))?;
        if binding.identity.size != length {
            return Err(format!(
                "{name} has an unsupported size; expected {length} bytes. Supply the required firmware yourself, then check again."
            ));
        }
        files.push(binding);
    }
    for name in [
        "FMT_F20.ROM",
        "MYTOWNS.ROM",
        "MAR_EX0.ROM",
        "MAR_EX1.ROM",
        "MAR_EX2.ROM",
        "MAR_EX3.ROM",
    ] {
        let path = directory.join(name);
        if std::fs::symlink_metadata(&path).is_ok() {
            files.push(capture(&path)?);
        }
    }
    Ok(files)
}

#[derive(Clone, Debug)]
pub(super) struct Request {
    core: TsugaruLaunchRequest,
    roots: TsugaruProfileDiscoveryRoots,
    identity: CanonicalIdentityStatus,
    executable: FileBinding,
    firmware: Vec<FileBinding>,
}
impl Request {
    pub(super) fn key(&self) -> (PathBuf, String) {
        (self.core.selected_content_path.clone(), "tsugaru".into())
    }
    fn inspect(
        path: &Path,
        identity: &CanonicalIdentityStatus,
        roots: &TsugaruProfileDiscoveryRoots,
        profile_id: &str,
    ) -> Result<Self, String> {
        if !matches!(identity, CanonicalIdentityStatus::Resolved(i) if i.platform_id == "FM Towns")
        {
            return Err(
                "A verified FM Towns identity is required; the filename is not sufficient.".into(),
            );
        }
        if !path
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("iso"))
        {
            return Err("Blue supports direct ISO only for Tsugaru. CUE/MDS companion binding and D88/HDI/NHD execution are not supported; no settings were changed.".into());
        }
        let content = capture(path)?;
        let profile = discover_tsugaru_profiles(roots).profiles.into_iter().find(|p| p.profile_id == profile_id)
            .ok_or("The configured Tsugaru profile is unavailable. Supply Tsugaru_CUI and check again.")?;
        let directory = profile.rom_directory.clone().ok_or(
            "Set EMUWIZ_TSUGARU_ROM_DIRECTORY to your own firmware directory, then check again.",
        )?;
        let firmware = firmware(&directory)?;
        let executable = capture(&profile.executable.path)?;
        // Derive firmware readiness from the files just inspected, rather than
        // treating an environment flag or a directory's existence as proof.
        let roots = TsugaruProfileDiscoveryRoots {
            explicit_profiles: vec![(
                profile.executable.path.clone(),
                directory.clone(),
                archivefs_core::patch_manager::TsugaruFirmwareState::PresentUnverified,
            )],
            ..Default::default()
        };
        let core = TsugaruLaunchRequest {
            selected_content_path: path.into(),
            expected_platform_id: "FM Towns".into(),
            profile_id: profile.profile_id,
            expected_executable: executable.path.clone(),
            expected_rom_directory: directory,
            expected_content_identity: content.identity,
        };
        preflight_tsugaru_launch(&core, &roots, identity).map_err(|e| e.detail)?;
        Ok(Self {
            core,
            roots,
            identity: identity.clone(),
            executable,
            firmware,
        })
    }
    fn execute_verified(self, current: &CanonicalIdentityStatus) -> Result<WatchedProcess, String> {
        if current != &self.identity {
            return Err("FM Towns identity changed. Check again before launching.".into());
        }
        if capture(&self.executable.path)? != self.executable
            || firmware(&self.core.expected_rom_directory)? != self.firmware
        {
            return Err("Tsugaru executable or firmware changed after review. Check again before launching.".into());
        }
        preflight_and_launch_tsugaru(&self.core, &self.roots, current).map_err(|e| match e {
            archivefs_core::launch::tsugaru_execution::TsugaruLaunchExecutionError::Preflight(
                e,
            ) => format!("Tsugaru was not started: {}. Check again.", e.detail),
            archivefs_core::launch::tsugaru_execution::TsugaruLaunchExecutionError::Spawn(e) => {
                format!("Tsugaru could not start: {e}")
            }
        })
    }
    pub(super) fn execute(self) -> Result<WatchedProcess, String> {
        let report = crate::selected_evidence_page::gather_selected_evidence_fast(
            &self.core.selected_content_path,
            Some("FM Towns"),
        )
        .map_err(|e| format!("FM Towns identity could not be rechecked: {e}"))?;
        let (identity, _) = archivefs_core::launch::canonical_identity_from_game_report(
            &report.game_identity_report,
        );
        self.execute_verified(&identity)
    }
}

type Results = Vec<(String, Result<Request, String>)>;
#[derive(Default)]
pub(super) struct Panel {
    key: Option<(Option<PathBuf>, CanonicalIdentityStatus)>,
    pending: Option<Receiver<Results>>,
    results: Results,
    chosen: Option<String>,
}
pub(super) fn show(ui: &mut egui::Ui, plan: &LaunchPlan, state: &mut StandaloneLaunchState) {
    if plan.platform_id.as_deref() != Some("FM Towns") {
        return;
    }
    let selected = plan
        .candidates
        .iter()
        .find_map(|c| c.content.resolved_path.clone());
    let identity = preview_identity(plan);
    let key = (selected.clone(), identity.clone());
    if state.tsugaru.key.as_ref() != Some(&key) {
        state.tsugaru = Panel {
            key: Some(key),
            ..Default::default()
        };
    }
    let active = state.is_active();
    let mut launch = None;
    ui.group(|ui| {
        ui.strong("Tsugaru · FM Towns CD");
        ui.small("Direct ISO only. Firmware presence and size are checked, not authenticity. Launch uses the existing profile's ROM directory and disables CMOS autosave. The emulator runs with your normal account permissions.");
        if ui.add_enabled(!active && state.tsugaru.pending.is_none(), egui::Button::new("Check configured Tsugaru profiles")).clicked() {
            let (tx, rx) = mpsc::channel();
            state.tsugaru.results.clear();
            state.tsugaru.chosen = None;
            state.tsugaru.pending = Some(rx);
            std::thread::spawn(move || {
                let roots = TsugaruProfileDiscoveryRoots::from_environment();
                let profiles = discover_tsugaru_profiles(&roots).profiles;
                let results = if profiles.is_empty() {
                    vec![("Tsugaru".into(), Err("No Tsugaru_CUI executable was found on PATH. Install/configure it yourself and set EMUWIZ_TSUGARU_ROM_DIRECTORY to your split ROM directory. Nothing was downloaded.".into()))]
                } else {
                    profiles.into_iter().map(|p| {
                        let result = selected.as_deref().ok_or("No selected media candidate is available. Configure the profile and refresh launch readiness.".into())
                            .and_then(|path| Request::inspect(path, &identity, &roots, &p.profile_id));
                        (p.profile_id, result)
                    }).collect()
                };
                let _ = tx.send(results);
            });
        }
        if let Some(rx) = &state.tsugaru.pending {
            match rx.try_recv() {
                Ok(results) => { state.tsugaru.results = results; state.tsugaru.pending = None; }
                Err(mpsc::TryRecvError::Disconnected) => { state.tsugaru.results = vec![("Tsugaru".into(), Err("Profile check ended unexpectedly. Check again.".into()))]; state.tsugaru.pending = None; }
                Err(mpsc::TryRecvError::Empty) => { ui.label("Checking…"); ui.ctx().request_repaint_after(std::time::Duration::from_millis(100)); }
            }
        }
        for (name, result) in &state.tsugaru.results {
            ui.small(name);
            match result {
                Err(reason) => { ui.label(reason); }
                Ok(request) => {
                    ui.small(format!("ISO: {} · ROM directory: {}", request.core.selected_content_path.display(), request.core.expected_rom_directory.display()));
                    if ui.add_enabled(!active, egui::Button::new("Use this configured profile")).clicked() { state.tsugaru.chosen = Some(name.clone()); }
                    if state.tsugaru.chosen.as_ref() == Some(name) { launch = Some(request.clone()); }
                }
            }
        }
    });
    if let Some(request) = launch {
        show_standalone_launch_action(ui, state, StandaloneLaunchRequest::Tsugaru(request));
    }
}

#[cfg(test)]
#[path = "tsugaru_tests.rs"]
mod tests;
