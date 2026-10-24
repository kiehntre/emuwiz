//! Selected-game recovery journey for durable standalone patch outputs.
//! Discovery and assessment are read-only; mutations require a fresh Core
//! plan and an explicit confirmation in this panel.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use archivefs_core::patch_output_recovery::{
    PatchOutputRecoveryClassification as Class, PatchOutputRecoveryInspection as Inspection,
    PatchOutputRecoveryPlan as Plan, PatchOutputState, inspect_patch_output_operation,
    plan_patch_output_recovery, resume_patch_output, rollback_patch_output,
};
use eframe::egui;

use crate::ui::components as widgets;

const JOURNAL_PREFIX: &str = ".emuwiz-patch-output-";
const MAX_JOURNALS: usize = 512;

#[derive(Default)]
pub(super) struct RecoveryPageState {
    selected_game: Option<PathBuf>,
    loading: Option<Receiver<Vec<Record>>>,
    records: Vec<Record>,
    confirmation: Option<(usize, Action)>,
    approved: bool,
    running: Option<Receiver<(Action, Result<(), String>)>>,
    status: Option<(bool, String)>,
}

enum Record {
    Inspected {
        inspection: Inspection,
        plan: Option<Plan>,
        plan_error: Option<String>,
    },
    Blocked {
        journal: PathBuf,
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    Resume,
    Rollback,
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut RecoveryPageState, game: &Path) {
    select_game(state, game, ui.ctx().clone());
    poll(state, ui.ctx());

    widgets::card(ui, |ui| {
        ui.heading("Interrupted patch outputs");
        ui.label("EmuWiz checks patch-operation journals beside this game. Assessment only reads files; age alone never makes an output disposable.");
        if state.loading.is_some() {
            ui.spinner();
            ui.label("Checking recorded patch operations…");
        } else if ui.button("Refresh patch recovery check").clicked() {
            start_scan(state, game.to_path_buf(), ui.ctx().clone());
        }
        if let Some((success, message)) = &state.status {
            widgets::banner(
                ui,
                if *success {
                    "Recovery finished"
                } else {
                    "Recovery blocked"
                },
                message,
                if *success {
                    widgets::StatusTone::Success
                } else {
                    widgets::StatusTone::Warning
                },
            );
        }
        if state.loading.is_none() && state.records.is_empty() {
            ui.label("No patch-output journals were found beside this game.");
        }
        for index in 0..state.records.len() {
            show_record(ui, state, index);
        }
    });
}

fn select_game(state: &mut RecoveryPageState, game: &Path, context: egui::Context) {
    if state.selected_game.as_deref() == Some(game) {
        return;
    }
    *state = RecoveryPageState {
        selected_game: Some(game.to_path_buf()),
        ..RecoveryPageState::default()
    };
    start_scan(state, game.to_path_buf(), context);
}

fn start_scan(state: &mut RecoveryPageState, game: PathBuf, context: egui::Context) {
    state.confirmation = None;
    state.approved = false;
    state.records.clear();
    state.status = None;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let records = inspect_game_directory(&game);
        let _ = sender.send(records);
        context.request_repaint();
    });
    state.loading = Some(receiver);
}

fn inspect_game_directory(game: &Path) -> Vec<Record> {
    let Some(directory) = game.parent() else {
        return Vec::new();
    };
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            return vec![Record::Blocked {
                journal: directory.to_path_buf(),
                reason: format!("Cannot inspect the game's patch-operation directory: {error}"),
            }];
        }
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(JOURNAL_PREFIX) && name.ends_with(".json"))
        })
        .collect::<Vec<_>>();
    paths.sort();
    let truncated = paths.len() > MAX_JOURNALS;
    paths.truncate(MAX_JOURNALS);
    let mut records = Vec::new();
    for journal in paths {
        match inspect_patch_output_operation(&journal) {
            Ok(inspection) if inspection.journal.source_path == game => {
                let eligible = matches!(
                    inspection.classification,
                    Class::SafeToResume | Class::PublicationLikelyComplete
                );
                let (plan, plan_error) = if eligible {
                    match plan_patch_output_recovery(&journal) {
                        Ok(plan) => (Some(plan), None),
                        Err(error) => (None, Some(error.to_string())),
                    }
                } else {
                    (None, None)
                };
                records.push(Record::Inspected {
                    inspection,
                    plan,
                    plan_error,
                });
            }
            Ok(_) => {} // The journal belongs to a different input game.
            Err(error) => records.push(Record::Blocked {
                journal,
                reason: error.to_string(),
            }),
        }
    }
    if truncated {
        records.push(Record::Blocked {
            journal: directory.to_path_buf(),
            reason: format!("More than {MAX_JOURNALS} patch journals were found; this view is incomplete. Refresh after reviewing the directory."),
        });
    }
    records
}

