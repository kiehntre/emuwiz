//! Shared-transaction Apply warnings, shown with the result of every adapter
//! that presents a `SharedApplyResult`.
//!
//! `SharedApplyEntry.warnings` is written while a file is being applied, so a
//! later failure can leave a warning describing a replacement that did not
//! finish. Those lines say so rather than stating it as fact.
//!
//! Texture packs also roll back automatically after a partial failure. That
//! run has its own journal, so the Apply journal still reads `ReplacedExisting`
//! afterwards; [`lines_after_rollback`] reads the rollback's per-file outcome.

use archivefs_core::patch_manager::{
    SharedApplyEntry, SharedApplyOutcome, SharedApplyResult, SharedApplyStatus,
    SharedRollbackEntry, SharedRollbackOutcome, SharedRollbackPermissions, SharedRollbackResult,
};
use eframe::egui;
use std::path::Path;

/// One plain line per warning, naming the file it belongs to.
pub(crate) fn lines(result: &SharedApplyResult) -> Vec<String> {
    result
        .journal
        .entries
        .iter()
        .flat_map(|entry| {
            let path = &entry.plan_entry.destination_relative_path.display;
            let finished = finished(entry);
            entry.warnings.iter().map(move |warning| {
                if finished {
                    format!("{path}: {warning}")
                } else {
                    format!(
                        "{path}: {warning} (this file's Apply did not finish, so check it before relying on this)"
                    )
                }
            })
        })
        .collect()
}

/// The same lines for an adapter that rolls back automatically: `rollback` is
/// what it reported, and `None` means no rollback ran.
pub(crate) fn lines_after_rollback(
    result: &SharedApplyResult,
    rollback: Option<&SharedRollbackResult>,
) -> Vec<String> {
    if result.journal.status == SharedApplyStatus::Success {
        return lines(result);
    }
    result
        .journal
        .entries
        .iter()
        .filter(|entry| !entry.warnings.is_empty())
        .flat_map(|entry| {
            let path = &entry.plan_entry.destination_relative_path.display;
            let rolled_back = rollback.and_then(|rollback| rollback_entry(entry, rollback));
            if let Some(restored) = rolled_back
                .filter(|entry| entry.outcome == SharedRollbackOutcome::RestoredBackup)
            {
                // The only engine warning is about the replacement's mode, so
                // the restored file's own permissions stand in for it.
                let permissions = restored.permissions.unwrap_or_else(|| {
                    SharedRollbackPermissions::for_recorded_mode(entry.destination_pre_mode)
                });
                return vec![format!(
                    "{path}: automatic rollback restored the original contents, so the replacement is gone. Permissions: {}.",
                    permissions.summary()
                )];
            }
            let finished = finished(entry);
            entry
                .warnings
                .iter()
                .map(|warning| match (finished, rollback) {
                    (false, _) => format!(
                        "{path}: {warning} (this file's Apply did not finish, so check it before relying on this)"
                    ),
                    (true, None) => format!(
                        "{path}: {warning} (automatic rollback did not run, so this replacement was left in place)"
                    ),
                    (true, Some(_)) => format!(
                        "{path}: {warning} (automatic rollback did not confirm this file was restored, so it may still be the replacement; check it before relying on this)"
                    ),
                })
                .collect()
        })
        .collect()
}

fn finished(entry: &SharedApplyEntry) -> bool {
    matches!(
        entry.outcome,
        SharedApplyOutcome::InstalledNew | SharedApplyOutcome::ReplacedExisting
    )
}

fn rollback_entry<'a>(
    entry: &SharedApplyEntry,
    rollback: &'a SharedRollbackResult,
) -> Option<&'a SharedRollbackEntry> {
    let plan = &entry.plan_entry;
    let expected =
        Path::new(&plan.destination_root.display).join(&plan.destination_relative_path.display);
    rollback.preview.entries.iter().find(|candidate| {
        candidate
            .destination
            .as_ref()
            .is_some_and(|destination| Path::new(&destination.display) == expected)
    })
}

/// For results drawn directly in a page.
pub(crate) fn show(ui: &mut egui::Ui, lines: &[String]) {
    for line in lines {
        ui.colored_label(ui.visuals().warn_fg_color, line);
    }
}

/// For adapters that report their result as a single status string.
pub(crate) fn append(text: &mut String, result: &SharedApplyResult) {
    append_lines(text, lines(result));
}

