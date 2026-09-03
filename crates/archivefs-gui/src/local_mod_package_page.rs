//! Beginner-facing local, ordinary (non-cheat) mod package workflow.
//! Inspection is read-only; applying and undoing use the shared transaction
//! journal and backup machinery.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{SystemTime, UNIX_EPOCH};

use archivefs_core::game_identity::GameIdentityReport;
use archivefs_core::mod_package::{
    LocalModPackagePlan, LocalModPackageRequest, SelectedGameForMod,
    build_local_mod_package_transaction_plan, inspect_local_mod_package,
};
use archivefs_core::patch_manager::{
    SharedApplyConfirmation, SharedApplyOptions, SharedApplyOutcome, SharedApplyResult,
    SharedApplyStatus, SharedRollbackConfirmation, SharedRollbackOptions, SharedRollbackPreview,
    SharedTransactionPlan, default_shared_backup_root, default_shared_history_root,
    execute_shared_apply, execute_shared_rollback, generate_shared_operation_id,
    preview_shared_rollback,
};
use eframe::egui;

use crate::ui::components as widgets;

enum Stage {
    Pick(Receiver<Option<PathBuf>>, SelectedGameForMod),
    Planned(LocalModPackagePlan),
    Confirm(SharedTransactionPlan),
    Applying(Receiver<SharedApplyResult>, SharedTransactionPlan),
    Applied(SharedApplyResult),
    Rollback(SharedRollbackPreview),
    RollingBack(Receiver<archivefs_core::patch_manager::SharedRollbackResult>),
    /// A rollback worker reported back. Carries the honest final status so
    /// the render arm can present success, partial, or failed distinctly -
    /// a successful undo must never reach `Stage::Failed`.
    RolledBack(SharedApplyStatus),
    Failed(String),
}

pub struct LocalModPackagePageState {
    key: Option<(PathBuf, PathBuf)>,
    stage: Option<Stage>,
}

impl Default for LocalModPackagePageState {
    fn default() -> Self {
        Self {
            key: None,
            stage: None,
        }
    }
}

impl LocalModPackagePageState {
    pub fn is_busy(&self) -> bool {
        matches!(
            self.stage,
            Some(Stage::Pick(..) | Stage::Applying(_, _) | Stage::RollingBack(_))
        )
    }

