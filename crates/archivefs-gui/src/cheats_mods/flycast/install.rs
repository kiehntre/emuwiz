//! Blue's asynchronous installation/recovery controller. No emulator is run.
use super::*;
use archivefs_core::patch_manager::flycast_install::*;

#[derive(Default)]
pub(super) struct InstallState {
    receiver: Option<Receiver<ReviewResult<Message>>>,
    generation: u64,
    mutating: bool,
    preview: Option<Box<FlycastInstallPreview>>,
    confirmed: bool,
    history: Vec<(SharedTransactionPath, SharedApplyJournal)>,
    undo: Option<(SharedRollbackPreview, FlycastInstallBinding)>,
    undo_confirmed: bool,
    outcome: Option<String>,
    failure: Option<String>,
}
enum Message {
    Preview(u64, Box<FlycastInstallPreview>),
    Applied(Box<SharedApplyResult>),
    History(Vec<(SharedTransactionPath, SharedApplyJournal)>),
    SelectionSaved,
    UndoPreview(u64, SharedRollbackPreview, FlycastInstallBinding),
    Undone(SharedRollbackResult),
}
impl InstallState {
    pub(super) fn busy(&self) -> bool {
        self.mutating && self.receiver.is_some()
    }
    pub(super) fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.preview = None;
        self.confirmed = false;
        self.undo = None;
        self.undo_confirmed = false;
    }
    fn spawn(
        &mut self,
        ctx: &egui::Context,
        mutating: bool,
        work: impl FnOnce() -> ReviewResult<Message> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        self.receiver = Some(rx);
        self.mutating = mutating;
        self.failure = None;
        if mutating {
            self.outcome=Some("Operation running; completion is unconfirmed. If interrupted, inspect retained History before retrying.".into());
        }
        std::thread::spawn(move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        });
    }
    fn poll(&mut self) {
        let result = match self.receiver.as_ref().map(|r| r.try_recv()) {
            Some(Ok(r)) => r,
            Some(Err(mpsc::TryRecvError::Disconnected)) => Err(
                "Worker interrupted; outcome is uncertain. Inspect History & Logs before retrying"
                    .into(),
            ),
            _ => return,
        };
        self.receiver = None;
        self.mutating = false;
        match result {
            Err(e) => {
                self.failure = Some(e);
                self.undo = None;
                self.undo_confirmed = false;
                self.preview = None;
                self.confirmed = false;
            }
            Ok(Message::Preview(g, p)) if g == self.generation => {
                self.preview = Some(p);
                self.confirmed = false;
                self.outcome = Some("Preview only: no installed files changed".into());
            }
            Ok(Message::UndoPreview(g, p, b)) if g == self.generation => {
                self.undo = Some((p, b));
                self.undo_confirmed = false;
            }
            Ok(Message::Preview(..) | Message::UndoPreview(..)) => {
                self.failure =
                    Some("Selection changed while Preview was running; Preview again".into());
            }
            Ok(Message::Applied(r)) => {
                self.outcome = Some(format!(
                    "Install status: {:?}. Journal: {:?}. In-game effectiveness has not been tested.",
                    r.journal.status, r.journal_path
                ));
                if r.journal.status != SharedApplyStatus::Success || r.journal_failure.is_some() {
                    self.failure = Some(format!(
                        "Incomplete or uncertain installation: {:?}; durable-record failure {:?}. Inspect History & Logs and recovery guidance.",
                        r.journal.entries, r.journal_failure
                    ));
                }
                if let Some(p) = r.journal_path.as_ref() {
                    self.history
                        .push((SharedTransactionPath::from_path(p), r.journal.clone()));
                }
                self.preview = None;
                self.confirmed = false;
            }
            Ok(Message::History(h)) => self.history = h,
            Ok(Message::SelectionSaved) => {
                self.invalidate();
                self.outcome=Some("Flycast selection saved through the shared resolver. Preview again; selected Blue Launch must agree.".into());
            }
            Ok(Message::Undone(r)) => {
                self.outcome = Some(format!(
                    "Undo outcomes: {:?}; warnings: {:?}. Inspect History for retained evidence.",
                    r.preview.entries, r.status
                ));
                self.undo = None;
                self.undo_confirmed = false;
                self.preview = None;
                self.confirmed = false;
            }
        }
    }
    pub(super) fn preview(
        &mut self,
        ctx: &egui::Context,
        launch: ReviewResult<LaunchView>,
        game: PathBuf,
        product: String,
        source: PathBuf,
        selections: Vec<(usize, bool)>,
    ) {
        if self.receiver.is_some() {
            return;
        }
        self.invalidate();
        let g = self.generation;
        self.spawn(ctx, false, move || {
            let env = environment(&launch?)?;
            fresh_identity(&game, &product)?;
            let (history, _) = history_roots()?;
            refuse_unfinished(&history, &game)?;
            let p = preview_flycast_install(&env, &game, &product, &source, &selections)?;
            Ok(Message::Preview(g, Box::new(p)))
        });
    }
}
fn environment(launch: &LaunchView) -> ReviewResult<FlycastInstallEnvironment> {
    #[cfg(test)]
    {
        let _ = launch;
        Err("Tests must supply disposable Flycast environments explicitly".into())
    }
    #[cfg(not(test))]
    {
        Ok(FlycastInstallEnvironment {
            roots: FlycastProfileDiscoveryRoots::from_environment().map_err(|e| e.to_string())?,
            selection_path:
                archivefs_core::emulator_profile_resolver::persist::default_selection_path()
                    .map_err(|e| e.to_string())?,
            launch_profile_id: launch.profile_id.clone(),
            launch_profile_root: launch
                .profile_path
                .clone()
                .ok_or("Launch profile path is unknown")?,
        })
    }
}
fn fresh_identity(game: &Path, product: &str) -> ReviewResult<()> {
    let id =
        archivefs_core::game_identity::inspect_catalogued_game_identity(game, Some("Dreamcast"));
    if verified_product(Some(&id), game)? != product {
        return Err("Game identity changed since Preview".into());
    }
    Ok(())
}
fn history_roots() -> ReviewResult<(PathBuf, PathBuf)> {
    super::super::duckstation_environment::shared_history_roots()
        .ok_or("History roots unavailable".into())
}
fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn undo_binding(
    env: &FlycastInstallEnvironment,
    path: &Path,
    game: &Path,
    product: &str,
    backups: &Path,
) -> ReviewResult<(SharedRollbackPreview, FlycastInstallBinding)> {
    let binding = env.resolve()?;
    let report = discover_shared_apply_history(path.parent().ok_or("History path has no parent")?);
    if !report.complete || !report.warnings.is_empty() {
        return Err("History incomplete; inspect retained recovery evidence".into());
    }
    let journal = report
        .journals
        .iter()
        .find(|(p, _)| p.to_path_buf().ok().as_deref() == Some(path))
        .map(|(_, j)| j)
        .ok_or("Install journal unavailable")?;
    if journal.context.adapter != PreviewAdapter::Flycast
        || journal.context.profile_id != env.launch_profile_id
        || journal
            .context
            .selected_archive
            .to_path_buf()
            .ok()
            .as_deref()
            != Some(game)
        || journal.context.verified_game_identity != product
        || journal.destination_root.to_path_buf().ok().as_deref()
            != Some(binding.profile_root.as_path())
    {
        return Err(
            "History entry does not belong to the current game and selected installation".into(),
        );
    }
    Ok((
        preview_shared_rollback(path, &binding.profile_root, backups),
        binding,
    ))
}

