//! Native RPCS3 local patch activation. Definitions and Launch preferences are read-only.
use crate::*;
use archivefs_core::patch_manager::*;
use std::path::{Path, PathBuf};
use std::result::Result;
use std::sync::mpsc::{self, Receiver};

pub(crate) fn is_rpcs3_route(w: &CheatWorkflowState) -> bool {
    matches!(&w.routing.decision, CheatRouteDecision::Routed(r) if matches!(&r.target, CheatRouteTarget::Standalone { adapter_id } if adapter_id == "rpcs3"))
}
fn selected_launch_profile<'a>(
    plan: &'a archivefs_core::launch::LaunchPlan,
    game: &Path,
) -> Result<&'a str, String> {
    use archivefs_core::launch::{LaunchReadiness, LaunchTarget};
    let c = crate::gui_v2::launch_readiness_summary::selected_candidate(plan)
        .ok_or("Launch has no unambiguous emulator/profile selection")?;
    if c.readiness == LaunchReadiness::Blocked
        || !c.blockers.is_empty()
        || c.content.requires_mount
        || c.content.resolved_path.as_deref() != Some(game)
    {
        return Err("Launch is blocked or selects different/unresolved content".into());
    }
    match &c.target {
        LaunchTarget::Standalone {
            adapter_id: "rpcs3",
            profile_id,
            ..
        } => Ok(profile_id),
        _ => Err("Launch selects another emulator; patches do not change Launch selection".into()),
    }
}
impl ArchiveFsApp {
    pub(crate) fn rpcs3_cheat_selection(
        &self,
        live: Option<&LoadedData>,
    ) -> Result<Rpcs3ActivationSelection, String> {
        let w = self.cheat_workflow.as_ref().ok_or("Choose a game")?;
        if !is_rpcs3_route(w) {
            return Err("RPCS3 patches are not selected".into());
        }
        match self.build_launch_readiness_input(live) {
            crate::launch_readiness_page::LaunchReadinessInput::Plan {
                plan,
                rpcs3: Some(context),
                ..
            } => {
                let profile_id = selected_launch_profile(&plan, &w.archive_path)?;
                let profile = context
                    .discovery
                    .profiles
                    .iter()
                    .find(|p| p.profile_id == profile_id)
                    .ok_or("Launch profile disappeared")?;
                let binding = resolve_rpcs3_native_launch_binding(profile).map_err(|e| e.detail)?;
                Ok(Rpcs3ActivationSelection {
                    game: w.archive_path.clone(),
                    roots: context.roots,
                    profile_id: profile_id.to_owned(),
                    configuration_root: profile.configuration_path.clone(),
                    executable: binding.executable,
                })
            }
            _ => Err("Resolve this game's RPCS3 Launch Readiness first".into()),
        }
    }
}

