//! PS4 launch presentation/controller over the shared profile resolver and
//! sealed backend plan. One worker, one latest selection, no per-frame hashing.
use crate::{
    emulator_setup_overrides::{self as overrides, OverridableEmulator},
    ui::components as widgets,
};
use archivefs_core::{
    emulator_inventory::InventoryEmulator,
    emulator_profile_resolver::{self as resolver, shadps4::ShadPs4Adapter, *},
    launch::{
        self,
        installation_known::KnownInstallRoots,
        process_spawn::{CapturedFileIdentity, WatchedProcess},
        shadps4::{ShadPs4LaunchPlan, launch_shadps4, plan_shadps4_launch},
        shadps4_input::inspect_shadps4_game,
        shadps4_profile::{ShadPs4Refusal, ShadPs4RefusalKind as Kind},
    },
};
use eframe::egui;
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, TryRecvError},
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    pub path: PathBuf,
    pub identity: launch::CanonicalIdentityStatus,
}
#[derive(Clone)]
pub(crate) struct Binding {
    pub roots: KnownInstallRoots,
    pub configured: Vec<PathBuf>,
    pub path: Option<OsString>,
    pub mode: EmulatorSelectionMode,
    pub selection_file: Option<PathBuf>,
    pub override_file: Option<PathBuf>,
}
impl Binding {
    fn adapter(&self) -> ShadPs4Adapter {
        ShadPs4Adapter {
            roots: self.roots.clone(),
            configured: self.configured.clone(),
            path: self.path.clone(),
            trusted_appimage: self.mode.identity().cloned(),
        }
    }
}
#[derive(Clone)]
pub(crate) struct Request {
    pub plan: ShadPs4LaunchPlan,
    pub binding: Binding,
}
impl Request {
    pub(crate) fn validate(&self) -> Result<(), ShadPs4Refusal> {
        if let Some(path) = &self.binding.selection_file {
            let now = load_selection_from(path, InventoryEmulator::ShadPs4)
                .map_err(|e| changed(e.to_string()))?;
            if now != self.binding.mode {
                return Err(changed("resolver mode changed after preview"));
            }
        }
        if let Some(path) = &self.binding.override_file {
            let now = read_override(path)?;
            if now.into_iter().collect::<Vec<_>>() != self.binding.configured {
                return Err(changed(
                    "executable discovery override changed after preview",
                ));
            }
        }
        let adapter = self.binding.adapter();
        let outcome = resolve_emulator_profile(&adapter, &self.binding.mode);
        let resolution = outcome
            .resolved()
            .ok_or_else(|| changed("resolver no longer has one usable selection"))?;
        let profile = adapter.bind(
            &resolution.profile.identity,
            self.binding.mode.identity() == Some(&resolution.profile.identity),
        )?;
        if &profile != self.plan.profile() {
            return Err(changed(
                "resolver-selected executable/profile changed after preview",
            ));
        }
        launch::shadps4::preflight_shadps4_launch(&self.plan)?;
        Ok(())
    }
    pub(crate) fn execute(self) -> Result<WatchedProcess, ShadPs4Refusal> {
        self.validate()?;
        launch_shadps4(&self.plan)
    }
}
fn changed(detail: impl Into<String>) -> ShadPs4Refusal {
    ShadPs4Refusal {
        kind: Kind::ChangedAfterPreview,
        detail: detail.into(),
    }
}
fn read_override(path: &Path) -> Result<Option<PathBuf>, ShadPs4Refusal> {
    use std::io::Read;
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(changed(e.to_string())),
    };
    let mut text = String::new();
    file.take(16385)
        .read_to_string(&mut text)
        .map_err(|e| changed(e.to_string()))?;
    if text.len() > 16384 {
        return Err(changed("executable override exceeds path bound"));
    }
    Ok((!text.trim().is_empty()).then(|| PathBuf::from(text.trim())))
}

