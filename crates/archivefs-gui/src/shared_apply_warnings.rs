//! Shared-transaction Apply warnings, shown with the result of every adapter
//! that presents a `SharedApplyResult`.
//!
//! `SharedApplyEntry.warnings` is written while a file is being applied, so a
//! later failure can leave a warning describing a replacement that did not
//! finish. Those lines say so rather than stating it as fact.

use archivefs_core::patch_manager::{SharedApplyOutcome, SharedApplyResult};
use eframe::egui;

/// One plain line per warning, naming the file it belongs to.
pub(crate) fn lines(result: &SharedApplyResult) -> Vec<String> {
    result
        .journal
        .entries
        .iter()
        .flat_map(|entry| {
            let path = &entry.plan_entry.destination_relative_path.display;
            let finished = matches!(
                entry.outcome,
                SharedApplyOutcome::InstalledNew | SharedApplyOutcome::ReplacedExisting
            );
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

/// For results drawn directly in a page.
pub(crate) fn show(ui: &mut egui::Ui, lines: &[String]) {
    for line in lines {
        ui.colored_label(ui.visuals().warn_fg_color, line);
    }
}

/// For adapters that report their result as a single status string.
pub(crate) fn append(text: &mut String, result: &SharedApplyResult) {
    for line in lines(result) {
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
        SharedApplyEntry, SharedApplyJournal, SharedApplyStatus, SharedPlanEntry,
        SharedTransactionPath,
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
    fn a_warning_on_an_unfinished_entry_is_not_stated_as_fact() {
        let mut result = warned_result();
        result.journal.entries[0].outcome = SharedApplyOutcome::WriteFailed;
        let lines = lines(&result);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains(WARNING));
        assert!(lines[0].contains("did not finish"), "{}", lines[0]);
    }
}