    pub fn poll(&mut self) -> bool {
        let Some(stage) = self.stage.take() else {
            return false;
        };
        match stage {
            Stage::Pick(receiver, selected_game) => match receiver.try_recv() {
                Ok(Some(path)) => {
                    self.stage = Some(Stage::Planned(inspect_local_mod_package(
                        LocalModPackageRequest {
                            selected_game,
                            package_root: path,
                        },
                    )))
                }
                Ok(None) | Err(TryRecvError::Disconnected) => {}
                Err(TryRecvError::Empty) => self.stage = Some(Stage::Pick(receiver, selected_game)),
            },
            Stage::Applying(receiver, plan) => match receiver.try_recv() {
                Ok(result) => self.stage = Some(Stage::Applied(result)),
                Err(TryRecvError::Empty) => self.stage = Some(Stage::Applying(receiver, plan)),
                Err(TryRecvError::Disconnected) => {
                    self.stage = Some(Stage::Failed(
                        "The mod apply worker stopped before reporting a result.".into(),
                    ))
                }
            },
            Stage::RollingBack(receiver) => match receiver.try_recv() {
                Ok(result) => self.stage = Some(Stage::RolledBack(result.status)),
                Err(TryRecvError::Empty) => self.stage = Some(Stage::RollingBack(receiver)),
                Err(TryRecvError::Disconnected) => {
                    self.stage = Some(Stage::Failed(
                        "The rollback worker stopped before reporting a result.".into(),
                    ))
                }
            },
            other => self.stage = Some(other),
        }
        true
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// How a finished apply or rollback should read to a beginner: one headline,
/// one plain sentence, and a tone drawn from the existing GUI conventions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StatusPresentation {
    headline: &'static str,
    detail: &'static str,
    tone: widgets::StatusTone,
}

/// Presentation for a completed `execute_shared_apply`. A written journal
/// (`journal_path.is_some()`) does not by itself mean the mod applied: the
/// journal is written for `PartialFailure` and `Failed` outcomes too, so the
/// only honest signal is `SharedApplyJournal::status`.
fn apply_presentation(status: SharedApplyStatus) -> StatusPresentation {
    match status {
        SharedApplyStatus::Success => StatusPresentation {
            headline: "Mod installed",
            detail: "Game files were changed only by the confirmed plan.",
            tone: widgets::StatusTone::Success,
        },
        SharedApplyStatus::PartialFailure => StatusPresentation {
            headline: "Mod only partly applied",
            detail: "Some of this mod's files were not applied, so the mod is not fully installed. Undo restores the files that did change.",
            tone: widgets::StatusTone::Warning,
        },
        SharedApplyStatus::Failed => StatusPresentation {
            headline: "Mod was not applied",
            detail: "None of this mod's changes were applied. Your game files are unchanged.",
            tone: widgets::StatusTone::Blocked,
        },
        SharedApplyStatus::DryRun => StatusPresentation {
            headline: "Preview only",
            detail: "Nothing was written to your game files.",
            tone: widgets::StatusTone::Info,
        },
    }
}

/// Presentation for a completed `execute_shared_rollback`. A successful undo
/// must render as success, never as `Stage::Failed`.
fn rollback_presentation(status: SharedApplyStatus) -> StatusPresentation {
    match status {
        SharedApplyStatus::Success => StatusPresentation {
            headline: "Mod removed",
            detail: "The previous game files were restored.",
            tone: widgets::StatusTone::Success,
        },
        SharedApplyStatus::PartialFailure => StatusPresentation {
            headline: "Undo only partly finished",
            detail: "Some files were restored, but not all. Check this game before playing.",
            tone: widgets::StatusTone::Warning,
        },
        SharedApplyStatus::Failed | SharedApplyStatus::DryRun => StatusPresentation {
            headline: "Undo did not complete",
            detail: "The previous game files were not restored.",
            tone: widgets::StatusTone::Blocked,
        },
    }
}

/// Whether an apply result actually changed at least one game file that a
/// rollback could restore. `AlreadyInstalled` is deliberately excluded (the
/// file was already in place, so there is nothing to undo), as is a result
/// with no written journal. This is what gates the Undo control for every
/// status: on `Failed` no entry ever qualifies, on `PartialFailure` only the
/// subset that genuinely changed does.
fn has_restorable_changes(result: &SharedApplyResult) -> bool {
    result.journal_path.is_some()
        && result.journal.entries.iter().any(|entry| {
            matches!(
                entry.outcome,
                SharedApplyOutcome::InstalledNew | SharedApplyOutcome::ReplacedExisting
            )
        })
}

pub fn show_local_mod_package_panel(
    ui: &mut egui::Ui,
    state: &mut LocalModPackagePageState,
    archive_path: &std::path::Path,
    identity: Option<&GameIdentityReport>,
) {
    let Some(identity) = identity else {
        widgets::section_header(ui, "Ordinary game mods", None);
        widgets::card(ui, |ui| {
            ui.label("Load exact game identity evidence before choosing a mod.");
        });
        return;
    };
    let Some(game_root) = archive_path
        .parent()
        .filter(|path| path.is_dir())
        .map(PathBuf::from)
    else {
        widgets::section_header(ui, "Ordinary game mods", None);
        widgets::card(ui, |ui| {
            ui.label("EmuWiz cannot identify a safe game folder for this file, so it will not offer mod installation.");
        });
        return;
    };
    let key = (archive_path.to_path_buf(), game_root.clone());
    if state.key.as_ref() != Some(&key) {
        state.key = Some(key);
        state.stage = None;
    }
    widgets::section_header(
        ui,
        "Ordinary game mods",
        Some("Choose a local mod folder. EmuWiz previews every file before anything changes."),
    );
    if state.stage.is_none() {
        if widgets::action_button(
            ui,
            "Choose local mod folder",
            widgets::ActionStyle::Secondary,
            true,
        )
        .clicked()
        {
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(rfd::FileDialog::new().pick_folder());
            });
            state.stage = Some(Stage::Pick(
                receiver,
                SelectedGameForMod {
                    game_root: game_root.clone(),
                    identity: identity.clone(),
                },
            ));
        }
        return;
    }
    let stage = state.stage.take().unwrap();
    match stage {
        Stage::Pick(receiver, selected_game) => {
            ui.label("Waiting for folder selection…");
            state.stage = Some(Stage::Pick(receiver, selected_game));
        }
        Stage::Planned(plan) => show_plan(ui, state, plan, archive_path, identity, &game_root),
        Stage::Confirm(plan) => {
            widgets::card(ui, |ui| {
                ui.label(format!(
                    "Apply {}? Nothing is written until you confirm.",
                    plan.entries.len()
                ));
                ui.horizontal(|ui| {
                    if widgets::action_button(ui, "Keep preview", widgets::ActionStyle::Quiet, true)
                        .clicked()
                    {
                        state.stage = None;
                    }
                    if widgets::action_button(
                        ui,
                        "Confirm apply",
                        widgets::ActionStyle::Primary,
                        true,
                    )
                    .clicked()
                    {
                        let (sender, receiver) = mpsc::channel();
                        let worker_plan = plan.clone();
                        std::thread::spawn(move || {
                            let result = (|| {
                                let history_root =
                                    default_shared_history_root().map_err(|e| e.detail)?;
                                let backup_root =
                                    default_shared_backup_root().map_err(|e| e.detail)?;
                                Ok::<_, String>(execute_shared_apply(
                                    &worker_plan,
                                    &SharedApplyOptions {
                                        dry_run: false,
                                        confirmation: Some(SharedApplyConfirmation {
                                            plan_id: worker_plan.plan_id.clone(),
                                            general_approved: true,
                                            replacement_approved: true,
                                        }),
                                        operation_id: generate_shared_operation_id(),
                                        timestamp_unix_seconds: now(),
                                        current_context: worker_plan.context.clone(),
                                        history_root,
                                        backup_root,
                                    },
                                ))
                            })();
                            if let Ok(result) = result {
                                let _ = sender.send(result);
                            }
                        });
                        state.stage = Some(Stage::Applying(receiver, plan));
                    }
                });
            });
        }
        Stage::Applying(receiver, plan) => {
            ui.label("Applying mod safely…");
            state.stage = Some(Stage::Applying(receiver, plan));
        }
        Stage::Applied(result) => {
            let presentation = apply_presentation(result.journal.status);
            let can_undo = has_restorable_changes(&result);
            widgets::card(ui, |ui| {
                widgets::status_badge(ui, presentation.headline, presentation.tone);
                ui.label(presentation.detail);
                if can_undo {
                    if let Some(journal) = result.journal_path.as_ref() {
                        if widgets::action_button(
                            ui,
                            "Undo this mod",
                            widgets::ActionStyle::Destructive,
                            true,
                        )
                        .clicked()
                        {
                            if let (Ok(backup), Ok(_history)) =
                                (default_shared_backup_root(), default_shared_history_root())
                            {
                                state.stage = Some(Stage::Rollback(preview_shared_rollback(
                                    journal, &game_root, &backup,
                                )));
                            }
                        }
                    }
                }
            });
            if !matches!(state.stage, Some(Stage::Rollback(_))) {
                state.stage = Some(Stage::Applied(result));
            }
        }
        Stage::Rollback(preview) => {
            widgets::card(ui, |ui| {
                ui.label(if preview.available {
                    "Undo this mod and restore the exact previous files?"
                } else {
                    "This mod can no longer be safely undone."
                });
                if widgets::action_button(
                    ui,
                    "Confirm undo",
                    widgets::ActionStyle::Destructive,
                    preview.available,
                )
                .clicked()
                {
                    if let (Ok(history_root), Ok(backup_root)) =
                        (default_shared_history_root(), default_shared_backup_root())
                    {
                        let (sender, receiver) = mpsc::channel();
                        std::thread::spawn(move || {
                            let result = execute_shared_rollback(
                                &preview,
                                &SharedRollbackOptions {
                                    confirmation: SharedRollbackConfirmation {
                                        preview_id: preview.preview_id.clone(),
                                        approved: true,
                                    },
                                    rollback_operation_id: generate_shared_operation_id(),
                                    timestamp_unix_seconds: now(),
                                    history_root,
                                    backup_root,
                                },
                            );
                            let _ = sender.send(result);
                        });
                        state.stage = Some(Stage::RollingBack(receiver));
                    }
                }
            });
        }
        Stage::RollingBack(receiver) => {
            ui.label("Undoing mod…");
            state.stage = Some(Stage::RollingBack(receiver));
        }
        Stage::RolledBack(status) => {
            let presentation = rollback_presentation(status);
            let mut dismissed = false;
            widgets::card(ui, |ui| {
                widgets::status_badge(ui, presentation.headline, presentation.tone);
                ui.label(presentation.detail);
                if widgets::action_button(ui, "Done", widgets::ActionStyle::Quiet, true).clicked() {
                    dismissed = true;
                }
            });
            if !dismissed {
                state.stage = Some(Stage::RolledBack(status));
            }
        }
        Stage::Failed(detail) => {
            widgets::banner(
                ui,
                "Mod workflow stopped",
                &detail,
                widgets::StatusTone::Blocked,
            );
            if widgets::action_button(ui, "Start over", widgets::ActionStyle::Quiet, true).clicked()
            {
                state.stage = None;
            } else {
                state.stage = Some(Stage::Failed(detail));
            }
        }
    }
}