/// A one-line, user-facing account of why the shared resolver produced no
/// single usable shadPS4 installation (the full assessment stays in the
/// card's installation section).
fn resolution_summary(resolution: &ResolutionOutcome) -> String {
    match resolution {
        ResolutionOutcome::Resolved(_) => String::new(),
        ResolutionOutcome::Ambiguous { choices, .. } => format!(
            "{} shadPS4 installations are equally suitable; choose PREFERRED or FORCED for one. ",
            choices.len()
        ),
        ResolutionOutcome::NoInstallation { .. } => {
            "No shadPS4 executable was found; choose the core executable. ".into()
        }
        ResolutionOutcome::NoUsableProfile { .. } => {
            "shadPS4 was found, but no installation and user directory are usable. ".into()
        }
        ResolutionOutcome::Refused(refusal) => {
            format!("The selected shadPS4 installation is unavailable ({refusal:?}). ")
        }
    }
}

pub(crate) struct Inspection {
    pub resolution: ResolutionOutcome,
    pub binding: Binding,
    pub request: Result<Request, ShadPs4Refusal>,
    files: Vec<(PathBuf, CapturedFileIdentity)>,
}
pub(crate) fn gather(selection: &Selection, binding: Binding) -> Inspection {
    let adapter = binding.adapter();
    let resolution = resolve_emulator_profile(&adapter, &binding.mode);
    let request = (|| {
        // Refuse package/mounted inputs independently of installation readiness.
        let game = inspect_shadps4_game(&selection.path)?;
        let Some(resolved) = resolution.resolved() else {
            let candidates = adapter.discover();
            if matches!(
                resolution,
                ResolutionOutcome::NoUsableProfile { .. }
                    | ResolutionOutcome::NoInstallation { .. }
            ) && let [candidate] = candidates.as_slice()
                && let Err(error) = adapter.bind(&candidate.identity, false)
            {
                return Err(error);
            }
            let detail = candidates
                .iter()
                .flat_map(|c| &c.warnings)
                .map(|w| format!("{w:?}"))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(ShadPs4Refusal {
                kind: if candidates.is_empty() {
                    Kind::ExecutableMissing
                } else {
                    Kind::ConfigurationUnavailable
                },
                detail: format!("{}{detail}", resolution_summary(&resolution)),
            });
        };
        let profile = adapter.bind(
            &resolved.profile.identity,
            binding.mode.identity() == Some(&resolved.profile.identity),
        )?;
        let plan = plan_shadps4_launch(&profile, &game, &selection.identity, Default::default())?;
        Ok(Request {
            plan,
            binding: binding.clone(),
        })
    })();
    let files = request
        .as_ref()
        .map(|request| {
            let plan = &request.plan;
            [
                plan.profile().executable.clone(),
                plan.profile().config_path.clone(),
                plan.game().executable().into(),
                plan.game().identity_provenance().into(),
            ]
            .into_iter()
            .filter_map(|path| {
                fs::symlink_metadata(&path)
                    .ok()
                    .map(|m| (path, CapturedFileIdentity::capture(&m)))
            })
            .collect()
        })
        .unwrap_or_default();
    Inspection {
        resolution,
        binding,
        request,
        files,
    }
}
impl Inspection {
    fn metadata_current(&self) -> bool {
        self.files.iter().all(|(path, identity)| {
            fs::symlink_metadata(path)
                .is_ok_and(|m| m.is_file() && CapturedFileIdentity::capture(&m) == *identity)
        })
    }
}

