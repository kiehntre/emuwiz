//! Bounded Blue Play bridge over Gold's existing VICE planner/preflight/spawn.
//! Plain Play and explicitly approved trainers share this request/preflight.
//! Emulator configuration is never rewritten.
pub(super) mod trainers;
use super::*;
use crate::gui_v2::vice_evidence::{self, Evidence, read_regular};
use archivefs_core::launch::{
    LaunchContentKind, LaunchContentRef, StandaloneProfileInput, ViceLaunchRequest,
    build_vice_command_plan, preflight_vice_launch, spawn_vice,
};
use archivefs_core::patch_manager::{
    ViceC64ExecutableKind, ViceProfileDiscoveryRoots, discover_vice_profiles,
    resolve_vice_native_launch_binding,
};
use sha2::Digest;

/// Native x64sc 3.7.1, exercised with an isolated synthetic standard 8K CRT.
/// Other builds need a separately verified contract, never a name-only approval.
const VERIFIED_X64SC_SHA256: &str =
    "e3fafd35e65ed271011cd694790998bfabe20a91b64acf9efe61d50795d5cc1c";
const MAX_EXECUTABLE_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const LIMITATION: &str = "VICE Play: verified native x64sc 3.7.1 and a current exact C64 DAT match for a standard 8 KiB ROM .crt only. VICE must already have its system ROMs configured. Process start does not prove the game ran.";

#[derive(Clone)]
pub(crate) struct Context {
    pub selected: PathBuf,
    evidence: Evidence,
    roots: ViceProfileDiscoveryRoots,
    profiles: Vec<(StandaloneProfileInput, Request)>,
}

#[derive(Clone)]
pub(crate) struct Request {
    launch: ViceLaunchRequest,
    roots: ViceProfileDiscoveryRoots,
    evidence: Evidence,
    executable_sha256: String,
}

fn identity(evidence: &Evidence) -> CanonicalIdentityStatus {
    CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "Commodore 64".into(),
        game_key: format!("sha256:{}", evidence.sha256),
    })
}

pub(super) fn command_preview(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
) -> Option<LaunchCommandSpec> {
    preview_with_roots(
        plan,
        candidate,
        &ViceProfileDiscoveryRoots::from_environment(),
    )
}

fn preview_with_roots(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
    roots: &ViceProfileDiscoveryRoots,
) -> Option<LaunchCommandSpec> {
    let LaunchTarget::Standalone {
        adapter_id: "vice",
        profile_id,
        ..
    } = &candidate.target
    else {
        return None;
    };
    let profile = discover_vice_profiles(roots)
        .profiles
        .into_iter()
        .find(|p| &p.profile_id == profile_id)?;
    let command = build_vice_command_plan(
        &preview_identity(plan),
        candidate,
        &resolve_vice_native_launch_binding(&profile),
    )
    .command?;
    Some(LaunchCommandSpec {
        executable: command.executable,
        arguments: command.arguments,
        working_directory: command.working_directory,
    })
}

impl Context {
    fn inspect(selected: PathBuf) -> Result<Self, String> {
        let evidence = vice_evidence::current(&selected)?;
        Self::inspect_with(
            selected,
            evidence,
            ViceProfileDiscoveryRoots::from_environment(),
            VERIFIED_X64SC_SHA256,
        )
    }

    fn inspect_with(
        selected: PathBuf,
        evidence: Evidence,
        roots: ViceProfileDiscoveryRoots,
        supported_digest: &str,
    ) -> Result<Self, String> {
        let mut profiles = Vec::new();
        for profile in discover_vice_profiles(&roots).profiles {
            let Some(executable) = &profile.executable else {
                continue;
            };
            if !profile.eligible || executable.kind != ViceC64ExecutableKind::X64sc {
                continue;
            }
            let (bytes, executable_identity) =
                read_regular(&executable.path, MAX_EXECUTABLE_BYTES)?;
            let digest = crate::gui_v2::vice_evidence::hex(&sha2::Sha256::digest(&bytes));
            if digest != supported_digest {
                continue;
            }
            let CanonicalIdentityStatus::Resolved(id) = identity(&evidence) else {
                unreachable!()
            };
            let request = Request {
                launch: ViceLaunchRequest {
                    selected_content_path: selected.clone(),
                    expected_platform_id: id.platform_id,
                    expected_game_key: id.game_key,
                    profile_id: profile.profile_id.clone(),
                    expected_executable: executable.path.clone(),
                    content_identity: evidence.file,
                    executable_identity,
                },
                roots: roots.clone(),
                evidence: evidence.clone(),
                executable_sha256: digest,
            };
            preflight_vice_launch(&request.launch, &roots, &identity(&evidence))
                .map_err(|e| e.detail)?;
            profiles.push((
                StandaloneProfileInput {
                    adapter_id: "vice",
                    profile_id: profile.profile_id,
                    profile_path: Some(executable.path.clone()),
                    eligible: true,
                    firmware: FirmwareReadiness::NotRequired,
                },
                request,
            ));
        }
        Ok(Self {
            selected,
            evidence,
            roots,
            profiles,
        })
    }

