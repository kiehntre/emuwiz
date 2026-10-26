use super::super::page::tests::{Fixture, GOOD};
use super::*;
use archivefs_core::dat::rename_apply::{
    EntryState, RollbackResult, TransactionState,
    journal::{list_journals, read_journal, write_journal},
    reconcile_recovery, rollback_transaction_confined,
};

fn fixture() -> Fixture {
    let f = Fixture::new(GOOD);
    f.rom("donor", b"hello world");
    std::fs::create_dir(f.request.root.join("g")).unwrap();
    f
}
fn options(f: &Fixture) -> MameInternalRepairApplyOptions {
    MameInternalRepairApplyOptions {
        journal_dir: Some(f.dir.path().join("journals")),
        ..Default::default()
    }
}
#[test]
fn blue_review_apply_reload_undo_uses_existing_journal() {
    let f = fixture();
    let report = f.report();
    let reviewed = report.repair.as_ref().expect("supported exploded repair");
    assert_eq!(reviewed.apply.operations.len(), 1);
    assert_eq!(
        reviewed.apply.operations[0].destination_path,
        f.request.root.join("g/main.bin")
    );
    let opts = options(&f);
    let notice = execute(reviewed, &opts, &mut |_| {}).unwrap();
    assert!(notice.starts_with("Applied transaction"), "{notice}");
    assert_eq!(
        std::fs::read(f.request.root.join("g/main.bin")).unwrap(),
        b"hello world"
    );
    let (records, warnings) = list_journals(opts.journal_dir.as_ref().unwrap());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(records.len(), 1);
    let path = archivefs_core::dat::rename_apply::journal_path(
        opts.journal_dir.as_ref().unwrap(),
        &records[0].transaction_id,
    )
    .unwrap();
    let mut tx = read_journal(&path).unwrap();
    assert_eq!(tx.unknown["mame_catalogue_sha256"], report.catalogue.sha256);
    assert_eq!(tx.unknown["mame_reviewed_snapshot_digest"], reviewed.digest);
    let rows = crate::gui_v2::history_view::build_entries(
        &crate::gui_v2::history_view::HistorySources {
            quarantine: &[],
            playing_library: &[],
            organisation: std::slice::from_ref(&tx),
            mame: &[],
            evidence: &[],
        },
        None,
    );
    assert_eq!(
        rows[0].source,
        crate::gui_v2::history_view::Source::MameInternalRepair
    );
    assert_eq!(rows[0].family, crate::gui_v2::history_view::Family::Mame);
    assert!(crate::gui_v2::history_view::check_undo_safety(&rows[0]).safe());
    let result = rollback_transaction_confined(
        &mut tx,
        opts.journal_dir.as_ref().unwrap(),
        &AtomicBool::new(false),
        &TrustedRoots::from_paths([&f.request.root]),
    )
    .unwrap();
    assert_eq!(result.result, RollbackResult::FullyRolledBack);
    assert!(!f.request.root.join("g/main.bin").exists());
    assert_eq!(
        std::fs::read(f.request.root.join("donor/main.bin")).unwrap(),
        b"hello world"
    );
}
#[test]
fn interrupted_apply_and_undo_recover_from_exact_journalled_slots() {
    let f = fixture();
    let report = f.report();
    let opts = options(&f);
    execute(report.repair.as_ref().unwrap(), &opts, &mut |_| {}).unwrap();
    let (mut records, _) = list_journals(opts.journal_dir.as_ref().unwrap());
    let mut tx = records.remove(0);
    // Deterministic crash window: filesystem publication completed; final
    // Applied checkpoint absent. The created-object proof was checkpointed.
    tx.state = TransactionState::Applying;
    tx.entries[0].state = EntryState::Applying;
    write_journal(opts.journal_dir.as_ref().unwrap(), &tx).unwrap();
    reconcile_recovery(&mut tx, opts.journal_dir.as_ref().unwrap()).unwrap();
    assert_eq!(tx.state, TransactionState::Applied);
    let outcome = rollback_transaction_confined(
        &mut tx,
        opts.journal_dir.as_ref().unwrap(),
        &AtomicBool::new(false),
        &TrustedRoots::from_paths([&f.request.root]),
    )
    .unwrap();
    assert_eq!(outcome.result, RollbackResult::FullyRolledBack);
    tx.state = TransactionState::RollingBack;
    tx.entries[0].state = EntryState::RollingBack;
    write_journal(opts.journal_dir.as_ref().unwrap(), &tx).unwrap();
    reconcile_recovery(&mut tx, opts.journal_dir.as_ref().unwrap()).unwrap();
    assert_eq!(tx.entries[0].state, EntryState::RolledBack);
    assert!(!f.request.root.join("g/main.bin").exists());
    let after = f.report();
    assert!(after.inspection.inventory.inspected_completely);
    assert!(
        after
            .inspection
            .inventory
            .sets
            .iter()
            .flat_map(|s| &s.members)
            .all(|m| !m.name.contains(".emuwiz-copy-"))
    );
    assert!(
        after
            .repair
            .as_ref()
            .unwrap()
            .plan
            .requirements
            .iter()
            .flat_map(|r| &r.exact_matching_sources)
            .all(|s| !s.member.contains(".emuwiz-copy-"))
    );
}

