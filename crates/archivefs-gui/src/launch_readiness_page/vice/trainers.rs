//! Explicit, single-launch C64 choices over the Stage 1 VICE request.
use super::*;
use archivefs_core::classic_trainer_evidence::{
    ClassicTrainerGame, exact_game_identities, vice as gold,
};
use archivefs_core::launch::cheat_runtime::{
    CheatRuntimeRoots, CheatRuntimeSession, LiveLaunchBinding, execute_cheat_runtime,
};
use archivefs_core::launch::process_spawn::WatchedProcess;
use archivefs_core::launch::vice_execution::preflight_vice_launch_with_cheats;
use archivefs_core::safe_read::TrustedRoots;
use std::collections::BTreeSet;

#[derive(Clone, PartialEq, Eq)]
struct Scope {
    launch: ViceLaunchRequest,
    evidence: Evidence,
    executable_sha256: String,
}
impl Scope {
    fn of(request: &Request) -> Self {
        Self {
            launch: request.launch.clone(),
            evidence: request.evidence.clone(),
            executable_sha256: request.executable_sha256.clone(),
        }
    }
}
type PreviewResult = Result<gold::Preview, String>;
type Pending = (Scope, Receiver<PreviewResult>);

#[derive(Default)]
pub(in crate::launch_readiness_page) struct Chooser {
    scope: Option<Scope>,
    enabled: bool,
    selected: BTreeSet<usize>,
    approved: bool,
    preview: Option<gold::Preview>,
    pending: Option<Pending>,
    problem: Option<String>,
}
#[derive(Clone)]
pub(in crate::launch_readiness_page) struct Selection {
    scope: Scope,
    preview: gold::Preview,
    entries: Vec<usize>,
}

fn game(evidence: &Evidence) -> ClassicTrainerGame {
    ClassicTrainerGame {
        platform: "Commodore 64".into(),
        identities: exact_game_identities(&evidence.hashes),
        identity_verified: true,
    }
}
fn current(request: &Request) -> Result<(Evidence, TrustedRoots), String> {
    let fresh = vice_evidence::current(&request.launch.selected_content_path)?;
    let config = archivefs_core::Config::load_default().map_err(|e| e.to_string())?;
    Ok((fresh, TrustedRoots::from_paths(config.source_folders)))
}

// Gold may keep other profiles launchable beside a remembered profile. Only
// its unique preferred request owns this single-launch trainer approval.
fn selected_request(context: &Context, plan: &LaunchPlan) -> Option<Request> {
    use archivefs_core::launch::CandidatePreference;
    let mut selected = plan
        .candidates
        .iter()
        .filter(|candidate| {
            matches!(
                candidate.preference,
                CandidatePreference::SoleEligible | CandidatePreference::Remembered
            )
        })
        .filter_map(|candidate| context.request(plan, candidate));
    let request = selected.next()?;
    selected.next().is_none().then_some(request)
}

pub(super) fn is_selected(context: &Context, plan: &LaunchPlan, request: &Request) -> bool {
    selected_request(context, plan)
        .is_some_and(|selected| Scope::of(&selected) == Scope::of(request))
}

impl Chooser {
    pub(in crate::launch_readiness_page) fn focus(&mut self, path: Option<&Path>) {
        if self
            .scope
            .as_ref()
            .is_some_and(|scope| Some(scope.launch.selected_content_path.as_path()) != path)
        {
            self.cancel_approval();
            self.scope = None;
            self.preview = None;
            self.problem = None;
        }
    }

    pub(in crate::launch_readiness_page) fn reconcile(&mut self, input: &LaunchReadinessInput) {
        let retained = match input {
            LaunchReadinessInput::Plan {
                plan,
                vice: Some(context),
                ..
            } => selected_request(context, plan)
                .is_some_and(|request| self.scope.as_ref() == Some(&Scope::of(&request))),
            _ => false,
        };
        if !retained {
            self.cancel_approval();
            self.scope = None;
            self.preview = None;
            self.problem = None;
            // Keep at most one old worker; show() discards a result whose
            // complete scope no longer matches. No approval survives.
        }
    }