type Reply = (u64, Selection, Result<Inspection, String>);
#[derive(Default)]
pub(crate) struct Controller {
    repaint: Option<egui::Context>,
    selection: Option<Selection>,
    generation: u64,
    worker: Option<Receiver<Reply>>,
    inspection: Option<Arc<Inspection>>,
    error: Option<String>,
    needs_check: bool,
}
#[derive(Clone)]
pub(crate) struct Snapshot {
    pub inspection: Option<Arc<Inspection>>,
    pub error: Option<String>,
    handle: Arc<Mutex<Controller>>,
    selection: Selection,
}
impl Controller {
    pub(crate) fn set_repaint_context(&mut self, context: &egui::Context) {
        self.repaint = Some(context.clone());
    }
    pub(crate) fn snapshot(handle: &Arc<Mutex<Self>>, selection: Selection) -> Snapshot {
        let mut state = handle.lock().unwrap();
        if state.selection.as_ref() != Some(&selection) {
            state.selection = Some(selection.clone());
            state.invalidate();
        }
        if state
            .inspection
            .as_ref()
            .is_some_and(|inspection| !inspection.metadata_current())
        {
            state.invalidate();
        }
        if let Some(receiver) = &state.worker {
            match receiver.try_recv() {
                Ok((generation, key, result)) => {
                    state.worker = None;
                    state.accept(generation, &key, result);
                }
                Err(TryRecvError::Disconnected) => {
                    state.worker = None;
                    state.error = Some("shadPS4 inspection worker stopped".into());
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if state.worker.is_none() && state.needs_check {
            state.needs_check = false;
            let generation = state.generation;
            let key = selection.clone();
            let (sender, receiver) = mpsc::sync_channel(1);
            state.worker = Some(receiver);
            let repaint = state.repaint.clone();
            std::thread::spawn(move || {
                let result = environment_binding().map(|binding| gather(&key, binding));
                let _ = sender.send((generation, key, result));
                if let Some(context) = repaint {
                    context.request_repaint();
                }
            });
        }
        Snapshot {
            inspection: state.inspection.clone(),
            error: state.error.clone(),
            handle: handle.clone(),
            selection,
        }
    }
    pub(crate) fn invalidate(&mut self) {
        self.generation += 1;
        self.inspection = None;
        self.error = None;
        self.needs_check = true;
        if let Some(context) = &self.repaint {
            context.request_repaint();
        }
    }
    fn accept(&mut self, generation: u64, key: &Selection, result: Result<Inspection, String>) {
        if generation != self.generation || self.selection.as_ref() != Some(key) {
            return;
        }
        match result {
            Ok(inspection) => self.inspection = Some(Arc::new(inspection)),
            Err(error) => self.error = Some(error),
        }
    }
}
fn environment_binding() -> Result<Binding, String> {
    let roots = KnownInstallRoots::from_environment().ok_or("HOME unavailable")?;
    let selection_file = resolver::persist::default_selection_path().map_err(|e| e.to_string())?;
    let override_file = archivefs_core::app_dirs::config_path("shadps4_executable_override.txt")
        .map_err(|e| e.to_string())?;
    let configured = read_override(&override_file)
        .map_err(|e| format!("{:?}: {}", e.kind, e.detail))?
        .into_iter()
        .collect();
    let mode = load_selection_from(&selection_file, InventoryEmulator::ShadPs4)
        .map_err(|e| e.to_string())?;
    Ok(Binding {
        roots,
        configured,
        path: std::env::var_os("PATH"),
        mode,
        selection_file: Some(selection_file),
        override_file: Some(override_file),
    })
}
impl Snapshot {
    pub(crate) fn request(&self, candidate: &launch::LaunchCandidate) -> Option<Request> {
        let request = self.inspection.as_ref()?.request.as_ref().ok()?;
        (candidate.target == *request.plan.target()
            && candidate.blockers.is_empty()
            && candidate.readiness == launch::LaunchReadiness::ReadyWithWarnings
            && candidate.content.resolved_path.as_deref() == Some(request.plan.game().executable()))
        .then(|| request.clone())
    }
    pub(crate) fn apply(&self, plan: &mut launch::LaunchPlan, content: &launch::LaunchContentRef) {
        if plan.platform_id.as_deref() != Some("PS4") {
            return;
        }
        plan.candidates.clear(); // PS4 currently has exactly this one adapter.
        let request = self
            .inspection
            .as_ref()
            .and_then(|inspection| inspection.request.as_ref().ok());
        let mut content = content.clone();
        if let Some(request) = request {
            content.kind = Some(launch::LaunchContentKind::Executable);
            content.container = Some(launch::LaunchContainerKind::PlainFile);
            content.resolved_path = Some(request.plan.game().executable().into());
            content.requires_mount = false;
            content.provenance = format!(
                "verified PS4 TITLE_ID {} from {}",
                request.plan.game().title_id(),
                request.plan.game().identity_provenance().display()
            );
        }
        let refusal = self
            .inspection
            .as_ref()
            .and_then(|inspection| inspection.request.as_ref().err());
        let detail = refusal
            .map(|error| format!("{:?}: {}", error.kind, error.detail))
            .or_else(|| self.error.clone())
            .unwrap_or_else(|| {
                "Checking shadPS4 installation/profile and PS4 input in the background…".into()
            });
        plan.candidates.push(launch::LaunchCandidate {
            target: request.map(|r| r.plan.target().clone()).unwrap_or(
                launch::LaunchTarget::Standalone {
                    adapter_id: "shadps4",
                    profile_id: "unresolved shadPS4 profile".into(),
                    profile_path: None,
                },
            ),
            content,
            firmware: request
                .map(|r| r.plan.profile().launch_profile_input().firmware)
                .unwrap_or(launch::FirmwareReadiness::Unknown),
            readiness: if request.is_some() {
                launch::LaunchReadiness::ReadyWithWarnings
            } else {
                launch::LaunchReadiness::Blocked
            },
            blockers: if request.is_some() {
                Vec::new()
            } else {
                vec![launch::LaunchBlocker::new(
                    launch::LaunchBlockerKind::ProfileIneligible,
                    detail,
                )]
            },
            warnings: request
                .map(|r| {
                    r.plan
                        .warnings()
                        .iter()
                        .map(|w| {
                            launch::LaunchWarning::new(
                                launch::LaunchWarningKind::ShadPs4PrerequisitesUnverified,
                                *w,
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            preference: launch::CandidatePreference::SoleEligible,
        });
        plan.summary = launch::LaunchPlanSummary {
            candidates: 1,
            ready: 0,
            ready_with_warnings: usize::from(request.is_some()),
            blocked: usize::from(request.is_none()),
        };
    }
    fn refresh(&self) {
        let mut state = self.handle.lock().unwrap();
        if state.selection.as_ref() == Some(&self.selection) {
            state.invalidate();
        }
    }
    fn choose_mode(&self, mode: EmulatorSelectionMode) -> Result<(), String> {
        let inspection = self.inspection.as_ref().ok_or("inspection is not ready")?;
        let file = inspection
            .binding
            .selection_file
            .as_ref()
            .ok_or("selection persistence unavailable")?;
        remember_selection_to(file, InventoryEmulator::ShadPs4, &mode)
            .map_err(|e| e.to_string())?;
        self.refresh();
        Ok(())
    }
}

/// The existing readiness card style; returns only a sealed typed request.
pub(crate) fn show(
    ui: &mut egui::Ui,
    snapshot: &Snapshot,
    candidate: &launch::LaunchCandidate,
) -> Option<Request> {
    let mut launch = None;
    widgets::card(ui, |ui| {
        ui.strong("shadPS4 · PS4");
        ui.label(if candidate.blockers.is_empty() {
            "Ready with warnings · compatibility unverified"
        } else {
            "Launch unavailable"
        });
        if let Some(inspection) = &snapshot.inspection {
            if let Some(resolution) = inspection.resolution.resolved() {
                ui.label(format!(
                    "Resolver: {:?} · {:?}",
                    resolution.profile.mode, resolution.profile.reason
                ));
                ui.label(format!(
                    "Executable: {}",
                    resolution.profile.executable.display()
                ));
                ui.label(format!(
                    "Profile/user directory: {}",
                    resolution.profile.profile_root.display()
                ));
                ui.label(format!(
                    "Configuration: {}",
                    resolution.profile.main_config.display()
                ));
                for warning in &resolution.profile.warnings {
                    ui.label(format!("{warning:?}"));
                }
            }
            if let Ok(request) = &inspection.request {
                ui.label(format!(
                    "Verified TITLE_ID: {}",
                    request.plan.game().title_id()
                ));
                if let Some(id) = request.plan.game().content_id() {
                    ui.label(format!("CONTENT_ID: {id}"));
                }
                ui.label(format!(
                    "Game root: {}",
                    request.plan.game().root().display()
                ));
                ui.label(format!(
                    "eboot.bin: {}",
                    request.plan.game().executable().display()
                ));
                ui.collapsing("Typed launch command", |ui| {
                    let spec = request.plan.command_preview();
                    ui.monospace(spec.executable.display().to_string());
                    for (index, argument) in spec.arguments.iter().enumerate() {
                        ui.label(format!("argv[{index}]: {}", argument.to_string_lossy()));
                    }
                    ui.label(format!(
                        "Working directory: {}",
                        request.plan.profile().working_directory.display()
                    ));
                    ui.label("Version unknown; no executable probe was run.");
                });
            }
            ui.collapsing("Installation/profile selection", |ui| {
                ui.label("AUTO/PREFERRED/FORCED use the shared resolver. Selecting an AppImage explicitly trusts its core CLI identity; its publisher is not authenticated.");
                if ui.button("AUTO").clicked() && let Err(e) = snapshot.choose_mode(EmulatorSelectionMode::Auto) { ui.label(e); }
                let assessments = match &inspection.resolution {
                    ResolutionOutcome::Resolved(result) => &result.assessments,
                    ResolutionOutcome::Ambiguous { assessments, .. } | ResolutionOutcome::NoInstallation { assessments } | ResolutionOutcome::NoUsableProfile { assessments } => assessments,
                    ResolutionOutcome::Refused(_) => { ui.label(format!("{:?}", inspection.resolution)); return; }
                };
                for assessment in assessments {
                    ui.label(assessment.identity.to_string());
                    ui.label(format!("{:?}", assessment.standing));
                    for evidence in &assessment.evidence { ui.small(&evidence.detail); }
                    for (label, mode) in [("PREFERRED", EmulatorSelectionMode::Preferred(assessment.identity.clone())), ("FORCED", EmulatorSelectionMode::Forced(assessment.identity.clone()))] {
                        if ui.button(label).clicked() && let Err(e) = snapshot.choose_mode(mode) { ui.label(e); }
                    }
                }
            });
        }
        for blocker in &candidate.blockers {
            ui.label(&blocker.detail);
        }
        for warning in &candidate.warnings {
            ui.label(&warning.detail);
        }
        ui.small("Preview is an inspection snapshot. Launch rechecks resolver selection, hashes and configuration. shadPS4 may write its own user state.");
        if snapshot.inspection.is_none() && snapshot.error.is_none() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        if ui.button("Recheck shadPS4").clicked() {
            snapshot.refresh();
        }
        if ui.button("Choose shadPS4 core executable…").clicked()
            && let Some(path) = rfd::FileDialog::new().pick_file()
        {
            // Reuse existing override persistence. This supplies discovery
            // evidence; AUTO/PREFERRED/FORCED still select the actual pair.
            match overrides::classify_picked_executable(&path) {
                overrides::PickedExecutable::Usable => {
                    let mut paths = overrides::EmulatorPathOverrides::load();
                    paths.set_executable(OverridableEmulator::ShadPs4, Some(path));
                    snapshot.refresh();
                }
                _ => {
                    ui.label(
                        "Choose a regular executable file; the previous override was retained.",
                    );
                }
            }
        }
        if snapshot
            .inspection
            .as_ref()
            .is_some_and(|i| !i.binding.configured.is_empty())
            && ui.button("Clear executable override").clicked()
        {
            overrides::EmulatorPathOverrides::load()
                .set_executable(OverridableEmulator::ShadPs4, None);
            snapshot.refresh();
        }
        if let Some(request) = snapshot.request(candidate) {
            launch = Some(request);
        }
    });
    launch
}
#[cfg(test)]
mod tests;