fn show_plan(
    ui: &mut egui::Ui,
    state: &mut LocalModPackagePageState,
    plan: LocalModPackagePlan,
    archive_path: &std::path::Path,
    identity: &GameIdentityReport,
    game_root: &std::path::Path,
) {
    widgets::card(ui, |ui| {
        if let Some(package) = plan.package.as_ref() {
            ui.label(format!("{} {}", package.title, package.version));
        }
        for blocker in &plan.blockers {
            widgets::banner(
                ui,
                "Cannot apply this mod",
                &blocker.detail,
                widgets::StatusTone::Blocked,
            );
        }
        for conflict in &plan.conflicts {
            ui.label(format!("Conflict: {}", conflict.detail));
        }
        if plan.blockers.is_empty() && plan.conflicts.is_empty() {
            ui.label(format!(
                "{} file(s) will be added or replaced below {}.",
                plan.operations.len(),
                game_root.display()
            ));
            for operation in plan.operations.iter().take(12) {
                ui.label(format!(
                    "{:?}: {}",
                    operation.kind,
                    operation.destination_path.display()
                ));
            }
            if let Ok(transaction) = build_local_mod_package_transaction_plan(&plan) {
                if widgets::action_button(
                    ui,
                    "Review and apply mod",
                    widgets::ActionStyle::Primary,
                    true,
                )
                .clicked()
                {
                    state.stage = Some(Stage::Confirm(transaction));
                }
            }
        }
        if widgets::action_button(
            ui,
            "Choose another folder",
            widgets::ActionStyle::Quiet,
            true,
        )
        .clicked()
        {
            state.stage = None;
        }
    });
    let _ = (archive_path, identity);
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use archivefs_core::game_identity::{
        GameIdentityReport, IdentityConfidence, IdentityEvidence, IdentityImageFormat,
        IdentityKind, IdentityPlatform, IdentityProvenance, IdentityStatus,
    };
    use archivefs_core::mod_package::{LocalModPackageRequest, SelectedGameForMod};
    use archivefs_core::patch_manager::{
        SharedApplyConfirmation, SharedApplyOptions, SharedApplyStatus, execute_shared_apply,
    };
    use eframe::egui;

    use super::*;

    fn identity(game_bin: &Path) -> GameIdentityReport {
        GameIdentityReport {
            archive_path: game_bin.to_path_buf(),
            platform: IdentityPlatform::Snes,
            format: IdentityImageFormat::LooseCartridgeRom,
            evidence: vec![IdentityEvidence {
                kind: IdentityKind::LooseRomSha256,
                status: IdentityStatus::Verified,
                value: Some("game-sha".to_string()),
                confidence: IdentityConfidence::ExactBytes,
                provenance: IdentityProvenance {
                    archive_path: game_bin.to_path_buf(),
                    member_path: None,
                    member_index: None,
                    method: "test".to_string(),
                },
                diagnostic: String::new(),
            }],
            warnings: Vec::new(),
            bytes_read: 0,
            archive_members_inspected: 0,
            metadata_paths_inspected: 0,
            nested_container_depth: 0,
            complete: true,
        }
    }

    /// A game tree with `game.bin`, and a local mod package directory holding
    /// one `operation` (a raw JSON object) plus optional payload bytes.
    fn scenario(
        operation: &str,
        payload: Option<(&str, &[u8])>,
    ) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::TempDir::new().unwrap();
        let game_root = temp.path().join("game root");
        fs::create_dir(&game_root).unwrap();
        fs::write(game_root.join("game.bin"), b"original").unwrap();
        let package_root = temp.path().join("mod");
        fs::create_dir(&package_root).unwrap();
        let manifest = format!(
            r#"{{"format_version":1,"package_id":"t.mod","title":"T","version":"1.0","supported_platform":"snes","supported_game":{{"identities":[{{"kind":"loose_rom_sha256","value":"game-sha"}}]}},"operations":[{operation}],"provenance":{{"source":"test"}}}}"#
        );
        fs::write(package_root.join("emuwiz.mod.json"), manifest).unwrap();
        if let Some((rel, bytes)) = payload {
            let path = package_root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        (temp, game_root, package_root)
    }

    fn plan_for(game_root: &Path, package_root: &Path) -> LocalModPackagePlan {
        inspect_local_mod_package(LocalModPackageRequest {
            selected_game: SelectedGameForMod {
                game_root: game_root.to_path_buf(),
                identity: identity(&game_root.join("game.bin")),
            },
            package_root: package_root.to_path_buf(),
        })
    }

    fn render(
        state: &mut LocalModPackagePageState,
        archive: &Path,
        id: Option<&GameIdentityReport>,
    ) -> egui::FullOutput {
        let ctx = egui::Context::default();
        let draw = |ctx: &egui::Context| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_local_mod_package_panel(ui, state, archive, id);
            });
        };
        ctx.run(egui::RawInput::default(), draw)
    }

    fn text_contains(output: &egui::FullOutput, needle: &str) -> bool {
        fn walk(shape: &egui::Shape, needle: &str) -> bool {
            match shape {
                egui::Shape::Text(t) => t.galley.text().contains(needle),
                egui::Shape::Vec(v) => v.iter().any(|s| walk(s, needle)),
                _ => false,
            }
        }
        output.shapes.iter().any(|c| walk(&c.shape, needle))
    }

    #[test]
    fn without_identity_evidence_the_panel_refuses_and_fabricates_no_success() {
        let temp = tempfile::TempDir::new().unwrap();
        let archive = temp.path().join("game root/game.bin");
        fs::create_dir_all(archive.parent().unwrap()).unwrap();
        fs::write(&archive, b"x").unwrap();
        let mut state = LocalModPackagePageState::default();
        let output = render(&mut state, &archive, None);
        assert!(text_contains(&output, "Load exact game identity evidence"));
        assert!(!text_contains(&output, "Mod apply finished"));
        assert!(!text_contains(&output, "Review and apply mod"));
        assert!(state.stage.is_none());
    }

    #[test]
    fn without_a_safe_game_folder_the_panel_refuses() {
        let temp = tempfile::TempDir::new().unwrap();
        // archive_path whose parent is not a directory.
        let archive = temp.path().join("nope/game.bin");
        let id = identity(&archive);
        let mut state = LocalModPackagePageState::default();
        let output = render(&mut state, &archive, Some(&id));
        assert!(text_contains(&output, "cannot identify a safe game folder"));
        assert!(!text_contains(&output, "Choose local mod folder"));
    }

    #[test]
    fn initial_state_only_offers_a_folder_choice() {
        let (_temp, game_root, _pkg) = scenario(
            r#"{"kind":"replace","payload":"p/new.bin","destination":"game.bin"}"#,
            None,
        );
        let archive = game_root.join("game.bin");
        let id = identity(&archive);
        let mut state = LocalModPackagePageState::default();
        let output = render(&mut state, &archive, Some(&id));
        assert!(text_contains(&output, "Choose local mod folder"));
        assert!(!text_contains(&output, "Confirm apply"));
        assert!(!text_contains(&output, "Mod apply finished"));
    }

    #[test]
    fn a_safe_inspected_package_reaches_a_preview_with_an_apply_control() {
        let (_temp, game_root, package_root) = scenario(
            r#"{"kind":"replace","payload":"p/new.bin","destination":"game.bin"}"#,
            Some(("p/new.bin", b"replacement")),
        );
        let archive = game_root.join("game.bin");
        let id = identity(&archive);
        let plan = plan_for(&game_root, &package_root);
        assert!(plan.eligible_for_later_apply);
        assert!(build_local_mod_package_transaction_plan(&plan).is_ok());

        let mut state = LocalModPackagePageState {
            key: Some((archive.clone(), game_root.clone())),
            stage: Some(Stage::Planned(plan)),
        };
        let output = render(&mut state, &archive, Some(&id));
        assert!(text_contains(&output, "Review and apply mod"));
        assert!(!text_contains(&output, "Cannot apply this mod"));
        // Rendering the preview writes nothing.
        assert_eq!(fs::read(archive).unwrap(), b"original");
    }

    #[test]
    fn a_blocked_package_shows_the_refusal_and_cannot_arm_apply() {
        // Delete is refused as an unsupported operation.
        let (_temp, game_root, package_root) =
            scenario(r#"{"kind":"delete","destination":"game.bin"}"#, None);
        let archive = game_root.join("game.bin");
        let id = identity(&archive);
        let plan = plan_for(&game_root, &package_root);
        assert!(!plan.eligible_for_later_apply);
        assert!(build_local_mod_package_transaction_plan(&plan).is_err());

        let mut state = LocalModPackagePageState {
            key: Some((archive.clone(), game_root.clone())),
            stage: Some(Stage::Planned(plan)),
        };
        let output = render(&mut state, &archive, Some(&id));
        assert!(text_contains(&output, "Cannot apply this mod"));
        assert!(!text_contains(&output, "Review and apply mod"));
    }

    #[test]
    fn the_confirm_stage_requires_explicit_confirmation_and_writes_nothing_on_render() {
        let (_temp, game_root, package_root) = scenario(
            r#"{"kind":"replace","payload":"p/new.bin","destination":"game.bin"}"#,
            Some(("p/new.bin", b"replacement")),
        );
        let archive = game_root.join("game.bin");
        let id = identity(&archive);
        let plan = plan_for(&game_root, &package_root);
        let transaction = build_local_mod_package_transaction_plan(&plan).unwrap();

        let mut state = LocalModPackagePageState {
            key: Some((archive.clone(), game_root.clone())),
            stage: Some(Stage::Confirm(transaction)),
        };
        let output = render(&mut state, &archive, Some(&id));
        assert!(text_contains(
            &output,
            "Nothing is written until you confirm"
        ));
        assert!(text_contains(&output, "Confirm apply"));
        assert!(!text_contains(&output, "Mod apply finished"));
        assert_eq!(
            fs::read(archive).unwrap(),
            b"original",
            "just rendering the confirm stage must not apply anything"
        );
    }

    #[test]
    fn the_applied_stage_exposes_an_undo_control() {
        // A create at a fresh nested path: an install has a verified payload
        // digest and needs no pre-existing source, so the end-to-end apply is
        // deterministic here. (The full replace-restore round trip is covered
        // by the core `mod_package` execution tests.)
        let (temp, game_root, package_root) = scenario(
            r#"{"kind":"create","payload":"p/new.bin","destination":"mods/added.bin"}"#,
            Some(("p/new.bin", b"added-bytes")),
        );
        let archive = game_root.join("game.bin");
        let id = identity(&archive);
        let plan = plan_for(&game_root, &package_root);
        let transaction = build_local_mod_package_transaction_plan(&plan).unwrap();

        let history_root = temp.path().join("history");
        let backup_root = temp.path().join("backups");
        let result = execute_shared_apply(
            &transaction,
            &SharedApplyOptions {
                dry_run: false,
                confirmation: Some(SharedApplyConfirmation {
                    plan_id: transaction.plan_id.clone(),
                    general_approved: true,
                    replacement_approved: true,
                }),
                operation_id: "gui-test-apply".into(),
                timestamp_unix_seconds: 1_700_000_000,
                current_context: transaction.context.clone(),
                history_root,
                backup_root,
            },
        );
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        assert!(result.journal_path.is_some());

        let mut state = LocalModPackagePageState {
            key: Some((archive.clone(), game_root.clone())),
            stage: Some(Stage::Applied(result)),
        };
        let output = render(&mut state, &archive, Some(&id));
        assert!(text_contains(&output, "Undo this mod"));
        assert!(matches!(state.stage, Some(Stage::Applied(_))));
    }

    // --- status -> presentation --------------------------------------------

    #[test]
    fn apply_presentation_never_reads_as_a_clean_install_below_success() {
        let success = apply_presentation(SharedApplyStatus::Success);
        assert_eq!(success.headline, "Mod installed");
        assert_eq!(success.tone, widgets::StatusTone::Success);

        let partial = apply_presentation(SharedApplyStatus::PartialFailure);
        assert_ne!(partial.headline, "Mod installed");
        assert_eq!(partial.tone, widgets::StatusTone::Warning);
        assert!(partial.detail.contains("not fully installed"));

        let failed = apply_presentation(SharedApplyStatus::Failed);
        assert_ne!(failed.headline, "Mod installed");
        assert_eq!(failed.tone, widgets::StatusTone::Blocked);
        assert!(failed.detail.contains("unchanged"));
    }

    #[test]
    fn rollback_presentation_reports_success_as_success_not_failure() {
        let success = rollback_presentation(SharedApplyStatus::Success);
        assert_eq!(success.headline, "Mod removed");
        assert_eq!(success.tone, widgets::StatusTone::Success);
        assert_ne!(success.headline, "Mod workflow stopped");

        assert_eq!(
            rollback_presentation(SharedApplyStatus::PartialFailure).tone,
            widgets::StatusTone::Warning
        );
        assert_eq!(
            rollback_presentation(SharedApplyStatus::Failed).tone,
            widgets::StatusTone::Blocked
        );
    }

    /// Applies `manifest` (already-substituted) from `package_root` against a
    /// `game.bin` game tree and returns the raw `SharedApplyResult`.
    fn apply_result(
        temp: &Path,
        game_root: &Path,
        package_root: &Path,
        operation_id: &str,
    ) -> SharedApplyResult {
        let plan = plan_for(game_root, package_root);
        let transaction = build_local_mod_package_transaction_plan(&plan).unwrap();
        execute_shared_apply(
            &transaction,
            &SharedApplyOptions {
                dry_run: false,
                confirmation: Some(SharedApplyConfirmation {
                    plan_id: transaction.plan_id.clone(),
                    general_approved: true,
                    replacement_approved: true,
                }),
                operation_id: operation_id.to_string(),
                timestamp_unix_seconds: 1_700_000_000,
                current_context: transaction.context.clone(),
                history_root: temp.join("history"),
                backup_root: temp.join("backups"),
            },
        )
    }

    #[test]
    fn successful_apply_renders_success_wording_and_keeps_undo() {
        let (temp, game_root, package_root) = scenario(
            r#"{"kind":"create","payload":"p/new.bin","destination":"mods/added.bin"}"#,
            Some(("p/new.bin", b"added-bytes")),
        );
        let archive = game_root.join("game.bin");
        let id = identity(&archive);
        let result = apply_result(temp.path(), &game_root, &package_root, "gui-success");
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        assert!(has_restorable_changes(&result));

        let mut state = LocalModPackagePageState {
            key: Some((archive.clone(), game_root.clone())),
            stage: Some(Stage::Applied(result)),
        };
        let output = render(&mut state, &archive, Some(&id));
        assert!(text_contains(&output, "Mod installed"));
        assert!(text_contains(&output, "Undo this mod"));
    }

    #[test]
    fn failed_apply_is_not_shown_as_a_clean_install_and_offers_no_undo() {
        // A written journal is not proof of success: force `Failed` with a
        // confirmation whose plan id does not match, then confirm the render
        // arm keys on `journal.status`, not on `journal_path`.
        let (_temp, game_root, package_root) = scenario(
            r#"{"kind":"create","payload":"p/new.bin","destination":"mods/added.bin"}"#,
            Some(("p/new.bin", b"added-bytes")),
        );
        let archive = game_root.join("game.bin");
        let id = identity(&archive);
        let plan = plan_for(&game_root, &package_root);
        let transaction = build_local_mod_package_transaction_plan(&plan).unwrap();
        let result = execute_shared_apply(
            &transaction,
            &SharedApplyOptions {
                dry_run: false,
                confirmation: Some(SharedApplyConfirmation {
                    plan_id: "not-the-real-plan-id".into(),
                    general_approved: true,
                    replacement_approved: true,
                }),
                operation_id: "gui-failed".into(),
                timestamp_unix_seconds: 1_700_000_000,
                current_context: transaction.context.clone(),
                history_root: _temp.path().join("history"),
                backup_root: _temp.path().join("backups"),
            },
        );
        assert_eq!(result.journal.status, SharedApplyStatus::Failed);
        assert!(!has_restorable_changes(&result));

        let mut state = LocalModPackagePageState {
            key: Some((archive.clone(), game_root.clone())),
            stage: Some(Stage::Applied(result)),
        };
        let output = render(&mut state, &archive, Some(&id));
        assert!(text_contains(&output, "Mod was not applied"));
        assert!(!text_contains(&output, "Mod installed"));
        assert!(!text_contains(&output, "Undo this mod"));
    }

    #[test]
    fn partial_apply_is_not_presented_as_clean_success_but_keeps_undo_for_the_changed_subset() {
        let temp = tempfile::TempDir::new().unwrap();
        let game_root = temp.path().join("game root");
        fs::create_dir(&game_root).unwrap();
        fs::write(game_root.join("game.bin"), b"original").unwrap();
        let package_root = temp.path().join("mod");
        fs::create_dir(&package_root).unwrap();
        let manifest = r#"{"format_version":1,"package_id":"t.mod","title":"T","version":"1.0","supported_platform":"snes","supported_game":{"identities":[{"kind":"loose_rom_sha256","value":"game-sha"}]},"operations":[{"kind":"create","payload":"p/a.bin","destination":"mods/a.bin"},{"kind":"create","payload":"p/b.bin","destination":"mods/b.bin"}],"provenance":{"source":"test"}}"#;
        fs::write(package_root.join("emuwiz.mod.json"), manifest).unwrap();
        fs::create_dir_all(package_root.join("p")).unwrap();
        fs::write(package_root.join("p/a.bin"), b"aaaa").unwrap();
        fs::write(package_root.join("p/b.bin"), b"bbbb").unwrap();

        let archive = game_root.join("game.bin");
        let id = identity(&archive);
        let plan = plan_for(&game_root, &package_root);
        let transaction = build_local_mod_package_transaction_plan(&plan).unwrap();

        // Race: one create's destination now exists with different bytes, so
        // that entry is skipped while the other still installs.
        fs::create_dir_all(game_root.join("mods")).unwrap();
        fs::write(game_root.join("mods/b.bin"), b"squatter").unwrap();

        let result = execute_shared_apply(
            &transaction,
            &SharedApplyOptions {
                dry_run: false,
                confirmation: Some(SharedApplyConfirmation {
                    plan_id: transaction.plan_id.clone(),
                    general_approved: true,
                    replacement_approved: true,
                }),
                operation_id: "gui-partial".into(),
                timestamp_unix_seconds: 1_700_000_000,
                current_context: transaction.context.clone(),
                history_root: temp.path().join("history"),
                backup_root: temp.path().join("backups"),
            },
        );
        assert_eq!(result.journal.status, SharedApplyStatus::PartialFailure);
        assert!(
            result.journal_path.is_some(),
            "a journal is written even on partial failure"
        );
        assert!(has_restorable_changes(&result));

        let mut state = LocalModPackagePageState {
            key: Some((archive.clone(), game_root.clone())),
            stage: Some(Stage::Applied(result)),
        };
        let output = render(&mut state, &archive, Some(&id));
        assert!(!text_contains(&output, "Mod installed"));
        assert!(text_contains(&output, "only partly applied"));
        assert!(text_contains(&output, "Undo this mod"));
    }

    #[test]
    fn a_completed_rollback_that_succeeded_becomes_rolled_back_not_failed() {
        let (temp, game_root, package_root) = scenario(
            r#"{"kind":"create","payload":"p/new.bin","destination":"mods/added.bin"}"#,
            Some(("p/new.bin", b"added-bytes")),
        );
        let archive = game_root.join("game.bin");
        let id = identity(&archive);
        let history_root = temp.path().join("history");
        let backup_root = temp.path().join("backups");
        let result = apply_result(temp.path(), &game_root, &package_root, "gui-rb-apply");
        let journal_path = result.journal_path.clone().unwrap();

        let preview = preview_shared_rollback(&journal_path, &game_root, &backup_root);
        assert!(preview.available);
        let rollback = execute_shared_rollback(
            &preview,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: preview.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: generate_shared_operation_id(),
                timestamp_unix_seconds: 1_700_000_100,
                history_root,
                backup_root,
            },
        );
        assert_eq!(rollback.status, SharedApplyStatus::Success);

        let (sender, receiver) = mpsc::channel();
        sender.send(rollback).unwrap();
        let mut state = LocalModPackagePageState {
            key: Some((archive.clone(), game_root.clone())),
            stage: Some(Stage::RollingBack(receiver)),
        };
        assert!(state.poll());
        assert!(
            matches!(
                state.stage,
                Some(Stage::RolledBack(SharedApplyStatus::Success))
            ),
            "a successful rollback must not land in Stage::Failed"
        );

        let output = render(&mut state, &archive, Some(&id));
        assert!(text_contains(&output, "Mod removed"));
        assert!(!text_contains(&output, "Mod workflow stopped"));
    }

    #[test]
    fn a_non_success_rollback_status_renders_a_non_success_state() {
        let temp = tempfile::TempDir::new().unwrap();
        let game_root = temp.path().join("game root");
        fs::create_dir(&game_root).unwrap();
        let archive = game_root.join("game.bin");
        fs::write(&archive, b"x").unwrap();
        let id = identity(&archive);

        for status in [SharedApplyStatus::PartialFailure, SharedApplyStatus::Failed] {
            let mut state = LocalModPackagePageState {
                key: Some((archive.clone(), game_root.clone())),
                stage: Some(Stage::RolledBack(status)),
            };
            let output = render(&mut state, &archive, Some(&id));
            assert!(!text_contains(&output, "Mod removed"));
            let expected = match status {
                SharedApplyStatus::PartialFailure => "Undo only partly finished",
                _ => "Undo did not complete",
            };
            assert!(text_contains(&output, expected));
        }
    }

    #[test]
    fn a_rollback_worker_that_disconnects_still_fails_loudly() {
        let (sender, receiver) =
            mpsc::channel::<archivefs_core::patch_manager::SharedRollbackResult>();
        drop(sender);
        let mut state = LocalModPackagePageState {
            key: None,
            stage: Some(Stage::RollingBack(receiver)),
        };
        assert!(state.poll());
        assert!(matches!(state.stage, Some(Stage::Failed(_))));
    }
}