    pub(crate) fn inputs(
        &self,
        prior: &CanonicalIdentityStatus,
    ) -> Option<(CanonicalIdentityStatus, LaunchContentRef)> {
        let id = identity(&self.evidence);
        if !matches!(prior, CanonicalIdentityStatus::Unknown) && prior != &id {
            return None;
        }
        Some((
            id,
            LaunchContentRef {
                kind: Some(LaunchContentKind::Cartridge),
                container: Some(LaunchContainerKind::PlainFile),
                resolved_path: Some(self.selected.clone()),
                requires_mount: false,
                provenance: "Current exact C64 DAT hashes and Gold-validated standard 8K CRT"
                    .into(),
            },
        ))
    }

    pub(crate) fn profiles(&self) -> impl Iterator<Item = StandaloneProfileInput> + '_ {
        self.profiles.iter().map(|(profile, _)| profile.clone())
    }

    pub(crate) fn apply(&self, plan: &mut LaunchPlan) {
        // This new content/identity bridge authorizes VICE only. It must not
        // turn an unverified C64 libretro configuration into another Play lane.
        for candidate in &mut plan.candidates {
            if !matches!(
                candidate.target,
                LaunchTarget::Standalone {
                    adapter_id: "vice",
                    ..
                }
            ) {
                candidate.blockers.push(LaunchBlocker::new(LaunchBlockerKind::CandidateBlocked,
                    "This cartridge readiness check authorizes the verified native VICE configuration only."));
                candidate.readiness = LaunchReadiness::Blocked;
            }
        }
        plan.summary.ready = plan
            .candidates
            .iter()
            .filter(|c| c.readiness == LaunchReadiness::Ready)
            .count();
        plan.summary.ready_with_warnings = plan
            .candidates
            .iter()
            .filter(|c| c.readiness == LaunchReadiness::ReadyWithWarnings)
            .count();
        plan.summary.blocked = plan
            .candidates
            .iter()
            .filter(|c| c.readiness == LaunchReadiness::Blocked)
            .count();
    }

    pub(crate) fn request(
        &self,
        plan: &LaunchPlan,
        candidate: &LaunchCandidate,
    ) -> Option<Request> {
        let LaunchTarget::Standalone {
            adapter_id: "vice",
            profile_id,
            profile_path,
        } = &candidate.target
        else {
            return None;
        };
        let (_, request) = self
            .profiles
            .iter()
            .find(|(p, _)| &p.profile_id == profile_id)?;
        (candidate.readiness == LaunchReadiness::Ready
            && candidate.blockers.is_empty()
            && candidate.warnings.is_empty()
            && !candidate.content.requires_mount
            && candidate.content.container == Some(LaunchContainerKind::PlainFile)
            && candidate.content.kind == Some(LaunchContentKind::Cartridge)
            && candidate.content.resolved_path.as_ref() == Some(&self.selected)
            && profile_path.as_ref() == Some(&request.launch.expected_executable)
            && plan.platform_id.as_deref() == Some("Commodore 64")
            && plan.game_key.as_deref() == Some(request.launch.expected_game_key.as_str()))
        .then(|| request.clone())
    }

    pub(crate) fn show(
        &self,
        ui: &mut egui::Ui,
        plan: &LaunchPlan,
        candidate: &LaunchCandidate,
        state: &mut StandaloneLaunchState,
    ) {
        let Some(request) = self.request(plan, candidate) else {
            return;
        };
        let command = preview_with_roots(plan, candidate, &self.roots);
        ui.group(|ui| {
            ui.label(LIMITATION);
            if let Some(command) = &command {
                ui.label(format!("Executable: {}", command.executable.display()));
                ui.label("Working directory: inherited");
                for (i, arg) in command.arguments.iter().enumerate() { ui.label(format!("Argument [{i}]: {}", arg.to_string_lossy())); }
            } else {
                ui.label("VICE command refused; refresh readiness.");
            }
            ui.small("Preview only. Play rechecks the DAT, cartridge and executable before Gold preflight and spawn.");
            if state.is_active() && !state.tracked.as_ref().is_some_and(|(p, a, _)| p == &self.selected && a == "vice") {
                ui.label("Another standalone launch is active; wait before starting VICE.");
            } else {
                // Keep historical process feedback visible when current
                // readiness fails, while disabling new launch actions.
                ui.add_enabled_ui(command.is_some(), |ui| {
                    show_standalone_launch_action(ui, state, StandaloneLaunchRequest::Vice(request.clone()));
                    let busy = state.is_active();
                    if let Some(selection) = state.vice_trainers.show(ui, &request, busy) {
                        trainers::start(state, request, selection);
                    }
                });
            }
        });
    }
}

