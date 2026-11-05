//! Blue page: an alternative native launch of RollerCoaster Tycoon 2 through
//! an already-installed OpenRCT2.
//!
//! Reuses the alternative-provider planner for readiness and the shared
//! standalone launch/monitor state for the launch itself. EmuWiz never
//! installs OpenRCT2, never downloads or copies game files, and never writes
//! to the chosen RCT2 folder; the folder is chosen explicitly each session.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use archivefs_core::alternative_launch_provider_planner::AlternativeLaunchReadiness;
use archivefs_core::launch::{
    OpenRct2Install, OpenRct2LaunchRequest, PinnedOpenRct2, Rct2DataReport, check_openrct2,
    discover_openrct2, inspect_rct2_data, openrct2_search_dirs, plan_openrct2,
    probe_openrct2_version,
};
use eframe::egui::{self, RichText};

use super::super::App;
use crate::launch_readiness_page::{
    StandaloneLaunchRequest, StandaloneLaunchState, openrct2::Request,
    show_standalone_launch_action,
};

/// Passive discovery: file metadata only, nothing is executed.
#[derive(Default)]
enum Scan {
    #[default]
    NotStarted,
    Scanning(Receiver<Vec<OpenRct2Install>>),
    Ready(Vec<OpenRct2Install>),
}

type Probe = Arc<dyn Fn(&PinnedOpenRct2) -> Option<String> + Send + Sync>;

/// The explicit, per-candidate version check (this is what runs the file).
enum Check {
    Running(Receiver<Result<OpenRct2Install, String>>),
    Verified(OpenRct2Install),
    Failed(String),
}

#[derive(Default)]
pub(in crate::gui_v2) struct State {
    scan: Scan,
    checks: BTreeMap<PathBuf, Check>,
    data: Option<Rct2DataReport>,
    dirs: Vec<PathBuf>,
}

impl State {
    fn start(&mut self, context: &egui::Context, dirs: Vec<PathBuf>) {
        let (sender, receiver) = mpsc::channel();
        self.scan = Scan::Scanning(receiver);
        self.checks.clear();
        self.dirs = dirs.clone();
        let context = context.clone();
        std::thread::spawn(move || {
            let _ = sender.send(discover_openrct2(&dirs));
            context.request_repaint();
        });
    }