fn poll(state: &mut RecoveryPageState, context: &egui::Context) {
    if let Some(receiver) = state.loading.take() {
        match receiver.try_recv() {
            Ok(records) => state.records = records,
            Err(TryRecvError::Empty) => state.loading = Some(receiver),
            Err(TryRecvError::Disconnected) => {
                state.status = Some((
                    false,
                    "The read-only inspection worker stopped before finishing.".into(),
                ))
            }
        }
    }
    if let Some(receiver) = state.running.take() {
        match receiver.try_recv() {
            Ok((action, result)) => {
                state.approved = false;
                state.confirmation = None;
                match result {
                    Ok(()) => {
                        let message = match action {
                            Action::Resume => "The reviewed output recovery completed. Original ROM, patch, backup and Undo records were retained.".into(),
                            Action::Rollback => "Only this operation's verified output artifacts were removed. The original ROM, patch, backup and Undo records were retained; its journal remains as history.".into(),
                        };
                        if let Some(game) = state.selected_game.clone() {
                            start_scan(state, game, context.clone());
                        }
                        state.status = Some((true, message));
                    }
                    Err(error) => {
                        state.status = Some((
                            false,
                            format!(
                                "Core refused the operation after rechecking its journal and filesystem evidence: {error}"
                            ),
                        ))
                    }
                }
            }
            Err(TryRecvError::Empty) => state.running = Some(receiver),
            Err(TryRecvError::Disconnected) => {
                state.status = Some((
                    false,
                    "The recovery worker stopped before reporting a result.".into(),
                ))
            }
        }
    }
}

fn show_record(ui: &mut egui::Ui, state: &mut RecoveryPageState, index: usize) {
    let record = match &state.records[index] {
        Record::Blocked { journal, reason } => Record::Blocked {
            journal: journal.clone(),
            reason: reason.clone(),
        },
        Record::Inspected {
            inspection,
            plan,
            plan_error,
        } => Record::Inspected {
            inspection: inspection.clone(),
            plan: plan.clone(),
            plan_error: plan_error.clone(),
        },
    };
    match record {
        Record::Blocked { journal, reason } => {
            ui.separator();
            widgets::status_badge(
                ui,
                "Blocked · ownership not verified",
                widgets::StatusTone::Blocked,
            );
            ui.label(format!("Record: {}", journal.display()));
            ui.label(reason);
            ui.label("Nothing can be recovered or removed from this record.");
        }
        Record::Inspected {
            inspection,
            plan,
            plan_error,
        } => {
            let journal = &inspection.journal;
            let (label, tone) =
                classification_label(&inspection.classification, inspection.destination_matches);
            ui.separator();
            widgets::status_badge(ui, label, tone);
            ui.label(format!(
                "Recorded operation: {:?} · {}",
                journal.state, journal.operation_id
            ));
            ui.label(format!(
                "Original game input: {}",
                journal.source_path.display()
            ));
            ui.label(format!(
                "Patch: {} · {:?}",
                journal.patch_path.display(),
                journal.patch_format
            ));
            ui.label(format!(
                "Destination: {}",
                journal.destination_path.display()
            ));
            ui.label(format!(
                "Temporary output: {}",
                journal.temporary_output_path.display()
            ));
            ui.label(format!(
                "Operation journal (retained as history): {}",
                inspection.journal_path.display()
            ));
            ui.label(format!(
                "Provenance artifact: {}",
                journal.provenance_path.display()
            ));
            ui.label(format!(
                "Provenance ownership hash recorded: {}",
                yes_no(journal.provenance_sha256.is_some())
            ));
            if let Some(backup) = journal.backup_path.as_deref() {
                ui.label(format!("Recorded backup (preserved): {}", backup.display()));
            }
            if let (Some(size), Some(hash)) = (
                journal.expected_output_size,
                journal.expected_output_sha256.as_deref(),
            ) {
                ui.label(format!(
                    "Expected derived output: {size} bytes · SHA-256 {hash}"
                ));
            }
            ui.label(format!(
                "Source and patch still match their recorded hashes: {} / {}",
                yes_no(inspection.source_matches),
                yes_no(inspection.patch_matches)
            ));
            ui.label(format!(
                "Temporary output matches the recorded output: {} · destination matches: {}",
                yes_no(inspection.temporary_matches),
                yes_no(inspection.destination_matches)
            ));
            ui.label(format!(
                "Recorded operation state: {}",
                state_label(journal.state)
            ));
            if let Some(reason) = journal.failure_reason.as_deref() {
                ui.label(format!("Recorded failure: {reason}"));
            }
            ui.label(action_guidance(
                &inspection.classification,
                inspection.temporary_matches,
                inspection.destination_matches,
            ));
            if let Some(error) = plan_error {
                widgets::banner(
                    ui,
                    "Recovery plan unavailable",
                    &error,
                    widgets::StatusTone::Blocked,
                );
            }
            if let Some((selected, action)) = state.confirmation
                && selected == index
            {
                let description = confirmation_text(action, &inspection.classification, journal);
                widgets::banner(
                    ui,
                    "Confirm filesystem change",
                    &description,
                    widgets::StatusTone::Warning,
                );
                ui.checkbox(
                    &mut state.approved,
                    "I approve this exact recovery action for this game and output",
                );
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            state.approved && state.running.is_none(),
                            egui::Button::new(confirm_label(action)),
                        )
                        .clicked()
                    {
                        if let Some(plan) = plan.clone() {
                            start_action(state, action, plan, ui.ctx().clone());
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        cancel_confirmation(state);
                    }
                });
            } else if state.running.is_none() {
                for action in offered_actions(&inspection.classification, plan.is_some()) {
                    if ui.button(action_button_label(action)).clicked() {
                        state.confirmation = Some((index, action));
                        state.approved = false;
                    }
                }
            }
        }
    }
}

