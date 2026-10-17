//! Standalone native mGBA local-file journey; no emulator/config selection writes.
use crate::*;
use archivefs_core::patch_manager::*;
use std::path::{Path, PathBuf};
use std::result::Result;
use std::sync::mpsc::{self, Receiver};

#[derive(Clone)]
struct Environment {
    roots: MgbaProfileDiscoveryRoots,
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
        let matching: Vec<_> = profiles.iter().filter(|p| p.adapter == "mgba").collect();
        match matching.as_slice() {
            [] => Ok(None),
            [one] => Ok(Some(one.profile_id.clone())),
            _ => Err("Conflicting remembered mGBA profiles; review Emulator setup".into()),
        }
    }
}
fn environment() -> Result<Environment, String> {
    #[cfg(test)]
    {
        Err("Tests must supply disposable mGBA profiles explicitly".into())
    }
    #[cfg(not(test))]
    {
        Ok(Environment {
            roots: MgbaProfileDiscoveryRoots::from_environment().map_err(|e| e.to_string())?,
            memory: Some(default_emulator_profile_memory_path().map_err(|e| e.to_string())?),
            history: default_shared_history_root().map_err(|e| format!("{e:?}"))?,
            backups: default_shared_backup_root().map_err(|e| format!("{e:?}"))?,
        })
    }
}
struct Loaded {
    source: PathBuf,
    source_sha256: String,
    file: MgbaCheatFile,
    profile: MgbaProfile,
}
enum Message {
    Loaded(Loaded),
    Preview(Box<MgbaInstallPreview>),
    Applied(Box<SharedApplyResult>),
    History(
        Vec<(SharedTransactionPath, SharedApplyJournal)>,
        Vec<String>,
    ),
    UndoPreview(SharedRollbackPreview),
    Undone(SharedRollbackResult),
}
#[derive(Default)]
pub(crate) struct MgbaPageState {
    receiver: Option<Receiver<Result<Message, String>>>,
    mutating: bool,
    loaded: Option<Loaded>,
    selected: Vec<bool>,
    enabled: Vec<bool>,
    expected: String,
    preview: Option<Box<MgbaInstallPreview>>,
    confirmed: bool,
    result: Option<Box<SharedApplyResult>>,
    history: Vec<(SharedTransactionPath, SharedApplyJournal)>,
    undo: Option<SharedRollbackPreview>,
    undo_confirmed: bool,
    outcome: Option<String>,
    failure: Option<String>,
    identity_key: Option<String>,
}
impl MgbaPageState {
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
pub(crate) fn is_mgba_route(w: &CheatWorkflowState) -> bool {
    matches!(&w.routing.decision, CheatRouteDecision::Routed(r) if matches!(&r.target, CheatRouteTarget::Standalone{adapter_id} if adapter_id == "mgba"))
}
fn verified_rom(identity: Option<&GameIdentityReport>, rom: &Path) -> Result<String, String> {
    let id = identity
        .filter(|i| i.platform == archivefs_core::game_identity::IdentityPlatform::GameBoyAdvance)
        .ok_or("A verified Game Boy Advance identity is required")?;
    if !id.complete
        || id.format != archivefs_core::game_identity::IdentityImageFormat::LooseCartridgeRom
    {
        return Err("Complete loose-cartridge identity evidence is required".into());
    }
    if id.archive_path != rom {
        return Err("Identity belongs to another source".into());
    }
    let hashes: Vec<_> = id
        .evidence
        .iter()
        .filter(|f| {
            f.kind == archivefs_core::game_identity::IdentityKind::LooseRomSha256
                && f.status == archivefs_core::game_identity::IdentityStatus::Verified
        })
        .map(|f| {
            f.value
                .as_ref()
                .ok_or("A verified identity fact has no SHA-256".to_string())
        })
        .collect::<Result<Vec<_>, String>>()?;
    let Some(first) = hashes.first() else {
        return Err("No verified loose-ROM SHA-256. Recheck game identity before Preview.".into());
    };
    if first.len() != 64
        || !first.bytes().all(|b| b.is_ascii_hexdigit())
        || hashes.iter().any(|h| !h.eq_ignore_ascii_case(first))
    {
        return Err("Conflicting or malformed game-identity hashes".into());
    }
    Ok(first.to_ascii_lowercase())
}
fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn apply_preview(p: &MgbaInstallPreview, env: &Environment) -> Result<SharedApplyResult, String> {
    let remembered = env.remembered()?;
    let plan = p.plan();
    apply_mgba_install(
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
pub(crate) fn show(
    ui: &mut egui::Ui,
    state: &mut MgbaPageState,
    rom: &Path,
    identity: Option<&GameIdentityReport>,
) {
    use crate::widgets;
    state.poll();
    let identity_hash = verified_rom(identity, rom);
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
        "Standalone mGBA cheats",
        Some(
            "Local native .cheats files only. No downloads, ROM/config writes, or Launch selection changes.",
        ),
    );
    ui.label("Backend: native format inspection available. Preview: available for verified GBA files. Installation: requires the supported subset and an exact confirmed Preview. In-game effectiveness: unverified.");
    ui.label("Only directive-free VBA address:byte RAM writes can be installed here. Encrypted/opaque codes, GB/GBC, portable/Flatpak/custom profiles, existing destination files and guessed paths are unavailable.");
    ui.label("This does not make Launch select mGBA. Check Launch Readiness separately. This native adapter uses existing AUTO/remembered profile selection; mGBA has no shared FORCED-policy integration.");
    if let Err(e) = &identity_hash {
        ui.colored_label(egui::Color32::YELLOW, e);
    }
    let idle = state.receiver.is_none();
    if ui
        .add_enabled(
            idle && identity_hash.is_ok(),
            egui::Button::new("Choose local mGBA .cheats file"),
        )
        .clicked()
    {
        state.spawn(ui.ctx(), false, || {
            let source = rfd::FileDialog::new()
                .add_filter("mGBA cheats", &["cheats"])
                .pick_file()
                .ok_or("No file selected")?;
            let env = environment()?;
            let remembered = env.remembered()?;
            let profile = select_mgba_cheat_profile(&env.roots, remembered.as_deref())?;
            let (file, source_sha256) = inspect_mgba_local_cheats(&source)?;
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
        ui.label("Installed mGBA version: unverified. Native file semantics were checked against upstream 0.10.5; runtime compatibility is not established by inspection.");
        ui.label(format!(
            "Parser issues (never silently discarded): {:?}",
            l.file.issues
        ));
        for (i, e) in l.file.entries.iter().enumerate() {
            let supported = mgba_entry_installable(e);
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
                        egui::Checkbox::new(&mut state.enabled[i], "Enable in mGBA"),
                    )
                    .changed();
                ui.label(format!(
                    "Source {:?} · {:?} · {}",
                    e.format,
                    e.state,
                    if supported {
                        "supported RAM-write subset"
                    } else {
                        "unavailable / opaque or unsupported"
                    }
                ));
            });
            ui.small(
                e.codes
                    .iter()
                    .map(|c| c.raw.as_str())
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
        && state.selected.iter().any(|s| *s)
        && identity_hash
            .as_ref()
            .is_ok_and(|h| h.eq_ignore_ascii_case(state.expected.trim()));
    if ui
        .add_enabled(ready, egui::Button::new("Preview exact installation"))
        .clicked()
    {
        let l = state.loaded.as_ref().unwrap();
        let request = MgbaInstallRequest {
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
            preview_mgba_install(&request, &env.roots, remembered.as_deref())
                .map(|p| Message::Preview(Box::new(p)))
        });
    }
    if let Some(p) = &state.preview {
        ui.separator();
        ui.heading("Preview — not installed");
        ui.label(format!("Executable: {}", p.executable().display()));
        ui.label(format!("New file: {}", p.destination.display()));
        ui.label(format!(
            "Selected sets: {} · autoload: {:?} (configuration unchanged)",
            p.file.entries.len(),
            p.autoload
        ));
        for e in &p.file.entries {
            ui.label(format!(
                "{} · {:?} · {}",
                e.name,
                e.state,
                e.codes
                    .iter()
                    .map(|c| c.raw.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        ui.label("Existing destination: must remain absent. No merging or replacement. A SHA-256 match binds this locally supplied declaration to exact bytes; it does not authenticate the cheat author or prove in-game suitability.");
        ui.add_enabled(idle,egui::Checkbox::new(&mut state.confirmed,"I reviewed the source/ROM binding, selected enable states and exact new file; mGBA is closed. Install only this Preview."));
        if ui
            .add_enabled(
                idle && state.confirmed
                    && identity_hash
                        .as_ref()
                        .is_ok_and(|h| h.eq_ignore_ascii_case(&p.request().expected_rom_sha256)),
                egui::Button::new("Install confirmed mGBA cheats"),
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
            "mGBA operation needs attention",
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
    ui.heading("mGBA installation History / Undo");
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
            let attention = mgba_install_attention(&env.history, &rom)?;
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
                    j.context.adapter == PreviewAdapter::Mgba
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
    if let Some(r) = &state.result {
        if let Some(path) = &r.journal_path {
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
mod tests {
    use super::*;
    use archivefs_core::game_identity::*;
    fn identity() -> GameIdentityReport {
        let path = PathBuf::from("/disposable/game.gba");
        GameIdentityReport {
            archive_path: path.clone(),
            platform: IdentityPlatform::GameBoyAdvance,
            format: IdentityImageFormat::LooseCartridgeRom,
            evidence: vec![IdentityEvidence {
                kind: IdentityKind::LooseRomSha256,
                status: IdentityStatus::Verified,
                value: Some("a".repeat(64)),
                confidence: IdentityConfidence::ExactBytes,
                provenance: IdentityProvenance {
                    archive_path: path,
                    member_path: None,
                    member_index: None,
                    method: "synthetic".into(),
                },
                diagnostic: String::new(),
            }],
            warnings: vec![],
            bytes_read: 0,
            archive_members_inspected: 0,
            metadata_paths_inspected: 0,
            nested_container_depth: 0,
            complete: true,
        }
    }
    #[test]
    fn gba_sha_identity_is_required_not_title() {
        let id = identity();
        assert_eq!(
            verified_rom(Some(&id), &id.archive_path).unwrap(),
            "a".repeat(64)
        );
        assert!(verified_rom(None, &id.archive_path).is_err());
        assert!(verified_rom(Some(&id), Path::new("/different.gba")).is_err());
    }
    #[test]
    fn gb_and_wrong_platform_are_unavailable() {
        let mut id = identity();
        for platform in [
            IdentityPlatform::GameBoy,
            IdentityPlatform::GameBoyColor,
            IdentityPlatform::Snes,
        ] {
            id.platform = platform;
            assert!(verified_rom(Some(&id), &id.archive_path).is_err());
        }
    }
    #[test]
    fn uncertain_and_malformed_hashes_refuse() {
        let mut id = identity();
        id.evidence[0].status = IdentityStatus::Candidate;
        assert!(verified_rom(Some(&id), &id.archive_path).is_err());
        id.evidence[0].status = IdentityStatus::Verified;
        for value in [None, Some("bad".into()), Some("g".repeat(64))] {
            id.evidence[0].value = value;
            assert!(verified_rom(Some(&id), &id.archive_path).is_err());
        }
    }
    #[test]
    fn conflicting_reviewed_hashes_refuse() {
        let mut id = identity();
        let mut other = id.evidence[0].clone();
        other.value = Some("b".repeat(64));
        id.evidence.push(other);
        assert!(verified_rom(Some(&id), &id.archive_path).is_err());
    }
    #[test]
    fn valid_plus_missing_verified_hash_refuses() {
        let mut id = identity();
        let mut missing = id.evidence[0].clone();
        missing.value = None;
        id.evidence.push(missing);
        assert!(verified_rom(Some(&id), &id.archive_path).is_err());
    }
    #[test]
    fn empty_panel_has_no_implicit_confirm_or_install_and_fits_small_layout() {
        let ctx = egui::Context::default();
        let mut state = MgbaPageState::default();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1024., 600.),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    show(ui, &mut state, Path::new("/synthetic.gba"), None)
                });
            },
        );
        assert!(!state.confirmed);
        assert!(!state.undo_confirmed);
        assert!(state.receiver.is_none());
        assert!(state.preview.is_none());
    }
    #[test]
    fn explicit_selection_and_enablement_start_off() {
        let (tx, rx) = mpsc::channel();
        let mut state = MgbaPageState {
            receiver: Some(rx),
            ..Default::default()
        };
        let profile = MgbaProfile {
            profile_id: "synthetic".into(),
            installation_type: MgbaInstallationType::Native,
            configuration_path: "/synthetic".into(),
            config_path: None,
            eligible: false,
            blocker: None,
            executable_candidates: vec![],
            config: None,
        };
        tx.send(Ok(Message::Loaded(Loaded {
            source: "/synthetic.cheats".into(),
            source_sha256: "a".repeat(64),
            file: parse_mgba_cheat_file(b"# lives\n02000000:09\n").unwrap(),
            profile,
        })))
        .unwrap();
        state.poll();
        assert_eq!(state.selected, vec![false]);
        assert_eq!(state.enabled, vec![false]);
        assert!(!state.confirmed);
    }
    #[test]
    fn worker_failure_is_not_success_and_keeps_history() {
        let (tx, rx) = mpsc::channel();
        let mut state = MgbaPageState {
            receiver: Some(rx),
            mutating: true,
            ..Default::default()
        };
        tx.send(Err("source changed".into())).unwrap();
        state.poll();
        assert_eq!(state.failure.as_deref(), Some("source changed"));
        assert!(state.result.is_none());
        assert!(!state.busy());
        assert!(!state.confirmed);
    }
    #[test]
    fn tests_cannot_reach_production_environment() {
        assert!(environment().is_err());
    }
}