    pub(in crate::launch_readiness_page) fn cancel_approval(&mut self) {
        self.enabled = false;
        self.selected.clear();
        self.approved = false;
    }
    pub(in crate::launch_readiness_page) fn show(
        &mut self,
        ui: &mut egui::Ui,
        request: &Request,
        busy: bool,
    ) -> Option<Selection> {
        let scope = Scope::of(request);
        if self.scope.as_ref() != Some(&scope) {
            // Keep a single old worker until it finishes, but never reuse its
            // result for a different game or backend approval.
            self.scope = Some(scope.clone());
            self.enabled = false;
            self.selected.clear();
            self.approved = false;
            self.preview = None;
            self.problem = None;
        }
        if let Some((working, receiver)) = &self.pending {
            let belongs = working == &scope;
            match receiver.try_recv() {
                Ok(result) => {
                    if belongs {
                        match result {
                            Ok(p) => self.preview = Some(p),
                            Err(e) => self.problem = Some(e),
                        }
                    }
                    self.pending = None;
                }
                Err(TryRecvError::Disconnected) => {
                    if belongs {
                        self.problem = Some("Trainer check stopped; refresh required.".into());
                    }
                    self.pending = None;
                }
                Err(TryRecvError::Empty) => ui
                    .ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100)),
            }
        }
        let mut selection = None;
        ui.collapsing("C64 POKE trainers (optional, default OFF)", |ui| {
            ui.label(gold::LIMITATION);
            ui.add_enabled_ui(!busy, |ui| {
                if ui.checkbox(&mut self.enabled, "Enable explicit C64 trainer selection for this launch").changed() {
                    self.selected.clear(); self.approved = false;
                }
                if !self.enabled { return; }
                if ui.add_enabled(self.pending.is_none(), egui::Button::new("Load / refresh verified C64 trainer entries")).clicked() {
                    self.preview = None; self.selected.clear(); self.approved = false; self.problem = None;
                    let (sender, receiver) = mpsc::channel(); self.pending = Some((scope.clone(), receiver));
                    let request = request.clone(); let ctx = ui.ctx().clone();
                    thread::spawn(move || {
                        let result = current(&request).and_then(|(fresh, roots)| {
                            request.command_with(&fresh)?;
                            gold::preview(&game(&fresh), &request.launch.selected_content_path, &roots)
                        });
                        let _ = sender.send(result); ctx.request_repaint();
                    });
                }
                if self.pending.is_some() { ui.spinner(); }
                if let Some(error) = &self.problem { ui.label(format!("Trainer launch refused: {error}. Create or inspect the association in Game Details, then refresh.")); }
                if let Some(preview) = &self.preview {
                    ui.label(format!("Verified source: {}", preview.source_label()));
                    ui.label("This trainer action adds -moncommands with a temporary file containing only the selected writes. Ordinary Play does not add it.");
                    for (index, title) in preview.titles().iter().enumerate() {
                        let compatibility = preview.compatibility(index);
                        let mut selected = self.selected.contains(&index);
                        let response = ui.add_enabled(compatibility.is_ok(), egui::Checkbox::new(&mut selected, format!("{title} ({} byte writes)", preview.operation_count(index))));
                        if response.changed() { self.approved = false; if selected { self.selected.insert(index); } else { self.selected.remove(&index); } }
                        match compatibility { Ok(()) => { ui.small("Compatible with the bounded startup monitor projection; gameplay effect unconfirmed."); }, Err(e) => { ui.label(format!("Unsupported: {e}")); } }
                        ui.monospace(preview.operation_preview(index));
                    }
                    ui.checkbox(&mut self.approved, "I approve these selected one-shot RAM writes for this launch");
                    let eligible = !self.selected.is_empty() && self.selected.iter().all(|&i| preview.compatibility(i).is_ok());
                    if ui.add_enabled(eligible && self.approved, egui::Button::new("Launch with selected C64 trainer")).clicked() {
                        selection = Some(Selection { scope: scope.clone(), preview: preview.clone(), entries: self.selected.iter().copied().collect() });
                        self.enabled = false; self.selected.clear(); self.approved = false;
                    }
                }
            });
        });
        selection
    }
}

pub(super) fn start(state: &mut StandaloneLaunchState, request: Request, selection: Selection) {
    let (path, adapter) = request.key();
    let roots = state.cheat_roots.clone().unwrap_or_default();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = request
            .launch_trainers(&selection, &roots)
            .map(|s| StandaloneProcess::ViceTrainers(Box::new(s)));
        let _ = sender.send(result);
    });
    state.tracked = Some((path, adapter, StandaloneLaunchStage::Starting(receiver)));
}

impl Request {
    fn launch_trainers(
        self,
        selected: &Selection,
        roots: &CheatRuntimeRoots,
    ) -> Result<CheatRuntimeSession<WatchedProcess>, String> {
        let request = self.clone();
        self.launch_trainers_checked(selected, roots, move || current(&request))
    }