impl Request {
    pub(crate) fn key(&self) -> (PathBuf, String) {
        (self.launch.selected_content_path.clone(), "vice".into())
    }
    pub(crate) fn execute(
        self,
    ) -> Result<archivefs_core::launch::process_spawn::WatchedProcess, String> {
        self.execute_checked(vice_evidence::current)
    }
    fn execute_checked(
        self,
        read: impl FnOnce(&Path) -> Result<Evidence, String>,
    ) -> Result<archivefs_core::launch::process_spawn::WatchedProcess, String> {
        let fresh = read(&self.launch.selected_content_path)
            .map_err(|e| format!("VICE was not started: {e}"))?;
        self.execute_with(fresh)
    }
    fn execute_with(
        self,
        fresh: Evidence,
    ) -> Result<archivefs_core::launch::process_spawn::WatchedProcess, String> {
        let command = self.command_with(&fresh)?;
        spawn_vice(&command).map_err(|e| format!("VICE could not be started: {e}"))
    }

    fn command_with(
        &self,
        fresh: &Evidence,
    ) -> Result<archivefs_core::launch::process_spawn::PreparedProcessCommand, String> {
        if fresh != &self.evidence {
            return Err(
                "VICE was not started: game or DAT evidence changed; refresh readiness.".into(),
            );
        }
        let (bytes, file) = read_regular(&self.launch.expected_executable, MAX_EXECUTABLE_BYTES)?;
        if file != self.launch.executable_identity
            || crate::gui_v2::vice_evidence::hex(&sha2::Sha256::digest(&bytes))
                != self.executable_sha256
        {
            return Err("VICE was not started: executable changed; refresh readiness.".into());
        }
        let command = preflight_vice_launch(&self.launch, &self.roots, &identity(fresh))
            .map_err(|e| format!("VICE was not started: {} ({:?})", e.detail, e.kind))?;
        Ok(command)
    }
}

#[derive(Default)]
pub(crate) struct Controller {
    path: Option<PathBuf>,
    receiver: Option<(PathBuf, Receiver<Result<Context, String>>)>,
    context: Option<Context>,
    error: Option<String>,
}
impl Controller {
    pub(crate) fn context(&self, path: Option<&Path>) -> Option<Context> {
        self.context
            .as_ref()
            .filter(|c| Some(c.selected.as_path()) == path && self.path.as_deref() == path)
            .cloned()
    }
    fn start(&mut self, path: PathBuf, ctx: egui::Context) {
        self.path = Some(path.clone());
        self.context = None;
        self.error = None;
        let (tx, rx) = mpsc::channel();
        self.receiver = Some((path.clone(), rx));
        thread::spawn(move || {
            let _ = tx.send(Context::inspect(path));
            ctx.request_repaint();
        });
    }
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, path: Option<&Path>) {
        if self.path.as_deref() != path {
            self.context = None;
            self.error = None;
            self.path = path.map(Path::to_path_buf);
        }
        if let Some((working, receiver)) = &self.receiver {
            let belongs = self.path.as_ref() == Some(working);
            match receiver.try_recv() {
                Ok(result) => {
                    if belongs {
                        match result {
                            Ok(c) => self.context = Some(c),
                            Err(e) => self.error = Some(e),
                        }
                    }
                    self.receiver = None;
                }
                Err(TryRecvError::Disconnected) => {
                    if belongs {
                        self.error = Some("VICE check stopped; refresh readiness.".into());
                    }
                    self.receiver = None;
                }
                Err(TryRecvError::Empty) => ui
                    .ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100)),
            }
        }
        let Some(path) =
            path.filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("crt")))
        else {
            return;
        };
        if self.receiver.is_none() && self.context.is_none() && self.error.is_none() {
            self.start(path.into(), ui.ctx().clone());
        }
        ui.collapsing("C64 VICE readiness", |ui| {
            ui.label(LIMITATION);
            if self.receiver.is_some() { ui.label("Checking current DAT evidence, cartridge and native VICE executable…"); }
            if let Some(e) = &self.error { ui.label(format!("VICE Play refused: {e}")); }
            if self.context.as_ref().is_some_and(|c| c.profiles.is_empty()) { ui.label("No verified native x64sc binary found. Symlink wrappers, x64 and other builds are unsupported by this initial slice."); }
            if ui.add_enabled(self.receiver.is_none(), egui::Button::new("Refresh VICE readiness")).clicked() { self.start(path.into(), ui.ctx().clone()); }
        });
    }
}

#[cfg(test)]
mod tests;