#[derive(Clone)]
struct Environment {
    history: PathBuf,
    backups: PathBuf,
}
fn environment() -> Result<Environment, String> {
    #[cfg(test)]
    {
        Err("Tests must supply disposable History and profiles explicitly".into())
    }
    #[cfg(not(test))]
    {
        Ok(Environment {
            history: default_shared_history_root().map_err(|e| format!("{e:?}"))?,
            backups: default_shared_backup_root().map_err(|e| format!("{e:?}"))?,
        })
    }
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
enum Message {
    Catalog(Rpcs3ActivationSelection, Rpcs3ActivationCatalog),
    Preview(Box<Rpcs3ActivationPreview>),
    Applied(Box<SharedApplyResult>),
    History(Vec<(SharedTransactionPath, SharedApplyJournal)>),
    UndoPreview(SharedRollbackPreview),
    Undone(SharedRollbackResult),
}
#[derive(Default)]
pub(crate) struct Rpcs3PageState {
    pub(crate) selection: Option<Result<Rpcs3ActivationSelection, String>>,
    context: Option<(
        Option<Result<Rpcs3ActivationSelection, String>>,
        Option<GameIdentityReport>,
    )>,
    receiver: Option<Receiver<Result<Message, String>>>,
    mutating: bool,
    catalog: Option<Rpcs3ActivationCatalog>,
    selected: Option<usize>,
    enabled: bool,
    preview: Option<Box<Rpcs3ActivationPreview>>,
    confirmed: bool,
    result: Option<Box<SharedApplyResult>>,
    history: Vec<(SharedTransactionPath, SharedApplyJournal)>,
    undo: Option<SharedRollbackPreview>,
    undo_confirmed: bool,
    outcome: Option<String>,
    failure: Option<String>,
}
impl Rpcs3PageState {
    #[cfg(test)]
    pub(crate) fn pending_activation_fixture() -> Self {
        // A disconnected worker stays busy until its owning panel polls it.
        // This models a retained workflow without launching a thread or files.
        let (_sender, receiver) = mpsc::channel();
        Self {
            receiver: Some(receiver),
            mutating: true,
            ..Self::default()
        }
    }

    pub(crate) fn busy(&self) -> bool {
        self.mutating && self.receiver.is_some()
    }
    fn update_context(&mut self, identity: Option<&GameIdentityReport>) {
        let key = (self.selection.clone(), identity.cloned());
        if self.context.as_ref() != Some(&key) {
            self.confirmed = false;
            self.undo_confirmed = false;
            self.preview = None;
            self.catalog = None;
            self.selected = None;
            self.enabled = false;
            self.context = Some(key);
        }
    }
    fn spawn(
        &mut self,
        ctx: &egui::Context,
        mutating: bool,
        work: impl FnOnce() -> Result<Message, String> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        self.receiver = Some(rx);
        self.mutating = mutating;
        self.failure = None;
        self.confirmed = false;
        self.undo_confirmed = false;
        if mutating {
            self.outcome = Some("Operation running; completion is not confirmed. If interrupted, inspect retained History before retrying.".into());
        }
        std::thread::spawn(move || {
            let r = work();
            let _ = tx.send(r);
            ctx.request_repaint();
        });
    }
    fn poll(&mut self) {
        let r = match self.receiver.as_ref().map(|r| r.try_recv()) {
            Some(Ok(r)) => r,
            Some(Err(mpsc::TryRecvError::Disconnected)) => Err(
                "Worker interrupted; inspect History for the recorded outcome before retrying"
                    .into(),
            ),
            _ => return,
        };
        self.receiver = None;
        self.mutating = false;
        self.confirmed = false;
        match r {
            Err(e) => {
                self.failure = Some(e);
                self.preview = None;
                self.outcome = None;
            }
            Ok(Message::Catalog(s, c)) => {
                if self.selection.as_ref() == Some(&Ok(s)) {
                    self.catalog = Some(c);
                    self.selected = None;
                    self.enabled = false;
                    self.preview = None;
                } else {
                    self.failure =
                        Some("Launch selection changed during inspection; inspect again".into());
                }
            }
            Ok(Message::Preview(p)) => {
                if self.selection.as_ref() == Some(&Ok(p.request().selection.clone())) {
                    self.preview = Some(p);
                    self.outcome =
                        Some("Preview only; no configuration or game files changed".into());
                } else {
                    self.failure = Some("Launch selection changed; Preview again".into());
                }
            }
            Ok(Message::Applied(r)) => {
                self.outcome = Some(format!(
                    "Recorded Apply: {:?} · receipt {:?}. In-game effectiveness remains unverified.",
                    r.journal.status, r.journal_path
                ));
                if r.journal.status != SharedApplyStatus::Success {
                    self.failure = Some(format!(
                        "Apply incomplete/refused: {:?}; receipt issue {:?}. Retain recovery evidence and inspect History.",
                        r.journal.entries, r.journal_failure
                    ));
                }
                self.result = Some(r);
                self.preview = None;
                self.catalog = None;
                self.selected = None;
                self.enabled = false;
            }
            Ok(Message::History(rows)) => {
                self.history = rows;
            }
            Ok(Message::UndoPreview(p)) => {
                self.undo = Some(p);
                self.undo_confirmed = false;
            }
            Ok(Message::Undone(r)) => {
                self.outcome = Some(format!(
                    "Recorded Undo: {:?} · receipt {:?}",
                    r.status, r.journal_path
                ));
                if r.status != SharedApplyStatus::Success {
                    self.failure =
                        Some("Undo incomplete/refused; retain evidence and review History".into());
                }
                self.undo = None;
                self.undo_confirmed = false;
                self.preview = None;
                self.catalog = None;
                self.selected = None;
                self.enabled = false;
            }
        }
    }
}
fn apply(
    p: &Rpcs3ActivationPreview,
    current: &Rpcs3ActivationSelection,
    env: &Environment,
) -> Result<SharedApplyResult, String> {
    apply_rpcs3_activation(
        p,
        current,
        &SharedApplyOptions {
            dry_run: false,
            confirmation: Some(SharedApplyConfirmation {
                plan_id: p.plan().plan_id.clone(),
                general_approved: true,
                replacement_approved: false,
            }),
            operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: now(),
            current_context: p.plan().context.clone(),
            history_root: env.history.clone(),
            backup_root: env.backups.clone(),
        },
    )
}
pub(crate) fn show(
    ui: &mut egui::Ui,
    state: &mut Rpcs3PageState,
    game: &Path,
    identity: Option<&GameIdentityReport>,
) {
    state.update_context(identity);
    state.poll();
    ui.heading("RPCS3 local patches");
    ui.label("Format recognised: native patch YAML 1.2. Compatible declarations require verified TITLE_ID and exact local APP_VER. Preview and config-only activation are available for the supported subset; in-game effectiveness is unverified.");
    ui.label("Only already-installed local definitions are read. No downloads or patch database edits. This route creates an absent patch_config.yml; existing user configurations are protected. Undo removes only the unchanged configuration created by this operation.");
    ui.label("Supported: native Linux XDG profile, extracted PS3_GAME folder, no installed update/custom VFS/portable profile, simple numeric direct patches. ISO/PKG, custom/Flatpak/AppImage profiles and unsupported YAML stay unavailable here.");
    let selection = state
        .selection
        .as_ref()
        .and_then(|s| s.as_ref().ok())
        .cloned();
    match &state.selection {
        Some(Ok(s)) => {
            ui.label(format!(
                "Launch profile: {} · {}\nExecutable: {}",
                s.profile_id,
                s.configuration_root.display(),
                s.executable.display()
            ));
        }
        Some(Err(e)) => {
            ui.colored_label(egui::Color32::YELLOW, e);
        }
        None => {
            ui.label("Launch selection is not resolved");
        }
    }
    let idle = state.receiver.is_none();
    if ui
        .add_enabled(
            idle && selection.is_some(),
            egui::Button::new("Inspect installed local RPCS3 patches"),
        )
        .clicked()
    {
        let selection = selection.clone().unwrap();
        state.spawn(ui.ctx(), false, move || {
            inspect_rpcs3_activation(&selection).map(|c| Message::Catalog(selection, c))
        });
    }
    let mut changed = false;
    if let Some(c) = &state.catalog {
        ui.label(format!(
            "Verified game: {} · APP_VER {} (no installed update override)",
            c.title_id, c.app_version
        ));
        ui.label("Matching declarations do not prove the runtime PPU hash or an in-game effect; RPCS3 still validates its actual executable hash.");
        for (i, e) in c.entries.iter().enumerate() {
            ui.horizontal_wrapped(|ui| {
                let compatible = matches!(
                    e.readiness,
                    Rpcs3PatchReadiness::ExactCompatible
                        | Rpcs3PatchReadiness::AllVersionsCompatible
                );
                if ui
                    .add_enabled(
                        idle && compatible,
                        egui::RadioButton::new(
                            state.selected == Some(i),
                            &e.definition.description,
                        ),
                    )
                    .clicked()
                {
                    state.selected = Some(i);
                    state.enabled = false;
                    changed = true;
                }
                ui.label(format!(
                    "{:?} · versions {:?}",
                    e.readiness, e.definition.versions
                ));
            });
            ui.small(format!(
                "{} · {} · {}",
                e.source.display(),
                e.hash,
                e.source_sha256
            ));
            ui.collapsing(format!("Operations: {}", e.definition.description), |ui| {
                for op in &e.definition.operations {
                    ui.monospace(format!("{} {} {}", op.kind, op.offset, op.value));
                }
            });
        }
        if c.entries.is_empty() {
            ui.label("No supported local patch definitions are available");
        }
    }
    changed |= ui
        .add_enabled(
            idle && state.selected.is_some(),
            egui::Checkbox::new(&mut state.enabled, "Enable selected patch in RPCS3"),
        )
        .changed();
    if changed {
        state.preview = None;
        state.confirmed = false;
    }
    if ui
        .add_enabled(
            idle && selection.is_some()
                && state
                    .catalog
                    .as_ref()
                    .is_some_and(|c| state.selected.is_some_and(|i| i < c.entries.len())),
            egui::Button::new("Preview exact configuration change"),
        )
        .clicked()
    {
        let c = state.catalog.as_ref().unwrap();
        let e = &c.entries[state.selected.unwrap()];
        let request = Rpcs3ActivationRequest {
            selection: selection.clone().unwrap(),
            source: e.source.clone(),
            expected_source_sha256: e.source_sha256.clone(),
            hash: e.hash.clone(),
            description: e.definition.description.clone(),
            expected_title_id: c.title_id.clone(),
            expected_app_version: c.app_version.clone(),
            enabled: state.enabled,
        };
        state.spawn(ui.ctx(), false, move || {
            preview_rpcs3_activation(&request).map(|p| Message::Preview(Box::new(p)))
        });
    }
    if let Some(p) = &state.preview {
        ui.heading("Exact Preview — not applied");
        ui.label(format!(
            "Create: {}\nBefore: absent\nAfter:",
            p.destination.display()
        ));
        ui.monospace(String::from_utf8_lossy(&p.config_bytes).as_ref());
        ui.label("Definition databases, games, firmware, RAP licences and Launch settings receive no writes. Creating this file may enable the selected patch next time RPCS3 runs; runtime effectiveness remains unverified.");
        ui.add_enabled(idle, egui::Checkbox::new(&mut state.confirmed, "I reviewed this exact configuration and enable state. RPCS3 is closed. Apply only this Preview."));
        if ui
            .add_enabled(
                idle && state.confirmed && selection.is_some(),
                egui::Button::new("Apply confirmed RPCS3 configuration"),
            )
            .clicked()
        {
            let p = p.clone();
            let current = selection.clone().unwrap();
            state.spawn(ui.ctx(), true, move || {
                apply(&p, &current, &environment()?).map(|r| Message::Applied(Box::new(r)))
            });
        }
    }
    if let Some(m) = &state.outcome {
        ui.label(m);
    }
    if let Some(e) = &state.failure {
        ui.colored_label(egui::Color32::YELLOW, e);
    }
    if !idle {
        ui.spinner();
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
    ui.separator();
    ui.heading("RPCS3 activation History / Undo");
    if ui
        .add_enabled(
            idle,
            egui::Button::new("Refresh this game's activation History"),
        )
        .clicked()
    {
        let game = game.to_path_buf();
        state.spawn(ui.ctx(), false, move || {
            let env = environment()?;
            let report = discover_shared_apply_history(&env.history);
            if !report.complete || !report.warnings.is_empty() {
                return Err("History evidence is incomplete; inspect History & Logs".into());
            }
            Ok(Message::History(
                report
                    .journals
                    .into_iter()
                    .filter(|(_, j)| {
                        j.context.adapter == PreviewAdapter::Rpcs3Patch
                            && is_rpcs3_activation_source_mode(&j.context.source_mode)
                            && j.context.selected_archive.to_path_buf().ok().as_ref() == Some(&game)
                    })
                    .collect(),
            ))
        });
    }
    let mut choose = None;
    if let Some(r) = &state.result
        && let Some(path) = &r.journal_path
    {
        ui.label(format!(
            "Recorded operation {} · {:?}",
            r.journal.operation_id, r.journal.status
        ));
        if ui
            .add_enabled(idle, egui::Button::new("Preview Undo this activation"))
            .clicked()
        {
            choose = r
                .journal
                .destination_root
                .to_path_buf()
                .ok()
                .map(|root| (path.clone(), root));
        }
    }
    for (path, j) in &state.history {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{} · {:?} (recorded outcome; current Undo eligibility needs a fresh preview)",
                j.operation_id, j.status
            ));
            if ui
                .add_enabled(
                    idle,
                    egui::Button::new(format!("Preview Undo {}", j.operation_id)),
                )
                .clicked()
            {
                choose = path
                    .to_path_buf()
                    .ok()
                    .zip(j.destination_root.to_path_buf().ok());
            }
        });
    }
    if let Some((path, root)) = choose {
        state.spawn(ui.ctx(), false, move || {
            let env = environment()?;
            Ok(Message::UndoPreview(preview_rpcs3_patch_rollback(
                &path,
                &root,
                &env.backups,
            )))
        });
    }
    if let Some(p) = &state.undo {
        ui.label(format!("Undo available: {} · {:?}", p.available, p.entries));
        ui.add_enabled(
            idle && p.available,
            egui::Checkbox::new(
                &mut state.undo_confirmed,
                "RPCS3 is closed; Undo this exact recorded activation",
            ),
        );
        if ui
            .add_enabled(
                idle && p.available && state.undo_confirmed,
                egui::Button::new("Undo confirmed RPCS3 activation"),
            )
            .clicked()
        {
            let p = p.clone();
            state.spawn(ui.ctx(), true, move || {
                let env = environment()?;
                if observe_emulator_process(&CheatRouteTarget::standalone("rpcs3"))
                    != EmulatorProcessObservation::NotRunning
                {
                    return Err(
                        "RPCS3 must be closed before Undo; process state is running or unverified"
                            .into(),
                    );
                }
                Ok(Message::Undone(rollback_rpcs3_patch(
                    &p,
                    &SharedRollbackOptions {
                        confirmation: SharedRollbackConfirmation {
                            preview_id: p.preview_id.clone(),
                            approved: true,
                        },
                        rollback_operation_id: generate_shared_operation_id(),
                        timestamp_unix_seconds: now(),
                        history_root: env.history,
                        backup_root: env.backups,
                    },
                )))
            });
        }
    }
}
#[cfg(test)]
mod tests;
