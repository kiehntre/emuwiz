//! Standalone native melonDS local-file journey; no emulator/config selection writes.
use crate::*;
use archivefs_core::patch_manager::*;
use std::path::{Path, PathBuf};
use std::result::Result;
use std::sync::mpsc::{self, Receiver};

#[derive(Clone)]
struct Environment {
    roots: MelonDsProfileDiscoveryRoots,
    memory: Option<PathBuf>,
    history: PathBuf,
    backups: PathBuf,
}
impl Environment {
    fn remembered(&self) -> Result<Option<String>, String> {
        let profiles = match &self.memory {
            Some(p) => load_remembered_emulator_profiles_from(p).map_err(|e| e.to_string())?,
            None => vec![],
        };
        let matching: Vec<_> = profiles.iter().filter(|p| p.adapter == "melonds").collect();
        match matching.as_slice() {
            [] => Ok(None),
            [one] => Ok(Some(one.profile_id.clone())),
            _ => Err("Conflicting remembered melonDS profiles; review Emulator setup".into()),
        }
    }
}
fn environment() -> Result<Environment, String> {
    #[cfg(test)]
    {
        Err("Tests must supply disposable melonDS profiles explicitly".into())
    }
    #[cfg(not(test))]
    {
        Ok(Environment {
            roots: MelonDsProfileDiscoveryRoots::from_environment().map_err(|e| e.to_string())?,
            memory: Some(default_emulator_profile_memory_path().map_err(|e| e.to_string())?),
            history: default_shared_history_root().map_err(|e| format!("{e:?}"))?,
            backups: default_shared_backup_root().map_err(|e| format!("{e:?}"))?,
        })
    }
}
fn pick_source() -> Result<PathBuf, String> {
    #[cfg(test)]
    {
        Err("Tests must supply disposable local sources explicitly".into())
    }
    #[cfg(not(test))]
    {
        rfd::FileDialog::new()
            .add_filter("melonDS MCH", &["mch"])
            .pick_file()
            .ok_or("No file selected".into())
    }
}
struct Loaded {
    source: PathBuf,
    source_sha256: String,
    file: MelonDsInstallSource,
    profile: MelonDsProfile,
}
enum Message {
    Loaded(Loaded),
    Preview(Box<MelonDsInstallPreview>),
    Applied(Box<SharedApplyResult>),
    History(
        Vec<(SharedTransactionPath, SharedApplyJournal)>,
        Vec<String>,
    ),
    UndoPreview(SharedRollbackPreview),
    Undone(SharedRollbackResult),
}
#[derive(Default)]
pub(crate) struct MelonDsPageState {
    receiver: Option<Receiver<Result<Message, String>>>,
    mutating: bool,
    loaded: Option<Loaded>,
    selected: Vec<bool>,
    enabled: Vec<bool>,
    expected: String,
    preview: Option<Box<MelonDsInstallPreview>>,
    confirmed: bool,
    result: Option<Box<SharedApplyResult>>,
    history: Vec<(SharedTransactionPath, SharedApplyJournal)>,
    undo: Option<SharedRollbackPreview>,
    undo_confirmed: bool,
    outcome: Option<String>,
    failure: Option<String>,
    identity_key: Option<String>,
}
impl MelonDsPageState {
    pub(crate) fn busy(&self) -> bool {
        self.mutating && self.receiver.is_some()
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
        if mutating {
            self.outcome = Some("Operation running; completion is not confirmed. Retained History is authoritative if interrupted.".into());
        }
        std::thread::spawn(move || {
            let result = work();
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    fn poll(&mut self) {
        let result = match self.receiver.as_ref().map(|r| r.try_recv()) {
            Some(Ok(r)) => r,
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                Err("Worker stopped unexpectedly; inspect History before retrying".into())
            }
            _ => return,
        };
        self.receiver = None;
        self.mutating = false;
        self.confirmed = false;
        match result {
            Err(e) => {
                self.failure = Some(e);
                self.outcome = None;
            }
            Ok(Message::Loaded(l)) => {
                self.selected = vec![false; l.file.entries.len()];
                self.enabled = vec![false; l.file.entries.len()];
                self.loaded = Some(l);
                self.preview = None;
                self.outcome = None;
            }
            Ok(Message::Preview(p)) => {
                self.preview = Some(p);
                self.result = None;
                self.outcome = Some("Preview only — no emulator or game files changed".into());
            }
            Ok(Message::Applied(r)) => {
                self.outcome = Some(format!(
                    "Install result: {:?}. Runtime cheat effectiveness remains unverified. Journal: {:?}",
                    r.journal.status, r.journal_path
                ));
                if r.journal.status != SharedApplyStatus::Success {
                    self.failure = Some(format!(
                        "Install incomplete or refused: {:?}; journal evidence: {:?}. Refresh History and inspect retained recovery guidance",
                        r.journal.entries, r.journal_failure
                    ));
                }
                self.result = Some(r);
                self.preview = None;
            }
            Ok(Message::History(rows, warnings)) => {
                self.history = rows;
                if !warnings.is_empty() {
                    self.failure = Some(warnings.join("\n"));
                }
            }
            Ok(Message::UndoPreview(p)) => {
                self.undo = Some(p);
                self.undo_confirmed = false;
            }
            Ok(Message::Undone(r)) => {
                self.outcome = Some(format!(
                    "Undo result: {:?}; receipt: {:?}",
                    r.status, r.journal_path
                ));
                if r.status != SharedApplyStatus::Success {
                    self.failure = Some(
                        "Undo incomplete/refused. Recovery evidence retained; inspect History."
                            .into(),
                    );
                }
                self.undo = None;
                self.undo_confirmed = false;
            }
        }
    }
}
pub(crate) fn is_melonds_route(w: &CheatWorkflowState) -> bool {
    matches!(&w.routing.decision, CheatRouteDecision::Routed(r) if matches!(&r.target, CheatRouteTarget::Standalone{adapter_id} if adapter_id == "melonds"))
}
fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn apply_preview(
    p: &MelonDsInstallPreview,
    env: &Environment,
) -> Result<SharedApplyResult, String> {
    let remembered = env.remembered()?;
    let plan = p.plan();
    apply_melonds_install(
        p,
        &env.roots,
        remembered.as_deref(),
        &SharedApplyOptions {
            dry_run: false,
            confirmation: Some(SharedApplyConfirmation {
                plan_id: plan.plan_id.clone(),
                general_approved: true,
                replacement_approved: false,
            }),
            operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: timestamp(),
            current_context: plan.context.clone(),
            history_root: env.history.clone(),
            backup_root: env.backups.clone(),
        },
    )
}
pub(crate) fn show(ui: &mut egui::Ui, state: &mut MelonDsPageState, rom: &Path) {
    use crate::widgets;
    state.poll();
    let identity_hash: Result<String, String> = if state.expected.trim().len() == 64
        && state.expected.trim().bytes().all(|b| b.is_ascii_hexdigit())
    {
        Ok(state.expected.trim().to_ascii_lowercase())
    } else {
        Err("Supply the exact selected ROM SHA-256 declared by your local cheat source; Preview computes and compares the actual file hash".into())
    };
    let key = identity_hash.as_ref().ok().cloned();
    if key != state.identity_key {
        state.confirmed = false;
        if !state.busy() {
            state.preview = None;
        }
        state.identity_key = key;
    }
    widgets::section_header(
        ui,
        "Standalone melonDS cheats",
        Some(
            "Local native .mch files only. No downloads, ROM/config writes, or Launch selection changes.",
        ),
    );
    ui.label("Backend: native format inspection available. Preview: compares your source declaration to the selected DS file SHA-256. Installation: requires the supported subset and an exact confirmed Preview. In-game effectiveness: unverified.");
    ui.label("Only one native entry with aligned DS main-RAM direct writes is installable per new MCH file. Conditional/opaque operations, multi-entry installs, existing destinations and guessed paths are unavailable. usrcheat.dat import stays in melonDS; EmuWiz does not install it.");
    ui.label("Choose the local MCH source from a dedicated folder, such as Downloads, that does not contain EmuWiz's History or backups.");
    ui.label("This does not change Launch selection. Installation requires the official melonDS 1.1 Ubuntu x86-64 binary and ordinary XDG melonDS.toml profile, using existing AUTO/remembered profile selection. Unknown builds, AppImage, Flatpak, portable, DSi and multi-instance profiles are unavailable.");
    if let Err(e) = &identity_hash {
        ui.colored_label(egui::Color32::YELLOW, e);
    }
    let idle = state.receiver.is_none();
    if ui
        .add_enabled(idle, egui::Button::new("Choose local melonDS .mch file"))
        .clicked()
    {
        state.spawn(ui.ctx(), false, || {
            let source = pick_source()?;
            let env = environment()?;
            let remembered = env.remembered()?;
            let profile = select_melonds_cheat_profile(&env.roots, remembered.as_deref())?;
            let (file, source_sha256) = inspect_melonds_local_cheats(&source)?;
            Ok(Message::Loaded(Loaded {
                source,
                source_sha256,
                file,
                profile,
            }))
        });
    }
    let mut edit = false;
    ui.add_enabled_ui(idle, |ui| {
        ui.label("Expected ROM SHA-256 from the cheat source (required; filenames/titles are not evidence)");
        edit |= ui.text_edit_singleline(&mut state.expected).changed();
    });
    if let Some(l) = &state.loaded {
        ui.label(format!(
            "Profile: {} · {}",
            l.profile.profile_id,
            l.profile.configuration_path.display()
        ));
        ui.label(format!("Local source: {}", l.source.display()));
        ui.label("Preview verifies the installed binary against the official 1.1 release SHA-256 and checks Instance0.CheatFilePath. It never executes the emulator. In-game effectiveness remains untested.");
        ui.label(format!(
            "Parser issues (never silently discarded): {:?}",
            l.file.issues
        ));
        for (i, e) in l.file.entries.iter().enumerate() {
            let supported = melonds_entry_installable(e);
            ui.horizontal_wrapped(|ui| {
                edit |= ui
                    .add_enabled(
                        idle && supported,
                        egui::Checkbox::new(&mut state.selected[i], &e.name),
                    )
                    .changed();
                edit |= ui
                    .add_enabled(
                        idle && supported && state.selected[i],
                        egui::Checkbox::new(&mut state.enabled[i], "Enable in melonDS"),
                    )
                    .changed();
                ui.label(format!(
                    "Source {:?} · {}",
                    e.state,
                    if supported {
                        "supported RAM-write subset"
                    } else {
                        "unavailable / opaque or unsupported"
                    }
                ));
            });
            ui.small(
                e.code
                    .words
                    .iter()
                    .map(|(a, b)| format!("{a:08X} {b:08X}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
    }
    if edit {
        state.preview = None;
        state.confirmed = false;
    }
    let ready = idle
        && state.loaded.is_some()
        && state.selected.iter().filter(|s| **s).count() == 1
        && identity_hash
            .as_ref()
            .is_ok_and(|h| h.eq_ignore_ascii_case(state.expected.trim()));
    if ui
        .add_enabled(ready, egui::Button::new("Preview exact installation"))
        .clicked()
    {
        let l = state.loaded.as_ref().unwrap();
        let request = MelonDsInstallRequest {
            rom: rom.into(),
            expected_rom_sha256: state.expected.trim().into(),
            source: l.source.clone(),
            expected_source_sha256: l.source_sha256.clone(),
            selected: state
                .selected
                .iter()
                .enumerate()
                .filter(|(_, s)| **s)
                .map(|(i, _)| (i, state.enabled[i]))
                .collect(),
            profile_id: l.profile.profile_id.clone(),
        };
        state.spawn(ui.ctx(), false, move || {
            let env = environment()?;
            let remembered = env.remembered()?;
            preview_melonds_install(&request, &env.roots, remembered.as_deref())
                .map(|p| Message::Preview(Box::new(p)))
        });
    }
    if let Some(p) = &state.preview {
        ui.separator();
        ui.heading("Preview — not installed");
        ui.label(format!("Executable: {}", p.executable().display()));
        ui.label(format!("New file: {}", p.destination.display()));
        ui.label(format!(
            "Selected entries: {} · Cheats enabled in melonDS: {} (setting unchanged)",
            p.file.entries.len(),
            match p.autoload {
                Some(true) => "On",
                Some(false) => "Off",
                None => "Not recorded",
            }
        ));
        for e in &p.file.entries {
            ui.label(format!(
                "{} · {:?} · {}",
                e.name,
                e.state,
                e.code
                    .words
                    .iter()
                    .map(|(a, b)| format!("{a:08X} {b:08X}"))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        ui.label("melonDS loads the new MCH when you reload this ROM. If cheats are off, turn them on in melonDS when ready; each code also needs its reviewed enabled state. EmuWiz leaves that setting unchanged. Installation does not prove in-game effectiveness.");
        ui.label("Existing destination: must remain absent. No merging or replacement. A SHA-256 match binds this locally supplied declaration to exact bytes; it does not authenticate the cheat author or prove in-game suitability.");
        ui.add_enabled(idle,egui::Checkbox::new(&mut state.confirmed,"I reviewed the source/ROM binding, selected enable states and exact new file; melonDS is closed. Install only this Preview."));
        if ui
            .add_enabled(
                idle && state.confirmed
                    && identity_hash
                        .as_ref()
                        .is_ok_and(|h| h.eq_ignore_ascii_case(&p.request().expected_rom_sha256)),
                egui::Button::new("Install confirmed melonDS cheats"),
            )
            .clicked()
        {
            let p = p.clone();
            state.spawn(ui.ctx(), true, move || {
                apply_preview(&p, &environment()?).map(|r| Message::Applied(Box::new(r)))
            });
        }
    }
    if let Some(message) = &state.outcome {
        ui.label(message);
    }
    if let Some(failure) = &state.failure {
        widgets::banner(
            ui,
            "melonDS operation needs attention",
            failure,
            widgets::StatusTone::Warning,
        );
    }
    if !idle {
        ui.spinner();
        ui.label("Working on local files; waiting for the recorded result…");
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
    ui.separator();
    ui.heading("melonDS installation History / Undo");
    if ui
        .add_enabled(
            idle,
            egui::Button::new("Refresh this ROM's install history"),
        )
        .clicked()
    {
        let rom = rom.to_path_buf();
        state.spawn(ui.ctx(), false, move || {
            let env = environment()?;
            let attention = melonds_install_attention(&env.history, &rom)?;
            let report = discover_shared_apply_history(&env.history);
            if !report.complete {
                return Err(
                    "History scan incomplete; retain evidence and review History & Logs".into(),
                );
            }
            let rows = report
                .journals
                .into_iter()
                .filter(|(_, j)| {
                    j.context.adapter == PreviewAdapter::MelonDs
                        && j.context.selected_archive.to_path_buf().ok().as_ref() == Some(&rom)
                })
                .collect();
            Ok(Message::History(
                rows,
                attention
                    .into_iter()
                    .chain(
                        report
                            .warnings
                            .iter()
                            .map(|w| format!("History evidence unavailable: {w:?}")),
                    )
                    .collect(),
            ))
        });
    }
    let mut choose = None;
    if let Some(r) = &state.result
        && let Some(path) = &r.journal_path
    {
        ui.label(format!(
            "Recorded install {:?}: {:?} (current Undo eligibility requires a fresh preview)",
            r.journal.operation_id, r.journal.status
        ));
        if ui
            .add_enabled(idle, egui::Button::new("Preview Undo current install"))
            .clicked()
        {
            choose = Some((
                path.clone(),
                r.journal
                    .destination_root
                    .to_path_buf()
                    .map_err(|e| format!("{e:?}")),
            ));
        }
    }
    for (path, j) in &state.history {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "Recorded install {} · {:?} · {} · ROM SHA-256 {} (not a current-installed claim)",
                j.operation_id,
                j.status,
                j.timestamp_unix_seconds,
                j.context.verified_game_identity
            ));
            if ui
                .add_enabled(
                    idle,
                    egui::Button::new(format!("Preview Undo {}", j.operation_id)),
                )
                .clicked()
            {
                choose = path.to_path_buf().ok().map(|p| {
                    (
                        p,
                        j.destination_root
                            .to_path_buf()
                            .map_err(|e| format!("{e:?}")),
                    )
                });
            }
        });
    }
    if let Some((path, root)) = choose {
        state.spawn(ui.ctx(), false, move || {
            let env = environment()?;
            Ok(Message::UndoPreview(preview_shared_rollback(
                &path,
                &root?,
                &env.backups,
            )))
        });
    }
    if let Some(p) = &state.undo {
        ui.label(format!(
            "Undo preview: {} · {:?}",
            if p.available {
                "available"
            } else {
                "refused / incomplete evidence"
            },
            p.entries
        ));
        ui.add_enabled(
            idle && p.available,
            egui::Checkbox::new(
                &mut state.undo_confirmed,
                "Confirm Undo of this exact recorded cheat install",
            ),
        );
        if ui
            .add_enabled(
                idle && p.available && state.undo_confirmed,
                egui::Button::new("Undo confirmed install"),
            )
            .clicked()
        {
            let p = p.clone();
            state.spawn(ui.ctx(), true, move || {
                let env = environment()?;
                let result = execute_shared_rollback(
                    &p,
                    &SharedRollbackOptions {
                        confirmation: SharedRollbackConfirmation {
                            preview_id: p.preview_id.clone(),
                            approved: true,
                        },
                        rollback_operation_id: generate_shared_operation_id(),
                        timestamp_unix_seconds: timestamp(),
                        history_root: env.history,
                        backup_root: env.backups,
                    },
                );
                Ok(Message::Undone(result))
            });
        }
    }
}

#[cfg(test)]
mod tests;