#[test]
fn stale_catalogue_files_and_new_destinations_reject_before_journal() {
    for change in 0..4 {
        let f = fixture();
        let report = f.report();
        match change {
            0 => std::fs::write(
                &f.request.catalogue_path,
                format!("<mame build=\"0.281\">{GOOD}</mame>"),
            )
            .unwrap(),
            1 => std::fs::write(f.request.root.join("donor/main.bin"), b"changed rom").unwrap(),
            2 => std::fs::write(f.request.root.join("g/main.bin"), b"foreign rom").unwrap(),
            _ => std::fs::create_dir(f.request.root.join("new-set")).unwrap(),
        }
        let opts = options(&f);
        assert!(execute(report.repair.as_ref().unwrap(), &opts, &mut |_| {}).is_err());
        assert!(!opts.journal_dir.unwrap().exists());
    }
}
#[test]
fn exact_copy_inode_is_required_for_undo_and_recovery() {
    let f = fixture();
    let report = f.report();
    let opts = options(&f);
    execute(report.repair.as_ref().unwrap(), &opts, &mut |_| {}).unwrap();
    let (records, warnings) = list_journals(opts.journal_dir.as_ref().unwrap());
    assert!(warnings.is_empty(), "{warnings:?}");
    let path = archivefs_core::dat::rename_apply::journal_path(
        opts.journal_dir.as_ref().unwrap(),
        &records[0].transaction_id,
    )
    .unwrap();
    let mut tx = read_journal(&path).unwrap();
    let dest = f.request.root.join("g/main.bin");
    std::fs::rename(&dest, f.request.root.join("retained-original")).unwrap();
    std::fs::write(&dest, b"hello world").unwrap();
    let outcome = rollback_transaction_confined(
        &mut tx,
        opts.journal_dir.as_ref().unwrap(),
        &AtomicBool::new(false),
        &TrustedRoots::from_paths([&f.request.root]),
    )
    .unwrap();
    assert_ne!(outcome.result, RollbackResult::FullyRolledBack);
    assert_eq!(std::fs::read(&dest).unwrap(), b"hello world");
    tx.state = TransactionState::Applying;
    tx.entries[0].state = EntryState::Applying;
    write_journal(opts.journal_dir.as_ref().unwrap(), &tx).unwrap();
    let _recovery = reconcile_recovery(&mut tx, opts.journal_dir.as_ref().unwrap()).unwrap();
    assert_ne!(tx.state, TransactionState::Applied);
}
#[test]
fn ambiguous_dependencies_and_chd_are_never_apply_authority() {
    let f = Fixture::new(&GOOD.replace(
        "<machine name=\"g\">",
        "<machine name=\"g\"><device_ref name=\"missing-device\"/>",
    ));
    f.rom("donor", b"hello world");
    std::fs::create_dir(f.request.root.join("g")).unwrap();
    let report = f.report();
    assert!(report.repair.is_none());
    let f = Fixture::new(&GOOD.replace(
        "<rom name=\"main.bin\" size=\"11\"",
        "<disk name=\"main.bin\"",
    ));
    f.rom("donor", b"hello world");
    std::fs::create_dir(f.request.root.join("g")).unwrap();
    let report = f.report();
    if let Some(r) = report.repair {
        assert!(r.apply.transaction.is_none());
    }
}