fn start_action(state: &mut RecoveryPageState, action: Action, plan: Plan, context: egui::Context) {
    if !state.approved || state.running.is_some() {
        return;
    }
    let (sender, receiver) = mpsc::channel();
    state.running = Some(receiver);
    state.confirmation = None;
    state.approved = false;
    std::thread::spawn(move || {
        let result = match action {
            Action::Resume => resume_patch_output(&plan),
            Action::Rollback => rollback_patch_output(&plan),
        }
        .map_err(|error| error.to_string());
        let _ = sender.send((action, result));
        context.request_repaint();
    });
}

fn cancel_confirmation(state: &mut RecoveryPageState) {
    state.confirmation = None;
    state.approved = false;
}

fn classification_label(
    class: &Class,
    destination_matches: bool,
) -> (&'static str, widgets::StatusTone) {
    match class {
        Class::SafeToResume => (
            "Interrupted · verified temporary output",
            widgets::StatusTone::Warning,
        ),
        Class::PublicationLikelyComplete => (
            "Published · output hash verified",
            widgets::StatusTone::Success,
        ),
        Class::Completed if destination_matches => (
            "Completed · output still matches",
            widgets::StatusTone::Success,
        ),
        Class::Completed => (
            "Completed in journal · output changed since",
            widgets::StatusTone::Warning,
        ),
        Class::Failed => (
            "Failed · output may be incomplete",
            widgets::StatusTone::Warning,
        ),
        Class::RolledBack => ("Rolled back · history retained", widgets::StatusTone::Info),
        Class::NeedsReview | Class::UnsafeToResume => {
            ("Blocked · needs review", widgets::StatusTone::Blocked)
        }
    }
}

fn offered_actions(class: &Class, plan_available: bool) -> Vec<Action> {
    if !plan_available {
        return Vec::new();
    }
    match class {
        Class::SafeToResume => vec![Action::Resume],
        Class::PublicationLikelyComplete => vec![Action::Resume, Action::Rollback],
        _ => Vec::new(),
    }
}

fn action_button_label(action: Action) -> &'static str {
    match action {
        Action::Resume => "Review and recover output",
        Action::Rollback => "Review output removal",
    }
}

fn confirm_label(action: Action) -> &'static str {
    match action {
        Action::Resume => "Confirm recovery",
        Action::Rollback => "Confirm removal",
    }
}

fn confirmation_text(
    action: Action,
    class: &Class,
    journal: &archivefs_core::patch_output_recovery::PatchOutputJournal,
) -> String {
    match action {
        Action::Resume if *class == Class::SafeToResume => format!(
            "Core will recheck this journal and source. It will publish the recorded verified temporary output to {}, finalize the journal and remove only that temporary file. The original ROM, patch, backups and Undo records remain.",
            journal.destination_path.display()
        ),
        Action::Resume => format!(
            "Core will recheck this journal and destination. It will keep the already verified output at {}, finish the journal and remove only the operation's temporary file. The original ROM, patch, backups and Undo records remain.",
            journal.destination_path.display()
        ),
        Action::Rollback => format!(
            "Core will recheck ownership and remove only this operation's verified output at {} and its recorded temporary output. It removes the provenance artifact only if its recorded SHA-256 still proves ownership. The original ROM, patch, backups and Undo records remain; the journal is retained.",
            journal.destination_path.display()
        ),
    }
}