    /// Runs `--version` on exactly this candidate, off the UI thread.
    fn begin_check(&mut self, context: &egui::Context, candidate: OpenRct2Install, probe: Probe) {
        let (sender, receiver) = mpsc::channel();
        self.checks
            .insert(candidate.executable.clone(), Check::Running(receiver));
        let (dirs, context) = (self.dirs.clone(), context.clone());
        std::thread::spawn(move || {
            let result = check_openrct2(&candidate, &dirs, &*probe).map_err(|error| match error {
                archivefs_core::launch::OpenRct2CheckError::Refused(why) => {
                    format!("Not checked: {why}. Refresh the list.")
                }
                archivefs_core::launch::OpenRct2CheckError::NoVersion => {
                    "It was run but did not report a version, so it is not trusted.".to_string()
                }
            });
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    fn poll(&mut self) {
        if let Scan::Scanning(receiver) = &self.scan {
            match receiver.try_recv() {
                Ok(found) => self.scan = Scan::Ready(found),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.scan = Scan::Ready(Vec::new()),
            }
        }
        for check in self.checks.values_mut() {
            if let Check::Running(receiver) = check {
                match receiver.try_recv() {
                    Ok(Ok(install)) => *check = Check::Verified(install),
                    Ok(Err(why)) => *check = Check::Failed(why),
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        *check = Check::Failed("The check stopped unexpectedly.".into())
                    }
                }
            }
        }
    }

    fn choose(&mut self, root: PathBuf) {
        self.data = Some(inspect_rct2_data(&root));
    }
}

/// Plain-language readiness line. Pure, for tests.
pub(in crate::gui_v2) fn readiness_text(readiness: AlternativeLaunchReadiness) -> &'static str {
    use AlternativeLaunchReadiness as R;
    match readiness {
        R::Ready => "Ready: OpenRCT2 and your RollerCoaster Tycoon 2 folder both check out.",
        R::ProviderMissing => "OpenRCT2 was not found.",
        R::IdentityInsufficient => "Choose your RollerCoaster Tycoon 2 folder.",
        R::VersionUnsupported => "This OpenRCT2 version is too old.",
        R::RequirementsUnknown => {
            "Not checked: the OpenRCT2 version is not verified, so it is not trusted."
        }
        R::OriginalAssetsMissing => {
            "The chosen folder is not a usable RollerCoaster Tycoon 2 folder."
        }
        R::Ambiguous | R::UnsupportedGame => "Not ready.",
    }
}

fn show_candidate(
    ui: &mut egui::Ui,
    candidate: &OpenRct2Install,
    check: Option<&Check>,
    data: Option<&Rct2DataReport>,
    standalone: &mut StandaloneLaunchState,
) -> bool {
    let mut check_clicked = false;
    ui.label(RichText::new("OpenRCT2 candidate").strong());
    ui.small(format!("Resolved path: {}", candidate.executable.display()));
    match check {
        None | Some(Check::Failed(_)) => {
            if let Some(Check::Failed(why)) = check {
                ui.label(why);
            }
            ui.label("Not checked. EmuWiz has not run this file.");
            ui.small(
                "Checking the version will run this file with --version. EmuWiz runs exactly the file it \
                 opened and hashed, and will only launch it later if it is byte-for-byte unchanged; \
                 scripts and wrappers are refused. A version is not proof that this is OpenRCT2. Only check a program you trust. EmuWiz limits the time and output, but that does not make running an \
                 untrusted file safe.",
            );
            check_clicked = ui.button("Check OpenRCT2").clicked();
        }
        Some(Check::Running(_)) => {
            ui.label("Checking… (the file is being run with --version)");
        }
        Some(Check::Verified(install)) => {
            ui.small(format!(
                "Version {}",
                install.version.as_deref().unwrap_or("unknown")
            ));
            let projection = plan_openrct2(Some(install), data);
            ui.label(readiness_text(projection.readiness));
            if projection.readiness != AlternativeLaunchReadiness::Ready {
                ui.small(&projection.reason);
            }
            if let (AlternativeLaunchReadiness::Ready, Some(data)) = (projection.readiness, data) {
                show_standalone_launch_action(
                    ui,
                    standalone,
                    StandaloneLaunchRequest::OpenRct2(Request::new(OpenRct2LaunchRequest {
                        install: install.clone(),
                        data: data.clone(),
                    })),
                );
            }
        }
    }
    ui.separator();
    check_clicked
}

pub(in crate::gui_v2) fn show(
    ui: &mut egui::Ui,
    state: &mut State,
    standalone: &mut StandaloneLaunchState,
) {
    if standalone.poll() || standalone.is_active() {
        ui.ctx().request_repaint();
    }
    if matches!(state.scan, Scan::NotStarted) {
        state.start(ui.ctx(), openrct2_search_dirs());
    }
    state.poll();
    ui.label(
        "An alternative way to play RollerCoaster Tycoon 2: the open-source OpenRCT2 engine \
         running your own copy of the game's data. This is a different engine from the original \
         game, not the original game itself. EmuWiz starts an OpenRCT2 you already installed; it \
         never installs OpenRCT2, downloads or copies game files, or changes your folders, and \
         it cannot see whether the game then runs.",
    );
    ui.small(
        "OpenRCT2 is a separate GPL-3.0 project (openrct2.org). RollerCoaster Tycoon is a \
         trademark of its owners; EmuWiz is not affiliated with either.",
    );
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        if ui.button("Choose RollerCoaster Tycoon 2 folder…").clicked()
            && let Some(path) = rfd::FileDialog::new().pick_folder()
        {
            state.choose(path);
        }
        let scanning = matches!(state.scan, Scan::Scanning(_));
        if ui
            .add_enabled(!scanning, egui::Button::new("Refresh list"))
            .clicked()
        {
            if let Some(data) = state.data.take() {
                state.data = Some(inspect_rct2_data(&data.root));
            }
            state.start(ui.ctx(), openrct2_search_dirs());
        }
    });
    match &state.data {
        None => {
            ui.label("No game folder chosen yet. Choose the folder that contains the Data folder.");
        }
        Some(data) => {
            ui.label(format!("Game folder: {}", data.root.display()));
            match &data.blocker {
                None => {
                    ui.small(
                        "Data/g1.dat is present. Your word that this is RollerCoaster Tycoon 2 \
                         is trusted; other data files are not checked.",
                    );
                }
                Some(blocker) => {
                    ui.label(format!("Not usable: {blocker}."));
                }
            }
        }
    }
    ui.separator();
    match &state.scan {
        Scan::NotStarted | Scan::Scanning(_) => {
            ui.label("Looking for OpenRCT2 (nothing is run)…");
        }
        Scan::Ready(installs) if installs.is_empty() => {
            ui.label(readiness_text(AlternativeLaunchReadiness::ProviderMissing));
            ui.small(
                "Install OpenRCT2 yourself (for example from your distribution), then refresh. \
                 EmuWiz does not download or install it.",
            );
        }
        Scan::Ready(installs) => {
            if installs.len() > 1 {
                ui.small(
                    "Several OpenRCT2 files were found. EmuWiz will not pick one for you; check \
                     only the one you want.",
                );
            }
            let mut selected = None;
            for candidate in installs {
                let check = state.checks.get(&candidate.executable);
                if show_candidate(ui, candidate, check, state.data.as_ref(), standalone) {
                    selected = Some(candidate.clone());
                }
            }
            if let Some(candidate) = selected {
                state.begin_check(ui.ctx(), candidate, Arc::new(probe_openrct2_version));
            }
        }
    }
}