#[test]
fn actual_blue_apply_button_runs_transaction_and_exposes_history_handoff() {
    use eframe::egui;
    let f = fixture();
    let report = f.report();
    let context = egui::Context::default();
    let mut state = State {
        journal_dir: options(&f).journal_dir,
        ..Default::default()
    };
    fn render(
        context: &egui::Context,
        state: &mut State,
        report: &Report,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 2200.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| state.show(ui, Some(report)));
            },
        )
    }
    fn text_rect(out: &egui::FullOutput, wanted: &str) -> egui::Rect {
        out.shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == wanted => {
                    Some(egui::Rect::from_min_size(t.pos, t.galley.size()))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("Missing actual control {wanted}"))
    }
    // With no confirmation, a click on the rendered Apply control must not run.
    let out = render(&context, &mut state, &report, vec![]);
    let button = text_rect(&out, "Apply internal repairs");
    for pressed in [true, false] {
        render(
            &context,
            &mut state,
            &report,
            vec![
                egui::Event::PointerMoved(button.center()),
                egui::Event::PointerButton {
                    pos: button.center(),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    assert!(!state.busy());
    assert!(!f.request.root.join("g/main.bin").exists());
    state.confirmation = "APPLY".into();
    let out = render(&context, &mut state, &report, vec![]);
    let button = text_rect(&out, "Apply internal repairs");
    for pressed in [true, false] {
        render(
            &context,
            &mut state,
            &report,
            vec![
                egui::Event::PointerMoved(button.center()),
                egui::Event::PointerButton {
                    pos: button.center(),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    assert!(state.busy());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state.busy() && std::time::Instant::now() < deadline {
        state.poll();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(!state.busy());
    assert!(
        state
            .notice
            .as_deref()
            .unwrap()
            .starts_with("Applied transaction"),
        "{:?}",
        state.notice
    );
    assert_eq!(
        std::fs::read(f.request.root.join("g/main.bin")).unwrap(),
        b"hello world"
    );
    let out = render(&context, &mut state, &report, vec![]);
    let button = text_rect(&out, "Open History & Undo / recovery");
    for pressed in [true, false] {
        render(
            &context,
            &mut state,
            &report,
            vec![
                egui::Event::PointerMoved(button.center()),
                egui::Event::PointerButton {
                    pos: button.center(),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    assert!(state.open_history);
}

#[test]
fn duplicate_observed_containers_and_supplied_release_mismatch_block_apply() {
    let f = fixture();
    zip::ZipWriter::new(std::fs::File::create(f.request.root.join("g.zip")).unwrap())
        .finish()
        .unwrap();
    let report = f.report();
    assert!(report.repair.is_none());
    assert!(
        report
            .repair_unavailable
            .unwrap()
            .contains("multiple collection containers")
    );
    let mut f = fixture();
    f.request.version_output = Some("MAME 0.279".into());
    let report = f.report();
    assert!(report.repair.is_none());
    assert!(report.repair_unavailable.unwrap().contains("version"));
}

#[test]
fn exact_preview_escapes_control_characters_in_affected_paths() {
    use eframe::egui;
    let f = fixture();
    // An actual fixture filename, including newline and bidi control.
    let hostile = f.request.root.join("donor/line\n\u{202e}.bin");
    std::fs::rename(f.request.root.join("donor/main.bin"), &hostile).unwrap();
    let report = f.report();
    assert_eq!(
        report.repair.as_ref().unwrap().apply.operations[0].selected_source,
        hostile
    );
    let context = egui::Context::default();
    let mut state = State::default();
    let out = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 2200.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| state.show(ui, Some(&report)));
        },
    );
    let labels: Vec<_> = out
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Text(t) => Some(t.galley.text()),
            _ => None,
        })
        .collect();
    assert!(labels.iter().all(|s| !s.contains('\u{202e}')));
    assert!(
        labels
            .iter()
            .any(|s| s.contains("\\u{202e}") && s.contains("\\n")),
        "{labels:?}"
    );
}

#[test]
fn blue_repair_undo_runs_through_the_native_history_controller() {
    let f = fixture();
    let report = f.report();
    let opts = options(&f);
    execute(report.repair.as_ref().unwrap(), &opts, &mut |_| {}).unwrap();
    let mut history = crate::repair_history_page::RepairHistoryPageState::load_with_journal_dir(
        opts.journal_dir.unwrap(),
    );
    let id = history.transactions[0].transaction_id.clone();
    assert!(history.can_undo(&id));
    let description = crate::repair_history_page::presentation::undo_confirmation_description(
        history.transaction_by_id(&id),
    );
    assert!(description.contains("private recovery staging"));
    assert!(description.contains("Original source files stay in place"));
    history.open_undo_confirmation(&id);
    assert_eq!(history.undo_confirm.as_ref().unwrap().transaction_id, id);
    history.confirm_undo();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while history.is_undo_running() && std::time::Instant::now() < deadline {
        history.poll_undo();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(!history.is_undo_running());
    assert!(history.undo_error.is_none(), "{:?}", history.undo_error);
    assert_eq!(
        history.undo_outcome.as_ref().unwrap().result,
        RollbackResult::FullyRolledBack
    );
    assert!(!f.request.root.join("g/main.bin").exists());
    assert_eq!(
        std::fs::read(f.request.root.join("donor/main.bin")).unwrap(),
        b"hello world"
    );
    assert_eq!(
        history.transaction_by_id(&id).unwrap().state,
        TransactionState::RolledBack
    );
}