fn action_guidance(
    class: &Class,
    temporary_matches: bool,
    destination_matches: bool,
) -> &'static str {
    match class {
        Class::SafeToResume if temporary_matches => {
            "Mr Wiz: The output is incomplete because it has not reached its destination. This temporary file matches the recorded output, and the destination is still free. You can recover it after reviewing the exact paths."
        }
        Class::PublicationLikelyComplete if destination_matches => {
            "Mr Wiz: The destination matches the verified output, but the journal did not finish. You can keep and finalize this output, or leave it untouched. No automatic cleanup runs."
        }
        Class::Completed if destination_matches => {
            "Mr Wiz: This operation completed and the current destination still matches its recorded output hash. Keep it as verified output; no recovery action is needed."
        }
        Class::Completed => {
            "Mr Wiz: The journal says this completed, but the destination no longer matches its recorded output hash. Keep the file for review; recovery and removal are blocked."
        }
        Class::Failed => {
            "Mr Wiz: The operation stopped before verified completion. Any partial file is left untouched because ownership or completeness is not proven."
        }
        Class::RolledBack => {
            "Mr Wiz: This operation was already rolled back. Its journal remains as history; nothing else will be removed."
        }
        Class::NeedsReview | Class::UnsafeToResume => {
            "Mr Wiz: The recorded state and current files do not prove a safe action. Keep every file and review the journal; recovery is blocked."
        }
        Class::SafeToResume => {
            "Mr Wiz: The expected temporary output is not verified. Nothing can be recovered or removed safely."
        }
        Class::PublicationLikelyComplete => {
            "Mr Wiz: The destination is not verified against the recorded output. Keep it untouched; recovery is blocked."
        }
    }
}

