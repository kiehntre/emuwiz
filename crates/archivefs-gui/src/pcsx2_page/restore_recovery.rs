//! Explicit refresh and restart Undo; discovery never creates a data directory.
use super::*;
use archivefs_core::memory_card_inventory::restore_guard::{
    Ps2DiscoveryProblem, Ps2RecoveryOutcome, Ps2RestoreJournalSummary, load_ps2_restore_journal,
    ps2_card_binding,
};

#[derive(Clone, Default)]
struct History {
    rows: Vec<Ps2RestoreJournalSummary>,
    /// Why `rows` may not be the whole inventory; empty only when it is.
    problems: Vec<Ps2DiscoveryProblem>,
    messages: Vec<String>,
}
impl History {
    /// Undo and interrupted-restore checks depend on the discovered set, so they
    /// are offered only when that set is known to be complete.
    fn incomplete(&self) -> bool {
        !self.problems.is_empty()
    }
    fn discover(dir: &Path) -> Self {
        let mut history = Self::default();
        history.refresh(dir);
        history
    }
    fn refresh(&mut self, dir: &Path) {
        let found = discover_ps2_restore_journals(dir);
        self.rows = found.records;
        self.problems = found.problems;
    }
}
fn history_id() -> egui::Id {
    egui::Id::new("ps2_restore_history_cache")
}
pub(super) fn invalidate(ui: &mut egui::Ui) {
    ui.data_mut(|data| data.remove::<History>(history_id()));
}

fn outcome_message(outcome: &Ps2RecoveryOutcome) -> String {
    match outcome {
        Ps2RecoveryOutcome::NothingToDo(_) => "No interrupted operation needs recovery.".into(),
        Ps2RecoveryOutcome::AbandonedBeforePublication | Ps2RecoveryOutcome::NotPublished => {
            "The card was not published. The backup was retained.".into()
        }
        Ps2RecoveryOutcome::ConfirmedPublished => {
            "The restored card was verified. Review Undo is available.".into()
        }
        Ps2RecoveryOutcome::UndoNotApplied => {
            "Undo had not changed the card. Review Undo is available again.".into()
        }
        Ps2RecoveryOutcome::UndoCompleted => "Undo completed before the interruption.".into(),
        Ps2RecoveryOutcome::NeedsAttention(detail) => {
            format!("Manual inspection required: {detail}")
        }
    }
}

pub(super) fn show(ui: &mut egui::Ui, advanced_mode: bool) {
    let dir = match default_ps2_restore_journal_dir() {
        Ok(dir) => dir,
        Err(error) => {
            ui.label(format!(
                "PS2 restore records cannot be located, so interrupted restores may be hidden: {}",
                psu_restore_error_message(&error)
            ));
            return;
        }
    };
    show_in(ui, advanced_mode, &dir);
}