pub(super) fn show(
    ui: &mut egui::Ui,
    state: &mut InstallState,
    launch: Option<ReviewResult<LaunchView>>,
    game: &Path,
    product: &ReviewResult<String>,
) {
    state.poll();
    let idle = state.receiver.is_none();
    let launch = launch.unwrap_or_else(|| Err("Resolve Blue Launch first".into()));
    ui.horizontal_wrapped(|ui| {
        use archivefs_core::emulator_inventory::InventoryEmulator;
        use archivefs_core::emulator_profile_resolver::{
            CandidateIdentity, EmulatorSelectionMode, remember_selection_to,
        };
        for (label, kind) in [
            ("Use AUTO", 0),
            ("Prefer current Launch pair", 1),
            ("Force current Launch pair", 2),
        ] {
            if ui.add_enabled(idle, egui::Button::new(label)).clicked() {
                let launch = launch.clone();
                state.invalidate();
                state.spawn(ui.ctx(), false, move || {
                    let env = environment(&launch?)?;
                    let mode = if kind == 0 {
                        EmulatorSelectionMode::Auto
                    } else {
                        let binding = env.resolve()?;
                        let id = CandidateIdentity {
                            emulator: InventoryEmulator::Flycast,
                            executable: Some(binding.executable),
                            profile_root: binding.profile_root,
                        };
                        if kind == 1 {
                            EmulatorSelectionMode::Preferred(id)
                        } else {
                            EmulatorSelectionMode::Forced(id)
                        }
                    };
                    remember_selection_to(&env.selection_path, InventoryEmulator::Flycast, &mode)
                        .map_err(|e| e.to_string())?;
                    Ok(Message::SelectionSaved)
                });
            }
        }
    });
    if let Some(p) = &state.preview {
        ui.heading("Exact installation preview");
        ui.label(format!(
            "Official Flycast v2.7 · {} · {}",
            p.binding.executable.display(),
            p.binding.selection_summary
        ));
        ui.label(format!(
            "Profile: {} · game mapping [cheats] {}",
            p.binding.profile_root.display(),
            p.product
        ));
        ui.label(format!(
            "Boot metadata: product {} · version {} · device/disc {} · SHA-256 {}",
            p.game_evidence.product,
            p.game_evidence.version,
            p.game_evidence.device_info,
            p.game_evidence.metadata_sha256
        ));
        for entry in &p.report.entries {
            ui.label(format!(
                "{:?}: {} · before SHA-256 {:?} · after SHA-256 {:?}",
                entry.proposed_action,
                entry
                    .destination_path
                    .as_deref()
                    .unwrap_or(Path::new("?"))
                    .display(),
                entry.existing_destination_digest,
                entry.source_digest
            ));
        }
        for (path, bytes) in &p.changes {
            ui.collapsing(format!("Exact output: {}", path.display()), |ui| {
                ui.monospace(String::from_utf8_lossy(bytes));
            });
        }
        ui.label("The mapping applies to Flycast's exact product key, shared by discs/revisions with that key. Confirm the source's disc/revision applicability yourself. Flycast may suppress cheats in Hardcore or online modes. Its Save action writes disabled states.");
        ui.add_enabled(idle,egui::Checkbox::new(&mut state.confirmed,"Confirm these exact cheat and configuration changes, including replacements and product-wide applicability"));
        if ui
            .add_enabled(
                idle && state.confirmed,
                egui::Button::new("Install confirmed Flycast cheats"),
            )
            .clicked()
        {
            let p = state.preview.take().unwrap();
            let launch = launch.clone();
            let game = game.to_path_buf();
            let product = product.clone();
            state.confirmed = false;
            state.spawn(ui.ctx(), true, move || {
                let env = environment(&launch?)?;
                let product = product?;
                fresh_identity(&game, &product)?;
                let (history, backups) = history_roots()?;
                let result = apply_flycast_install(
                    &env,
                    &p,
                    &game,
                    &product,
                    &FlycastCheatApplyOptions {
                        general_approved: true,
                        replacement_approved: true,
                        operation_id: generate_shared_operation_id(),
                        timestamp_unix_seconds: timestamp(),
                        history_root: history,
                        backup_root: backups,
                    },
                )?;
                Ok(Message::Applied(Box::new(result)))
            });
        }
    }
    if ui
        .add_enabled(idle, egui::Button::new("Refresh Flycast History"))
        .clicked()
    {
        let game = game.to_path_buf();
        state.spawn(ui.ctx(), false, move || {
            let (history, _) = history_roots()?;
            let report = discover_shared_apply_history(&history);
            if !report.complete || !report.warnings.is_empty() {
                return Err("History incomplete; inspect History & Logs recovery evidence".into());
            }
            Ok(Message::History(
                report
                    .journals
                    .into_iter()
                    .filter(|(_, j)| {
                        j.context.adapter == PreviewAdapter::Flycast
                            && j.context.selected_archive.to_path_buf().ok().as_deref()
                                == Some(game.as_path())
                    })
                    .collect(),
            ))
        });
    }
    let mut choose = None;
    for (path, j) in &state.history {
        ui.label(format!(
            "History {} · {:?} · {}. Recorded result; current files require an Undo preview.",
            j.operation_id, j.status, j.timestamp_unix_seconds
        ));
        if ui
            .add_enabled(
                idle,
                egui::Button::new(format!("Preview Undo {}", j.operation_id)),
            )
            .clicked()
        {
            choose = path.to_path_buf().ok();
        }
    }
    if let Some(path) = choose {
        let launch = launch.clone();
        let game = game.to_path_buf();
        let product = product.clone();
        let g = state.generation;
        state.spawn(ui.ctx(), false, move || {
            let env = environment(&launch?)?;
            let (_, backups) = history_roots()?;
            let (p, b) = undo_binding(&env, &path, &game, &product?, &backups)?;
            Ok(Message::UndoPreview(g, p, b))
        });
    }
    if let Some((p, _)) = &state.undo {
        ui.label(format!(
            "Guarded Undo: available={} · {:?}",
            p.available, p.entries
        ));
        ui.add_enabled(
            idle && p.available,
            egui::Checkbox::new(
                &mut state.undo_confirmed,
                "Confirm Undo of this exact recorded installation",
            ),
        );
        if ui
            .add_enabled(
                idle && p.available && state.undo_confirmed,
                egui::Button::new("Undo confirmed Flycast installation"),
            )
            .clicked()
        {
            let (p, b) = state.undo.take().unwrap();
            let launch = launch.clone();
            state.undo_confirmed = false;
            state.spawn(ui.ctx(), true, move || {
                let env = environment(&launch?)?;
                if env.resolve()? != b {
                    return Err(
                        "Selected installation or config changed after Undo Preview; Preview again"
                            .into(),
                    );
                }
                let (history, backups) = history_roots()?;
                Ok(Message::Undone(execute_shared_rollback(
                    &p,
                    &SharedRollbackOptions {
                        confirmation: SharedRollbackConfirmation {
                            preview_id: p.preview_id.clone(),
                            approved: true,
                        },
                        rollback_operation_id: generate_shared_operation_id(),
                        timestamp_unix_seconds: timestamp(),
                        history_root: history,
                        backup_root: backups,
                    },
                )))
            });
        }
    }
    if let Some(o) = &state.outcome {
        ui.label(o);
    }
    if let Some(e) = &state.failure {
        ui.colored_label(egui::Color32::YELLOW, e);
    }
    if !idle {
        ui.spinner();
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::emulator_profile_resolver::EmulatorSelectionMode;
    fn binding() -> FlycastInstallBinding {
        FlycastInstallBinding {
            executable: "/disposable/flycast".into(),
            profile_root: "/disposable/profile".into(),
            executable_sha256: "a".repeat(64),
            config_sha256: "b".repeat(64),
            selection: EmulatorSelectionMode::Auto,
            selection_summary: "AUTO".into(),
            selection_sha256: Some("c".repeat(64)),
        }
    }
    fn undo() -> SharedRollbackPreview {
        SharedRollbackPreview {
            schema_version: 1,
            preview_id: "synthetic".into(),
            journal_path: SharedTransactionPath::from_path(Path::new("/disposable/history.json")),
            original_operation_id: "synthetic".into(),
            destination_root: SharedTransactionPath::from_path(Path::new("/disposable/profile")),
            entries: vec![],
            available: true,
        }
    }
    fn deliver(s: &mut InstallState, m: ReviewResult<Message>) {
        let (tx, rx) = mpsc::channel();
        tx.send(m).unwrap();
        s.receiver = Some(rx);
        s.poll();
    }
    #[test]
    fn stale_worker_preview_does_not_restore_old_approval() {
        let mut s = InstallState::default();
        let g = s.generation;
        s.invalidate();
        deliver(&mut s, Ok(Message::UndoPreview(g, undo(), binding())));
        assert!(s.undo.is_none());
        assert!(!s.undo_confirmed);
        assert!(s.failure.as_ref().unwrap().contains("Selection changed"));
    }
    #[test]
    fn current_undo_preview_requires_separate_confirmation() {
        let mut s = InstallState::default();
        let g = s.generation;
        deliver(&mut s, Ok(Message::UndoPreview(g, undo(), binding())));
        assert!(s.undo.is_some());
        assert!(!s.undo_confirmed);
        s.undo_confirmed = true;
        s.invalidate();
        assert!(s.undo.is_none());
        assert!(!s.undo_confirmed);
    }
    #[test]
    fn worker_failure_and_disconnect_are_truthful_and_clear_confirmation() {
        let mut s = InstallState::default();
        s.confirmed = true;
        s.undo = Some((undo(), binding()));
        s.undo_confirmed = true;
        s.mutating = true;
        deliver(&mut s, Err("incomplete operation; inspect History".into()));
        assert!(!s.busy());
        assert!(!s.confirmed);
        assert!(!s.undo_confirmed);
        assert!(s.undo.is_none());
        assert!(s.failure.as_ref().unwrap().contains("History"));
        let (tx, rx) = mpsc::channel();
        drop(tx);
        s.receiver = Some(rx);
        s.mutating = true;
        s.poll();
        assert!(s.failure.as_ref().unwrap().contains("uncertain"));
        assert!(!s.busy());
    }
    #[test]
    fn installing_blocks_workspace_replacement_and_initial_render_has_no_io() {
        let mut app = crate::tests::app_with_cheats_mods_context();
        let w = app.cheat_workflow.as_mut().unwrap();
        let original = w.archive_path.clone();
        let (tx, rx) = mpsc::channel();
        w.flycast.install.receiver = Some(rx);
        w.flycast.install.mutating = true;
        assert!(!app.prepare_cheats_mods_workspace("/different/game.gdi".into()));
        assert_eq!(app.cheat_workflow.as_ref().unwrap().archive_path, original);
        drop(tx);
    }
    #[test]
    fn selection_change_invalidates_both_preview_and_undo_approval() {
        let mut s = InstallState::default();
        s.confirmed = true;
        s.undo_confirmed = true;
        deliver(&mut s, Ok(Message::SelectionSaved));
        assert!(!s.confirmed);
        assert!(!s.undo_confirmed);
        assert!(s.outcome.unwrap().contains("shared resolver"));
    }
    #[test]
    fn test_environment_cannot_fall_back_to_user_profile() {
        assert!(
            environment(&LaunchView {
                profile_id: "x".into(),
                profile_path: Some("/user/profile".into())
            })
            .is_err()
        );
        assert!(history_roots().is_err());
    }
    #[test]
    fn starting_source_picker_revokes_approval_before_dialog_returns() {
        let mut state = FlycastPageState::default();
        state.reviewed = true;
        state.install.confirmed = true;
        state.install.undo = Some((undo(), binding()));
        state.install.undo_confirmed = true;
        state.prepare_source_pick();
        assert!(!state.reviewed);
        assert!(!state.install.confirmed);
        assert!(!state.install.undo_confirmed);
        assert!(state.install.undo.is_none());
    }
}