    fn launch_trainers_checked(
        self,
        selected: &Selection,
        roots: &CheatRuntimeRoots,
        mut read: impl FnMut() -> Result<(Evidence, TrustedRoots), String>,
    ) -> Result<CheatRuntimeSession<WatchedProcess>, String> {
        if selected.scope != Scope::of(&self)
            || selected.preview.game_path() != self.launch.selected_content_path
        {
            return Err("Game, emulator or catalogue evidence changed since trainer approval. Select again.".into());
        }
        let (fresh, trusted) = read()?;
        let command = self.command_with(&fresh)?;
        let prepared = gold::prepare(
            &selected.preview,
            &game(&fresh),
            &trusted,
            &selected.entries,
            &self.launch.expected_game_key,
        )?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let launch_id = format!("vice-{}-{nanos}", std::process::id());
        let (plan, handoff) = prepared.runtime_plan(
            &launch_id,
            &roots.approved_root,
            &self.launch.expected_executable,
            &self.executable_sha256,
        )?;
        let binding = LiveLaunchBinding {
            game_identity: self.launch.expected_game_key.clone(),
            adapter_id: "vice".into(),
        };
        let mut session = execute_cheat_runtime(
            &plan,
            &binding,
            roots,
            command,
            |mut command, extra| {
                command.arguments.splice(1..1, extra.iter().cloned());
                command
            },
            |approved| {
                let (fresh, trusted) = read()?;
                // The same Stage 1 validation, then the exact source/entry
                // approval again at the last handoff after materialisation.
                let _ = self.command_with(&fresh)?;
                let now = gold::prepare(
                    &selected.preview,
                    &game(&fresh),
                    &trusted,
                    &selected.entries,
                    &self.launch.expected_game_key,
                )?;
                if now != prepared {
                    return Err("Selected trainer preparation changed before handoff.".into());
                }
                let command = preflight_vice_launch_with_cheats(
                    &self.launch,
                    &self.roots,
                    &identity(&fresh),
                    Some(&handoff),
                )
                .map_err(|e| e.detail)?;
                if command != approved {
                    return Err("VICE command changed after approval.".into());
                }
                spawn_vice(&command).map_err(|e| e.to_string())
            },
        );
        session.note(gold::LIMITATION);
        Ok(session)
    }
}

pub(in crate::launch_readiness_page) fn phase_label(
    session: &CheatRuntimeSession<WatchedProcess>,
) -> &'static str {
    match &session.receipt().process {
        None => "Trainer launch refused",
        Some(process) if process.exit.is_some() => "Process exited",
        Some(_) => "Running",
    }
}

pub(in crate::launch_readiness_page) fn failure_detail(
    session: &CheatRuntimeSession<WatchedProcess>,
) -> Option<String> {
    use archivefs_core::launch::cheat_runtime::HandoffOutcome;
    let receipt = session.receipt();
    (receipt.process.is_none()
        || receipt.handoff != HandoffOutcome::PassedToEmulator
        || !receipt.errors.is_empty())
    .then(|| {
        format!(
            "VICE trainer launch: preparation {:?}; handoff {:?}; cleanup {:?}; {}",
            receipt.materialisation,
            receipt.handoff,
            receipt.cleanup,
            receipt.errors.join("; ")
        )
    })
}

pub(in crate::launch_readiness_page) fn show_receipt(
    ui: &mut egui::Ui,
    session: &CheatRuntimeSession<WatchedProcess>,
) {
    use archivefs_core::launch::cheat_runtime::{HandoffOutcome, MaterialisationOutcome};
    let receipt = session.receipt();
    ui.label(match receipt.materialisation {
        MaterialisationOutcome::Materialised { .. } => {
            "Trainer commands prepared in a disposable workspace."
        }
        _ => "Trainer commands were not prepared successfully.",
    });
    if receipt.handoff == HandoffOutcome::PassedToEmulator {
        ui.label("VICE was started with the prepared command file. Monitor consumption and in-game activation are not confirmed.");
    } else {
        ui.label("Trainer handoff was refused or failed; activation is not confirmed.");
    }
    if let Some(process) = &receipt.process {
        ui.label(format!("VICE PID {}", process.pid));
        if let Some(exit) = &process.exit {
            ui.label(format!(
                "VICE process exited: {exit:?}. This does not prove the game ran."
            ));
        }
    }
    ui.label(format!("Trainer workspace cleanup: {:?}", receipt.cleanup));
    for error in &receipt.errors {
        ui.label(error);
    }
}

#[cfg(test)]
mod tests;