pub(crate) fn append_lines(text: &mut String, lines: Vec<String>) {
    for line in lines {
        text.push('\n');
        text.push_str(&line);
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::transaction_outcome::test_support::rendered_text;
    use archivefs_core::patch_manager::{
        PreviewAdapter, PreviewDestinationState, PreviewProposedAction, SharedApplyContext,
        SharedApplyJournal, SharedPlanEntry, SharedRollbackPreview, SharedTransactionPath,
    };
    use std::path::Path;

    pub(crate) const WARNING: &str =
        "existing permissions 4755 were not kept; the replaced file is owner-only 0600";
    pub(crate) const WARNED_FILE: &str = "pack/replaced.bin";

    /// A finished replacement whose only entry carries the engine's warning.
    pub(crate) fn warned_result() -> SharedApplyResult {
        let path = |value: &str| SharedTransactionPath::from_path(Path::new(value));
        SharedApplyResult {
            journal: SharedApplyJournal {
                schema_version: 1,
                operation_id: "warned-op".into(),
                plan_id: "warned-plan".into(),
                timestamp_unix_seconds: 1,
                context: SharedApplyContext {
                    adapter: PreviewAdapter::RetroArch,
                    selected_archive: path("/roms/game.bin"),
                    verified_game_identity: "game".into(),
                    profile_id: "profile".into(),
                    source_mode: "test".into(),
                },
                approved_source_root: path("/disposable/source"),
                destination_root: path("/disposable/destination"),
                created_root_directories: Vec::new(),
                dry_run: false,
                entries: vec![SharedApplyEntry {
                    plan_entry: SharedPlanEntry {
                        adapter: PreviewAdapter::RetroArch,
                        selected_archive: path("/roms/game.bin"),
                        verified_game_identity: "game".into(),
                        source_path: path("/disposable/source/replaced.bin"),
                        source_digest: "a".repeat(64),
                        destination_root: path("/disposable/destination"),
                        destination_relative_path: path(WARNED_FILE),
                        destination_pre_state: PreviewDestinationState::RegularFileDifferent,
                        destination_pre_digest: None,
                        proposed_action: PreviewProposedAction::Replace,
                        backup_required: true,
                        parent_creation_approved: false,
                        content_verification: None,
                    },
                    destination_existed_before_apply: Some(true),
                    destination_parent_existed_before_apply: Some(true),
                    observed_source_digest: None,
                    observed_destination_digest: None,
                    backup_path: None,
                    backup_digest: None,
                    destination_pre_mode: Some(0o4755),
                    temporary_path: None,
                    final_destination_digest: None,
                    created_directories: Vec::new(),
                    replacement_approved: true,
                    verification_succeeded: true,
                    outcome: SharedApplyOutcome::ReplacedExisting,
                    publication: Default::default(),
                    stages: Vec::new(),
                    warnings: vec![WARNING.into()],
                    failures: Vec::new(),
                }],
                status: SharedApplyStatus::Success,
                rollback_operation_id: None,
            },
            journal_path: None,
            journal_failure: None,
        }
    }

    /// [`warned_result`] after a later file failed: the status a texture pack's
    /// automatic rollback runs for, with the replaced entry still journalled
    /// as `ReplacedExisting`.
    pub(crate) fn partly_failed_result() -> SharedApplyResult {
        let mut result = warned_result();
        result.journal.status = SharedApplyStatus::PartialFailure;
        result
    }

    /// A rollback of [`warned_result`]'s file ending in `outcome`.
    pub(crate) fn rollback_with(
        outcome: SharedRollbackOutcome,
        status: SharedApplyStatus,
    ) -> SharedRollbackResult {
        let destination = Path::new("/disposable/destination").join(WARNED_FILE);
        SharedRollbackResult {
            preview: SharedRollbackPreview {
                schema_version: 1,
                preview_id: "rollback".into(),
                journal_path: SharedTransactionPath::from_path(Path::new("/disposable/j.json")),
                original_operation_id: "warned-op".into(),
                destination_root: SharedTransactionPath::from_path(Path::new(
                    "/disposable/destination",
                )),
                entries: vec![SharedRollbackEntry {
                    destination: Some(SharedTransactionPath::from_path(&destination)),
                    backup: None,
                    expected_installed_digest: None,
                    observed_destination_digest: None,
                    observed_backup_digest: None,
                    outcome,
                    failure: None,
                    permissions: Some(SharedRollbackPermissions::NotRestorable { mode: 0o4755 }),
                }],
                available: true,
            },
            journal_path: None,
            status,
        }
    }

    pub(crate) fn succeeded_rollback() -> SharedRollbackResult {
        rollback_with(
            SharedRollbackOutcome::RestoredBackup,
            SharedApplyStatus::Success,
        )
    }

    pub(crate) fn failed_rollback() -> SharedRollbackResult {
        rollback_with(SharedRollbackOutcome::Failed, SharedApplyStatus::Failed)
    }

    /// The contents are back, nothing claims the replacement stayed, and the
    /// permission loss that survives rollback is still stated.
    pub(crate) fn assert_rollback_succeeded(text: &str) {
        assert!(
            text.contains(&format!(
                "{WARNED_FILE}: automatic rollback restored the original contents"
            )),
            "{text}"
        );
        assert!(text.contains("the replacement is gone"), "{text}");
        assert!(
            text.contains("owner-only 0600; the original 4755 is not restored automatically"),
            "{text}"
        );
        assert!(!text.contains("the replaced file is owner-only"), "{text}");
    }

    /// Neither outcome is claimed, and the recorded warning is kept.
    pub(crate) fn assert_rollback_failed(text: &str) {
        assert!(text.contains(WARNING), "{text}");
        assert!(text.contains("did not confirm"), "{text}");
        assert!(text.contains("may still be the replacement"), "{text}");
        assert!(!text.contains("restored the original"), "{text}");
    }

    /// The Apply's real outcome: the replacement is there, and no rollback or
    /// success is invented.
    pub(crate) fn assert_rollback_did_not_run(text: &str) {
        assert!(text.contains(WARNING), "{text}");
        assert!(
            text.contains("rollback did not run, so this replacement was left in place"),
            "{text}"
        );
        assert!(!text.contains("restored"), "{text}");
    }

    /// Everything one frame of `content` draws.
    pub(crate) fn render(mut content: impl FnMut(&mut egui::Ui)) -> String {
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| content(ui));
        });
        rendered_text(&output)
    }

    pub(crate) fn assert_shows_warning(text: &str) {
        assert!(
            text.contains(&format!("{WARNED_FILE}: {WARNING}")),
            "warning missing from: {text}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn a_result_without_warnings_adds_nothing() {
        let mut result = warned_result();
        result.journal.entries[0].warnings.clear();
        assert!(lines(&result).is_empty());
        let mut status = "Saved.".to_string();
        append(&mut status, &result);
        assert_eq!(status, "Saved.");
    }

    #[test]
    fn every_warning_is_kept_and_names_its_file() {
        let mut result = warned_result();
        result.journal.entries.push({
            let mut entry = result.journal.entries[0].clone();
            entry.plan_entry.destination_relative_path.display = "pack/other.bin".into();
            entry.warnings = vec!["second".into(), "third".into()];
            entry
        });
        assert_eq!(
            lines(&result),
            vec![
                format!("{WARNED_FILE}: {WARNING}"),
                "pack/other.bin: second".to_string(),
                "pack/other.bin: third".to_string(),
            ]
        );
    }

    #[test]
    fn rollback_wording_follows_each_files_own_outcome() {
        let result = partly_failed_result();
        let text = |rollback: Option<&SharedRollbackResult>| {
            lines_after_rollback(&result, rollback).join("\n")
        };
        assert_rollback_succeeded(&text(Some(&succeeded_rollback())));
        assert_rollback_failed(&text(Some(&failed_rollback())));
        assert_rollback_did_not_run(&text(None));
    }

    #[test]
    fn an_overall_rollback_success_does_not_vouch_for_an_unrestored_file() {
        let result = partly_failed_result();
        for outcome in [
            SharedRollbackOutcome::Retained,
            SharedRollbackOutcome::Available,
            SharedRollbackOutcome::DestinationChanged,
            SharedRollbackOutcome::NoChangeRequired,
        ] {
            let rollback = rollback_with(outcome, SharedApplyStatus::Success);
            assert_rollback_failed(&lines_after_rollback(&result, Some(&rollback)).join("\n"));
        }
        // A rollback that never reached this file proves nothing about it.
        let mut rollback = succeeded_rollback();
        rollback.preview.entries[0].destination = None;
        assert_rollback_failed(&lines_after_rollback(&result, Some(&rollback)).join("\n"));
    }

    #[test]
    fn a_restored_entry_that_failed_apply_is_reported_as_restored() {
        let mut result = partly_failed_result();
        result.journal.entries[0].outcome = SharedApplyOutcome::VerificationFailed;
        let lines = lines_after_rollback(&result, Some(&succeeded_rollback()));
        assert_rollback_succeeded(&lines.join("\n"));
    }

    #[test]
    fn an_unfinished_entry_never_claims_rollback_or_replacement_state() {
        let mut result = partly_failed_result();
        result.journal.status = SharedApplyStatus::Failed;
        result.journal.entries[0].outcome = SharedApplyOutcome::WriteFailed;
        for rollback in [None, Some(failed_rollback())] {
            let text = lines_after_rollback(&result, rollback.as_ref()).join("\n");
            assert!(text.contains("did not finish"), "{text}");
            assert!(!text.contains("left in place"), "{text}");
        }
    }

    #[test]
    fn a_successful_apply_keeps_the_plain_warning() {
        assert_eq!(
            lines_after_rollback(&warned_result(), None),
            lines(&warned_result())
        );
    }

    #[test]
    fn a_warning_on_an_unfinished_entry_is_not_stated_as_fact() {
        let mut result = warned_result();
        result.journal.entries[0].outcome = SharedApplyOutcome::WriteFailed;
        let lines = lines(&result);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains(WARNING));
        assert!(lines[0].contains("did not finish"), "{}", lines[0]);
    }
}