fn state_label(state: PatchOutputState) -> &'static str {
    match state {
        PatchOutputState::Planned => "planned",
        PatchOutputState::Preparing => "preparing output",
        PatchOutputState::Prepared => "temporary output prepared",
        PatchOutputState::VerifyingTemporary => "verifying temporary output",
        PatchOutputState::Publishing => "publishing output",
        PatchOutputState::Published => "output published",
        PatchOutputState::VerifyingPublished => "verifying published output",
        PatchOutputState::Completed => "completed and verified",
        PatchOutputState::Failed => "failed before completion",
        PatchOutputState::RollingBack => "rollback started",
        PatchOutputState::RolledBack => "rolled back",
        PatchOutputState::RollbackFailed => "rollback failed",
        PatchOutputState::NeedsReview => "needs review",
        PatchOutputState::UnsafeToResume => "unsafe to resume",
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::patch_output_recovery::{
        PatchOutputIntent, PreparedPatchOutput, publish_durable_patch_output,
    };
    use archivefs_core::standalone_patch::StandalonePatchFormat;
    use sha2::{Digest, Sha256};

    struct Fixture {
        root: tempfile::TempDir,
        source: PathBuf,
        output: PathBuf,
        journal: PathBuf,
        temp: PathBuf,
    }

    fn hash(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("game.rom");
        let patch = root.path().join("hack.ips");
        let output = root.path().join("game.patched.rom");
        let provenance = root.path().join("game.patched.rom.provenance.json");
        let source_bytes = b"original ROM bytes";
        let patch_bytes = b"patch instructions";
        let output_bytes = b"verified derived ROM";
        std::fs::write(&source, source_bytes).unwrap();
        std::fs::write(&patch, patch_bytes).unwrap();
        let result = publish_durable_patch_output(
            PatchOutputIntent {
                patch_format: StandalonePatchFormat::Ips,
                source_path: source.clone(),
                source_size: source_bytes.len() as u64,
                source_sha256: hash(source_bytes),
                patch_path: patch.clone(),
                patch_size: patch_bytes.len() as u64,
                patch_sha256: hash(patch_bytes),
                destination_path: output.clone(),
                expected_output_size: Some(output_bytes.len() as u64),
                provenance_path: provenance.clone(),
            },
            || {
                Ok(PreparedPatchOutput {
                    bytes: output_bytes.to_vec(),
                    application: "fixture".into(),
                })
            },
            |_| {
                std::fs::write(&provenance, b"provenance").map_err(|e| {
                    archivefs_core::patch_output_recovery::PatchOutputRecoveryError::Io(
                        e.to_string(),
                    )
                })
            },
        )
        .unwrap();
        let journal_value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&result.journal_path).unwrap()).unwrap();
        let temp = PathBuf::from(journal_value["temporary_output_path"].as_str().unwrap());
        Fixture {
            root,
            source,
            output,
            journal: result.journal_path,
            temp,
        }
    }

    fn make_interrupted(fixture: &Fixture) {
        std::fs::rename(&fixture.output, &fixture.temp).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&fixture.journal).unwrap()).unwrap();
        value["state"] = serde_json::Value::String("prepared".into());
        std::fs::write(&fixture.journal, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    }

    fn inspected(fixture: &Fixture) -> Inspection {
        inspect_patch_output_operation(&fixture.journal).unwrap()
    }

    #[test]
    fn completed_output_is_distinguished_from_verified_interrupted_temp() {
        let complete = fixture();
        let complete_view = inspect_game_directory(&complete.source);
        assert!(
            matches!(&complete_view[0], Record::Inspected { inspection, plan: None, .. }
            if inspection.classification == Class::Completed && inspection.destination_matches)
        );
        let interrupted = fixture();
        make_interrupted(&interrupted);
        let interrupted_view = inspect_game_directory(&interrupted.source);
        assert!(
            matches!(&interrupted_view[0], Record::Inspected { inspection, plan: Some(_), ..}
            if inspection.classification == Class::SafeToResume && inspection.temporary_matches && !inspection.destination_matches)
        );
    }

    #[test]
    fn missing_or_changed_source_is_blocked() {
        let missing = fixture();
        make_interrupted(&missing);
        std::fs::remove_file(&missing.source).unwrap();
        assert_eq!(inspected(&missing).classification, Class::UnsafeToResume);
        let changed = fixture();
        make_interrupted(&changed);
        std::fs::write(&changed.source, b"different ROM").unwrap();
        assert_eq!(inspected(&changed).classification, Class::UnsafeToResume);
    }

    #[test]
    fn destination_not_owned_by_journal_is_never_offered_for_recovery() {
        let fixture = fixture();
        make_interrupted(&fixture);
        std::fs::write(&fixture.output, b"some other output").unwrap();
        let records = inspect_game_directory(&fixture.source);
        assert!(
            matches!(&records[0], Record::Inspected { inspection, plan: None, .. }
            if inspection.classification == Class::NeedsReview)
        );
        assert!(offered_actions(&Class::NeedsReview, false).is_empty());
    }

    #[test]
    fn only_core_reviewed_classes_expose_supported_actions() {
        assert_eq!(
            offered_actions(&Class::SafeToResume, true),
            vec![Action::Resume]
        );
        assert_eq!(
            offered_actions(&Class::PublicationLikelyComplete, true),
            vec![Action::Resume, Action::Rollback]
        );
        assert!(offered_actions(&Class::Completed, true).is_empty());
        assert!(offered_actions(&Class::NeedsReview, true).is_empty());
        assert!(offered_actions(&Class::SafeToResume, false).is_empty());
    }

    #[test]
    fn stale_recovery_plan_is_refused_by_core() {
        let fixture = fixture();
        make_interrupted(&fixture);
        let plan = plan_patch_output_recovery(&fixture.journal).unwrap();
        std::fs::write(&fixture.temp, b"changed after review").unwrap();
        assert!(resume_patch_output(&plan).is_err());
        assert!(fixture.output.exists() == false);
    }

    #[test]
    fn wrong_game_selection_hides_other_games_journal() {
        let fixture = fixture();
        let other_game = fixture.root.path().join("another-game.rom");
        std::fs::write(&other_game, b"another source").unwrap();
        assert!(inspect_game_directory(&other_game).is_empty());
    }

    #[test]
    fn cancellation_and_rapid_selection_drop_approval_and_old_results() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("one.rom");
        let second = root.path().join("two.rom");
        std::fs::write(&first, b"one").unwrap();
        std::fs::write(&second, b"two").unwrap();
        let context = egui::Context::default();
        let mut state = RecoveryPageState::default();
        select_game(&mut state, &first, context.clone());
        state.confirmation = Some((0, Action::Resume));
        state.approved = true;
        cancel_confirmation(&mut state);
        select_game(&mut state, &second, context);
        assert_eq!(state.selected_game.as_deref(), Some(second.as_path()));
        assert!(state.confirmation.is_none());
        assert!(!state.approved);
        assert!(state.records.is_empty());
        assert!(state.loading.is_some());
    }
}