fn show_in(ui: &mut egui::Ui, advanced_mode: bool, dir: &Path) {
    let mut history = ui
        .data_mut(|data| data.get_temp::<History>(history_id()))
        .unwrap_or_else(|| History::discover(dir));
    ui.horizontal(|ui| {
        ui.label("PS2 restore records");
        if widgets::action_button(ui, "Refresh records", widgets::ActionStyle::Secondary, true)
            .clicked()
        {
            history.refresh(dir);
        }
        if !history.incomplete()
            && history
                .rows
                .iter()
                .any(|row| row.needs_recovery && row.error.is_none())
            && widgets::action_button(
                ui,
                "Check interrupted restores",
                widgets::ActionStyle::Secondary,
                true,
            )
            .clicked()
        {
            let run = recover_all_interrupted_ps2_restores(dir, unix_now());
            history.messages = run
                .outcomes
                .into_iter()
                .map(|(path, outcome)| {
                    format!(
                        "{}: {}",
                        path.display(),
                        match outcome {
                            Ok(outcome) => outcome_message(&outcome),
                            Err(error) => psu_restore_error_message(&error),
                        }
                    )
                })
                .collect();
            history.refresh(dir);
        }
    });
    let incomplete = history.incomplete();
    if incomplete {
        ui.label("Warning: the restore record list below is INCOMPLETE. Do not treat it as the full recovery inventory; interrupted or failed restores may not be shown.");
        for problem in &history.problems {
            ui.label(format!("  - {problem}"));
        }
        ui.label("Undo and the interrupted-restore check are withheld until the list is complete, because they act on this list. Nothing was deleted or changed.");
        ui.label("To continue, fix the cause (permissions or a damaged folder), or, if older records were omitted, move older restore records out of the folder yourself (EmuWiz never deletes them), then press Refresh records.");
    }
    for row in &history.rows {
        if row.undo_available || row.needs_attention || row.needs_recovery {
            widgets::path_value(ui, "Restore record", &row.path);
            if let Some(error) = &row.error {
                ui.label(format!("Damaged record: {error}"));
                ui.label("Automatic recovery and Undo are blocked for this record. Preserve the record, card and backups for independent inspection. Other records can still be reviewed.");
            } else if row.undo_available {
                if let Some(card) = &row.card_path {
                    widgets::path_value(ui, "Card", card);
                }
                if incomplete {
                    ui.label("Undo is withheld for this record while the list is INCOMPLETE.");
                } else if widgets::action_button(
                    ui,
                    "Review Undo",
                    widgets::ActionStyle::Secondary,
                    advanced_mode,
                )
                .clicked()
                {
                    ui.data_mut(|data| {
                        data.insert_temp(
                            psu_restore_dialog_id(),
                            PsuRestoreDialogState::UndoConfirm(row.path.clone()),
                        )
                    });
                }
            } else {
                if let Some(detail) = &row.detail {
                    ui.label(detail);
                }
                ui.label(if row.needs_attention { "Manual inspection is required. Preserve all artifacts; no card write will be attempted automatically." } else { "Interrupted operation: check its recorded state before proceeding." });
            }
        }
    }
    for message in &history.messages {
        ui.label(message);
    }
    ui.data_mut(|data| data.insert_temp(history_id(), history));
}

pub(super) fn process_gate(card: &Path, save: &str) -> Result<(), Ps2PsuRestoreError> {
    let report = ProcScanQuiescence::new().report(&ps2_card_binding(card, save));
    if report.state
        == archivefs_core::save_snapshots::tree_restore::safety::EmulatorQuiescence::Unknown
    {
        return Err(Ps2PsuRestoreError::RecoveryRequired(format!(
            "Process inspection is incomplete. Unreadable live process IDs: {:?}. Close these processes or correct /proc access, then retry. The card was not changed.",
            report.unreadable_processes
        )));
    }
    Ok(())
}

/// True while the cached discovery is known to be incomplete.
fn undo_withheld(ui: &egui::Ui) -> bool {
    ui.data(|data| data.get_temp::<History>(history_id()))
        .is_some_and(|history| history.incomplete())
}

pub(super) fn confirm_undo(
    ui: &mut egui::Ui,
    path: &Path,
    advanced_mode: bool,
) -> Option<PsuRestoreDialogState> {
    let journal = match load_ps2_restore_journal(path) {
        Ok(journal) => journal,
        Err(error) => {
            ui.label(psu_restore_error_message(&error));
            return None;
        }
    };
    ui.heading("Review Undo from restore record");
    // An already-open review must not outlive a discovery that became
    // incomplete: the same list-dependent gate applies to this control.
    let incomplete = undo_withheld(ui);
    if incomplete {
        ui.label("Undo is withheld: the restore record list is INCOMPLETE. Fix the cause and press Refresh records first. The card was not changed.");
    }
    widgets::path_value(ui, "Card", &journal.card_path);
    widgets::path_value(ui, "Verified backup", &journal.backup_path);
    ui.label(format!("Save: {}", journal.save_display_name));
    ui.label("Undo replaces the whole card with its verified pre-restore bytes. Any change since the restore blocks Undo. Close the emulator completely first.");
    if widgets::action_button(
        ui,
        "Confirm Undo",
        widgets::ActionStyle::Secondary,
        advanced_mode && !incomplete && journal.phase == Ps2RestorePhase::Published,
    )
    .clicked()
        && !incomplete
    {
        let provider = ProcScanQuiescence::new();
        let report = provider.report(&ps2_card_binding(
            &journal.card_path,
            &journal.save_display_name,
        ));
        if report.state
            == archivefs_core::save_snapshots::tree_restore::safety::EmulatorQuiescence::Unknown
        {
            return Some(PsuRestoreDialogState::Refused(format!(
                "Undo is blocked because process inspection is incomplete. Unreadable live process IDs: {:?}. Close these processes or correct /proc access, then retry. The card was not changed.",
                report.unreadable_processes
            )));
        }
        invalidate(ui);
        return Some(
            match undo_ps2_psu_restore_guarded(path, &provider, unix_now()) {
                Ok(()) => PsuRestoreDialogState::Undone,
                Err(error) => PsuRestoreDialogState::Refused(psu_restore_error_message(&error)),
            },
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_outcomes_use_actionable_text() {
        assert!(outcome_message(&Ps2RecoveryOutcome::ConfirmedPublished).contains("Review Undo"));
        assert!(
            outcome_message(&Ps2RecoveryOutcome::NeedsAttention(
                "Retained artifact".into()
            ))
            .contains("Retained artifact")
        );
        assert!(outcome_message(&Ps2RecoveryOutcome::UndoCompleted).contains("completed"));
    }
    #[test]
    fn discovery_of_corrupt_synthetic_record_is_visible_and_blocks_undo() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("ps2-psu-restore-synthetic.json"),
            b"corrupt synthetic journal",
        )
        .unwrap();
        let rows = discover_ps2_restore_journals(dir.path());
        assert_eq!(rows.len(), 1);
        assert!(rows[0].error.is_some());
        assert!(!rows[0].undo_available);
    }
}