impl App {
    pub(in crate::gui_v2) fn openrct2_page(&mut self, ui: &mut egui::Ui) {
        let workflows = self.native_workflows.get_or_insert_with(|| {
            super::super::native_workflows::NativeWorkflows::new(ui.ctx().clone())
        });
        show(
            ui,
            &mut self.tools.openrct2,
            &mut workflows.app.launch_standalone,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_non_ready_state_has_its_own_truthful_text() {
        use AlternativeLaunchReadiness as R;
        assert!(readiness_text(R::Ready).starts_with("Ready"));
        for r in [
            R::ProviderMissing,
            R::IdentityInsufficient,
            R::VersionUnsupported,
            R::RequirementsUnknown,
            R::OriginalAssetsMissing,
        ] {
            assert!(!readiness_text(r).starts_with("Ready"), "{r:?}");
        }
    }

    fn elf_dir(root: &std::path::Path) -> Vec<PathBuf> {
        use std::os::unix::fs::PermissionsExt;
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let source = ["/usr/bin/true", "/bin/true"]
            .into_iter()
            .find(|p| std::path::Path::new(p).exists())
            .unwrap();
        let exe = bin.join("openrct2");
        std::fs::copy(source, &exe).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        vec![bin]
    }

    fn wait_scan(state: &mut State) {
        for _ in 0..500 {
            state.poll();
            if matches!(state.scan, Scan::Ready(_)) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("scan did not finish");
    }

    fn counting() -> (Probe, Arc<std::sync::atomic::AtomicUsize>) {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let probe: Probe = Arc::new(move |_| {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some("0.4.14".to_string())
        });
        (probe, calls)
    }

    #[test]
    fn opening_the_page_scans_passively_and_never_probes_a_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = elf_dir(dir.path());
        let mut state = State::default();
        state.start(&egui::Context::default(), dirs);
        wait_scan(&mut state);
        let Scan::Ready(found) = &state.scan else {
            unreachable!()
        };
        assert_eq!(found.len(), 1);
        assert!(found[0].version.is_none() && found[0].content_sha256.is_none());
        // Rendering the whole page (scan already Ready) checks nothing either.
        let ctx = egui::Context::default();
        let mut standalone = StandaloneLaunchState::default();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, &mut state, &mut standalone));
        });
        assert!(state.checks.is_empty(), "no check starts without a click");
    }

    #[test]
    fn only_the_explicit_check_probes_the_selected_file_off_the_ui_thread() {
        use std::sync::atomic::Ordering;
        let dir = tempfile::tempdir().unwrap();
        let dirs = elf_dir(dir.path());
        let mut state = State::default();
        let ctx = egui::Context::default();
        state.start(&ctx, dirs);
        wait_scan(&mut state);
        let Scan::Ready(found) = &state.scan else {
            unreachable!()
        };
        let candidate = found[0].clone();
        let (probe, calls) = counting();
        state.begin_check(&ctx, candidate.clone(), probe);
        for _ in 0..500 {
            state.poll();
            if matches!(
                state.checks.get(&candidate.executable),
                Some(Check::Verified(_))
            ) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "exactly one probe, after the click"
        );
        let Some(Check::Verified(install)) = state.checks.get(&candidate.executable) else {
            panic!("check did not verify");
        };
        assert_eq!(install.version.as_deref(), Some("0.4.14"));
        assert!(install.content_sha256.is_some());
    }

    #[test]
    fn choosing_a_bad_folder_records_a_blocker_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = State::default();
        state.choose(dir.path().to_path_buf());
        assert!(state.data.as_ref().unwrap().blocker.is_some());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
