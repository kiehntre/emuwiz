//! PPSSPP's proven local CWCheat slice, using shared transactions and History.
use crate::*;
use archivefs_core::patch_manager::{self as pm, ppsspp_install as install};
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
};
type Checked<T> = std::result::Result<T, String>;
pub(crate) fn is_route(w: &CheatWorkflowState) -> bool {
    matches!(&w.routing.decision, pm::CheatRouteDecision::Routed(r) if matches!(&r.target, pm::CheatRouteTarget::Standalone{adapter_id} if adapter_id=="ppsspp"))
}
enum Message {
    Profiles(Vec<pm::PpssppProfile>),
    Preview(Box<install::Preview>),
    Applied(pm::SharedApplyResult),
    History(Vec<(pm::SharedTransactionPath, pm::SharedApplyJournal)>),
    Undo(pm::SharedRollbackPreview, install::Binding),
    Undone(pm::SharedRollbackResult),
}
#[derive(Default)]
pub(crate) struct State {
    key: Option<(PathBuf, Option<GameIdentityReport>)>,
    executable: Option<PathBuf>,
    source: Option<PathBuf>,
    profiles: Vec<pm::PpssppProfile>,
    profile: Option<String>,
    receiver: Option<Receiver<(u64, Checked<Message>)>>,
    generation: u64,
    mutating: bool,
    preview: Option<Arc<install::Preview>>,
    confirmed: bool,
    text_page: usize,
    history_page: usize,
    history: Vec<(pm::SharedTransactionPath, pm::SharedApplyJournal)>,
    undo: Option<(pm::SharedRollbackPreview, install::Binding)>,
    undo_confirmed: bool,
    notice: Option<String>,
    error: Option<String>,
}
impl State {
    pub(crate) fn busy(&self) -> bool {
        self.mutating && self.receiver.is_some()
    }
    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.preview = None;
        self.confirmed = false;
        self.text_page = 0;
        self.undo = None;
        self.undo_confirmed = false;
    }
    fn start(
        &mut self,
        ctx: &egui::Context,
        mutating: bool,
        f: impl FnOnce() -> Checked<Message> + Send + 'static,
    ) {
        if self.receiver.is_some() {
            return;
        }
        let generation = self.generation;
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        self.receiver = Some(rx);
        self.mutating = mutating;
        self.error = None;
        if mutating {
            self.notice=Some("Operation running. Completion is unconfirmed; inspect History after interruption before retrying.".into());
        }
        std::thread::spawn(move || {
            let _ = tx.send((generation, f()));
            ctx.request_repaint();
        });
    }
    fn poll(&mut self) {
        let value = match self.receiver.as_ref().map(|r| r.try_recv()) {
            Some(Ok(v)) => v,
            Some(Err(mpsc::TryRecvError::Disconnected)) => (
                self.generation,
                Err(
                    "Worker stopped; inspect History before retrying. No success is assumed."
                        .into(),
                ),
            ),
            _ => return,
        };
        self.receiver = None;
        self.mutating = false;
        if value.0 != self.generation {
            return;
        }
        match value.1 {
            Err(e) => {
                self.notice = None;
                self.invalidate();
                self.error = Some(e);
            }
            Ok(Message::Profiles(p)) => self.profiles = p,
            Ok(Message::Preview(p)) => {
                self.preview = Some(Arc::from(p));
                self.confirmed = false;
                self.notice = Some("Preview only; nothing installed.".into());
            }
            Ok(Message::Applied(r)) => {
                self.notice = Some(format!(
                    "Install: {:?}; receipt: {:?}. Entries are disabled. Global settings unchanged; effectiveness untested.",
                    r.journal.status, r.journal_path
                ));
                if r.journal.status != pm::SharedApplyStatus::Success || r.journal_failure.is_some()
                {
                    self.error = Some(format!(
                        "Installation incomplete or uncertain: {:?}; journal failure {:?}. Review History & Logs.",
                        r.journal.entries, r.journal_failure
                    ));
                }
                if let Some(path) = r.journal_path {
                    self.history
                        .push((pm::SharedTransactionPath::from_path(&path), r.journal));
                }
                self.invalidate();
            }
            Ok(Message::History(h)) => self.history = h,
            Ok(Message::Undo(p, b)) => {
                self.undo = Some((p, b));
                self.undo_confirmed = false;
            }
            Ok(Message::Undone(r)) => {
                self.notice = Some(format!(
                    "Undo: {:?}. Outcomes: {:?}",
                    r.status, r.preview.entries
                ));
                self.invalidate();
            }
        }
    }
    fn environment(&self) -> Checked<install::Environment> {
        Ok(install::Environment {
            roots: pm::PpssppProfileDiscoveryRoots::from_environment()
                .map_err(|e| e.to_string())?,
            executable: self
                .executable
                .clone()
                .ok_or("Choose the supported AppImage")?,
            profile_id: self
                .profile
                .clone()
                .ok_or("Explicitly choose a PPSSPP profile")?,
            proc_root: PathBuf::from("/proc"),
        })
    }
}
fn history_roots() -> Checked<(PathBuf, PathBuf)> {
    super::duckstation_environment::shared_history_roots()
        .ok_or("Shared History roots unavailable".into())
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    state: &mut State,
    game: &Path,
    identity: Option<&GameIdentityReport>,
) {
    state.poll();
    let key = (game.to_path_buf(), identity.cloned());
    if state.key.as_ref() != Some(&key) {
        if state.busy() {
            ui.label("An operation for the previous selection is still running. Inspect History before continuing.");
            return;
        }
        state.invalidate();
        state.key = Some(key);
        state.history.clear();
        state.notice = None;
        state.error = None;
    }
    ui.heading("PPSSPP cheats");
    ui.label("Install disabled CWCheat Write32 entries for this game's exact Disc ID. Existing cheat files are preserved; this slice creates new files only.");
    ui.small("Supported: official PPSSPP 1.19.3 Linux x86-64 AppImage, default XDG profile, bounded game files up to 512 MiB. Custom/Flatpak/portable overrides are refused. Installation does not enable cheats or prove in-game effectiveness.");
    if let Some(message) = &state.notice {
        ui.label(message);
    }
    if let Some(error) = &state.error {
        ui.colored_label(egui::Color32::YELLOW, error);
    }
    let id = match identity
        .ok_or_else(|| "Inspect PSP identity first".to_string())
        .and_then(|r| install::disc_id(r, game))
    {
        Ok(id) => id,
        Err(e) => {
            state.preview = None;
            state.confirmed = false;
            ui.label(e);
            return;
        }
    };
    ui.label(format!("Verified PSP Disc ID: {id}"));
    if state.receiver.is_some() {
        ui.spinner();
        ui.label("Checking the selected profile and retained evidence…");
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
        return;
    }
    ui.horizontal_wrapped(|ui| {
        if ui.button("Choose supported PPSSPP AppImage").clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("AppImage", &["AppImage"])
                .pick_file()
            {
                state.invalidate();
                state.executable = Some(path.clone());
                state.profile = None;
                state.profiles.clear();
                state.history.clear();
                state.start(ui.ctx(), false, move || {
                    let mut roots = pm::PpssppProfileDiscoveryRoots::from_environment()
                        .map_err(|e| e.to_string())?;
                    roots.explicit_executables = vec![path];
                    roots.path_override = Some(vec![]);
                    let profiles = pm::discover_ppsspp_profiles(&roots)
                        .profiles
                        .into_iter()
                        .filter(|p| {
                            p.configuration_path == roots.xdg_config_home.join("ppsspp")
                                && p.installation_type == pm::PpssppInstallationType::Native
                                && p.eligible
                        })
                        .collect();
                    Ok(Message::Profiles(profiles))
                });
            }
        }
        if let Some(exe) = &state.executable {
            ui.label(exe.display().to_string());
        }
    });
    let previous = state.profile.clone();
    egui::ComboBox::from_id_salt("ppsspp_cheat_profile")
        .selected_text(
            state
                .profile
                .as_deref()
                .unwrap_or("Choose existing PPSSPP profile"),
        )
        .show_ui(ui, |ui| {
            for profile in &state.profiles {
                ui.selectable_value(
                    &mut state.profile,
                    Some(profile.profile_id.clone()),
                    profile.configuration_path.display().to_string(),
                );
            }
        });
    if previous != state.profile {
        state.invalidate();
        state.history.clear();
    }
    if state.executable.is_some() && state.profiles.is_empty() {
        ui.label("No eligible default XDG profile was found. Configure this supported build in PPSSPP first; no settings are created here.");
    }
    if ui.button("Choose local CWCheat INI").clicked() {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("CWCheat INI", &["ini"])
            .pick_file()
        {
            state.invalidate();
            state.source = Some(path);
        }
    }
    if let Some(source) = &state.source {
        ui.label(format!("Source: {}", source.display()));
    }
    let environment = state.environment();
    if ui
        .add_enabled(
            environment.is_ok() && state.source.is_some(),
            egui::Button::new("Preview PPSSPP install"),
        )
        .clicked()
    {
        let env = environment.clone().unwrap();
        let game = game.to_path_buf();
        let source = state.source.clone().unwrap();
        let id = id.clone();
        state.invalidate();
        state.start(ui.ctx(), false, move || {
            Ok(Message::Preview(Box::new(install::preview(
                &env, &game, &id, &source,
            )?)))
        });
    }
    if let Some(p) = state.preview.clone() {
        ui.label(format!(
            "New file: {}.ini | {} disabled entries | {}",
            p.id,
            p.cheat_count,
            p.binding.cheats_root.display()
        ));
        ui.collapsing("Review exact installed text", |ui| {
            egui::ScrollArea::vertical()
                .max_height(180.0)
                .show(ui, |ui| {
                    let count = p.native.staged.contents.lines().count();
                    state.text_page = state.text_page.min(count.saturating_sub(1) / 50);
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(
                                state.text_page > 0,
                                egui::Button::new("Previous code page"),
                            )
                            .clicked()
                        {
                            state.text_page -= 1;
                        }
                        if ui
                            .add_enabled(
                                (state.text_page + 1) * 50 < count,
                                egui::Button::new("Next code page"),
                            )
                            .clicked()
                        {
                            state.text_page += 1;
                        }
                        ui.label(format!("Code page {}", state.text_page + 1));
                    });
                    for line in p
                        .native
                        .staged
                        .contents
                        .lines()
                        .skip(state.text_page * 50)
                        .take(50)
                    {
                        ui.monospace(line);
                    }
                });
        });
        ui.checkbox(
            &mut state.confirmed,
            "I reviewed this exact disabled cheat file and destination.",
        );
        if ui
            .add_enabled(
                state.confirmed && environment.is_ok(),
                egui::Button::new("Confirm and install disabled cheats"),
            )
            .clicked()
        {
            state.confirmed = false;
            let env = environment.clone().unwrap();
            state.start(ui.ctx(), true, move || {
                let (history_root, backup_root) = history_roots()?;
                let result = install::apply(
                    &env,
                    &p,
                    &pm::SharedApplyOptions {
                        dry_run: false,
                        confirmation: Some(pm::SharedApplyConfirmation {
                            plan_id: p.native.transaction.plan_id.clone(),
                            general_approved: true,
                            replacement_approved: false,
                        }),
                        operation_id: pm::generate_shared_operation_id(),
                        timestamp_unix_seconds: now(),
                        current_context: p.native.transaction.context.clone(),
                        history_root,
                        backup_root,
                    },
                )?;
                Ok(Message::Applied(result))
            });
        }
    }
    ui.separator();
    ui.strong("PPSSPP install History and guarded Undo");
    if ui
        .add_enabled(
            environment.is_ok(),
            egui::Button::new("Refresh PPSSPP install History"),
        )
        .clicked()
    {
        let env = environment.clone().unwrap();
        let game = game.to_path_buf();
        let id = id.clone();
        state.start(ui.ctx(), false, move || {
            let (history, _) = history_roots()?;
            Ok(Message::History(install::history(
                &env, &game, &id, &history,
            )?))
        });
    }
    let mut requested = None;
    state.history_page = state
        .history_page
        .min(state.history.len().saturating_sub(1) / 25);
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                state.history_page > 0,
                egui::Button::new("Previous receipts"),
            )
            .clicked()
        {
            state.history_page -= 1;
        }
        if ui
            .add_enabled(
                (state.history_page + 1) * 25 < state.history.len(),
                egui::Button::new("Next receipts"),
            )
            .clicked()
        {
            state.history_page += 1;
        }
        ui.label(format!("{} retained receipts", state.history.len()));
    });
    for (path, journal) in state
        .history
        .iter()
        .rev()
        .skip(state.history_page * 25)
        .take(25)
    {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("{} · {:?}", journal.operation_id, journal.status));
            if ui.button("Preview guarded Undo").clicked() {
                requested = path.to_path_buf().ok();
            }
        });
    }
    if let Some(path) = requested {
        if let Ok(env) = environment.clone() {
            let game = game.to_path_buf();
            let id = id.clone();
            state.undo = None;
            state.undo_confirmed = false;
            state.start(ui.ctx(), false, move || {
                let (h, b) = history_roots()?;
                let (p, binding) = install::undo_preview(&env, &game, &id, &path, &h, &b)?;
                Ok(Message::Undo(p, binding))
            });
        }
    }
    if let Some((p, b)) = state.undo.clone() {
        ui.label(format!(
            "Undo affects {} reviewed file(s); edited files are refused.",
            p.entries.len()
        ));
        ui.label(format!("Receipt: {:?}", p.journal_path));
        for entry in &p.entries {
            ui.label(format!(
                "File: {:?} | reviewed action: {:?}",
                entry.destination, entry.outcome
            ));
            if let Some(failure) = &entry.failure {
                ui.label(format!("Refusal: {failure:?}"));
            }
        }
        ui.checkbox(
            &mut state.undo_confirmed,
            "I reviewed this exact Undo preview.",
        );
        if ui
            .add_enabled(
                state.undo_confirmed && p.available && environment.is_ok(),
                egui::Button::new("Confirm PPSSPP Undo"),
            )
            .clicked()
        {
            let env = environment.unwrap();
            state.undo_confirmed = false;
            state.start(ui.ctx(), true, move || {
                let (history_root, backup_root) = history_roots()?;
                let r = install::undo(
                    &env,
                    &p,
                    &b,
                    &pm::SharedRollbackOptions {
                        confirmation: pm::SharedRollbackConfirmation {
                            preview_id: p.preview_id.clone(),
                            approved: true,
                        },
                        rollback_operation_id: pm::generate_shared_operation_id(),
                        timestamp_unix_seconds: now(),
                        history_root,
                        backup_root,
                    },
                )?;
                Ok(Message::Undone(r))
            });
        }
    }
}
#[cfg(test)]
mod tests;