#[cfg(test)]
mod restart_tests {
    use super::*;
    use archivefs_core::memory_card_inventory::restore_guard::{FileIdentity, Ps2RestoreJournal};
    use sha2::{Digest, Sha256};

    fn render(ctx: &egui::Context, input: egui::RawInput, dir: &Path) -> egui::FullOutput {
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show_in(ui, true, dir));
        })
    }

    #[test]
    fn restarted_published_record_offers_confirmation_and_discovery_is_cached() {
        let dir = tempfile::tempdir().unwrap();
        let card = dir.path().join("synthetic-card.ps2");
        let backup = dir.path().join("synthetic-backup.ps2");
        std::fs::write(&card, b"synthetic unchanged card").unwrap();
        std::fs::write(&backup, b"synthetic unchanged backup").unwrap();
        let journal = Ps2RestoreJournal {
            version: 1,
            operation_id: "synthetic".into(),
            phase: Ps2RestorePhase::Published,
            binding: ps2_card_binding(&card, "synthetic save"),
            card_path: card.clone(),
            psu_path: dir.path().join("synthetic.psu"),
            psu_sha256: "synthetic".into(),
            backup_path: backup.clone(),
            backup_sha256: Some("synthetic".into()),
            original_sha256: "synthetic".into(),
            original_size: 0,
            original_identity: FileIdentity {
                device: 0,
                inode: 0,
                size: 0,
                mtime_seconds: 0,
                mtime_nanoseconds: 0,
            },
            staged_path: None,
            staged_sha256: None,
            post_sha256: Some("synthetic".into()),
            post_identity: None,
            undo_identity: None,
            save_display_name: "synthetic save".into(),
            file_count: 1,
            detail: None,
            history: vec![(Ps2RestorePhase::Published, 1)],
        };
        let path = dir.path().join("ps2-psu-restore-synthetic.json");
        let body = serde_json::to_vec(&journal).unwrap();
        let mut bytes = format!(
            "EMUWIZ-PS2-RESTORE-JOURNAL v1 sha256={}\n",
            Sha256::digest(&body)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        )
        .into_bytes();
        bytes.extend(body);
        std::fs::write(&path, bytes).unwrap();
        let ctx = egui::Context::default();
        let output = render(&ctx, egui::RawInput::default(), dir.path());
        let point = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == "Review Undo" => {
                    Some(text.pos + text.galley.size() / 2.0)
                }
                _ => None,
            })
            .expect("Published journal must show a Review Undo control after restart");
        // A later change does not cause a disk scan on every frame.
        std::fs::write(&path, b"corrupted synthetic journal").unwrap();
        for pressed in [true, false] {
            let input = egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            };
            render(&ctx, input, dir.path());
        }
        let state =
            ctx.data_mut(|data| data.get_temp::<PsuRestoreDialogState>(psu_restore_dialog_id()));
        assert!(matches!(state, Some(PsuRestoreDialogState::UndoConfirm(found)) if found == path));
        assert_eq!(std::fs::read(&card).unwrap(), b"synthetic unchanged card");
        assert_eq!(
            std::fs::read(&backup).unwrap(),
            b"synthetic unchanged backup"
        );
        ctx.data_mut(|data| data.remove::<History>(history_id()));
        render(&ctx, egui::RawInput::default(), dir.path());
        let history = ctx
            .data_mut(|data| data.get_temp::<History>(history_id()))
            .unwrap();
        assert!(history.rows[0].error.is_some());
        assert!(!history.rows[0].undo_available);
    }

    #[test]
    fn restarted_unavailable_stage_is_visible_without_undo() {
        let dir = tempfile::tempdir().unwrap();
        let card = dir.path().join("synthetic-card.ps2");
        let backup = dir.path().join("synthetic-backup.ps2");
        std::fs::write(&card, b"synthetic unchanged card").unwrap();
        std::fs::write(&backup, b"synthetic unchanged card").unwrap();
        let mut journal = Ps2RestoreJournal {
            version: 1,
            operation_id: "synthetic".into(),
            phase: Ps2RestorePhase::Staged,
            binding: ps2_card_binding(&card, "synthetic save"),
            card_path: card.clone(),
            psu_path: dir.path().join("synthetic.psu"),
            psu_sha256: "synthetic".into(),
            backup_path: backup.clone(),
            backup_sha256: Some("synthetic".into()),
            original_sha256: "synthetic".into(),
            original_size: 0,
            original_identity: FileIdentity {
                device: 0,
                inode: 0,
                size: 0,
                mtime_seconds: 0,
                mtime_nanoseconds: 0,
            },
            staged_path: None,
            staged_sha256: None,
            post_sha256: Some("synthetic".into()),
            post_identity: None,
            undo_identity: None,
            save_display_name: "synthetic save".into(),
            file_count: 1,
            detail: None,
            history: vec![(Ps2RestorePhase::Staged, 1)],
        };
        let stage = dir.path().join(".emuwiz-ps2-stage-synthetic.tmp");
        std::os::unix::fs::symlink(&stage, &stage).unwrap();
        journal.staged_path = Some(stage.clone());
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(&card).unwrap();
        journal.original_size = metadata.len();
        journal.original_identity = FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            mtime_seconds: metadata.mtime(),
            mtime_nanoseconds: metadata.mtime_nsec(),
        };
        journal.original_sha256 = Sha256::digest(std::fs::read(&backup).unwrap())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let path = dir.path().join("ps2-psu-restore-synthetic.json");
        let body = serde_json::to_vec(&journal).unwrap();
        let mut bytes = format!(
            "EMUWIZ-PS2-RESTORE-JOURNAL v1 sha256={}\n",
            Sha256::digest(&body)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        )
        .into_bytes();
        bytes.extend(body);
        std::fs::write(&path, bytes).unwrap();
        let receipt = std::fs::read(&path).unwrap();
        assert!(matches!(
            archivefs_core::memory_card_inventory::restore_guard::recover_ps2_psu_restore(&path, 2)
                .unwrap(),
            Ps2RecoveryOutcome::NeedsAttention(_)
        ));
        for _ in 0..2 {
            let ctx = egui::Context::default();
            let output = render(&ctx, egui::RawInput::default(), dir.path());
            let history = ctx
                .data_mut(|data| data.get_temp::<History>(history_id()))
                .unwrap();
            assert!(history.rows[0].needs_attention && history.rows[0].needs_recovery);
            assert!(!history.rows[0].undo_available);
            let texts: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                    _ => None,
                })
                .collect();
            assert!(!texts.contains(&"Review Undo"));
            assert!(
                texts
                    .iter()
                    .any(|text| text.contains("Unsupported recovery artifact"))
            );
            assert_eq!(std::fs::read(&path).unwrap(), receipt);
            assert_eq!(std::fs::read(&card).unwrap(), b"synthetic unchanged card");
            assert!(std::fs::symlink_metadata(&stage).unwrap().is_symlink());
        }
    }

    #[test]
    fn viewing_missing_history_does_not_create_directory() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing-history");
        render(
            &egui::Context::default(),
            egui::RawInput::default(),
            &missing,
        );
        assert!(!missing.exists());
    }
}

#[cfg(test)]
mod discovery_tests;
