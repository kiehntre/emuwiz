use super::*;

fn settled(state: &mut RepairHistoryPageState) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while state.background_busy() || state.is_undo_running() {
        state.poll_background();
        state.poll_undo();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn native_history_loads_real_records_and_requires_confirmation_before_exact_undo() {
    let dir = TestDir::new("native-history");
    let (roms, journals, result) = scan_and_apply(dir.path());
    let mut page = RepairHistoryPageState::load_background(journals);
    assert!(page.background_busy());
    assert!(page.transactions.is_empty());
    settled(&mut page);
    let id = result.summary.transaction_id;
    assert!(page.can_undo(&id));
    assert!(page.reverifications.contains_key(&id));
    page.open_undo_confirmation(&id);
    assert!(!page.undo_running);
    assert_eq!(std::fs::read(roms.join("alpha.bin")).unwrap(), b"test");
    page.confirm_undo();
    settled(&mut page);
    assert_eq!(
        page.undo_outcome.as_ref().unwrap().result,
        RollbackResult::FullyRolledBack
    );
    assert_eq!(std::fs::read(roms.join("a.bin")).unwrap(), b"test");
    assert_eq!(std::fs::read(roms.join("b.bin")).unwrap(), b"abc");
    assert!(!page.can_undo(&id));
}

#[test]
fn native_history_single_worker_keeps_gui_responsive_and_refuses_overlapping_work() {
    let dir = TestDir::new("native-blocked");
    let mut page = RepairHistoryPageState::empty(dir.path().to_owned());
    page.background_ui = true;
    let (release, blocked) = std::sync::mpsc::channel();
    page.start_io(move |_| {
        blocked.recv().unwrap();
    });
    page.refresh(); // must not replace or duplicate accepted work
    assert!(page.background_busy());
    let ctx = egui::Context::default();
    let mut clipboard = NoopClipboard::default();
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_repair_history_page(ui, &mut page, &mut clipboard);
        });
    });
    assert!(page.background_busy());
    release.send(()).unwrap();
    settled(&mut page);
}

#[test]
fn native_history_worker_panic_is_unknown_and_never_success() {
    let dir = TestDir::new("native-panic");
    let mut page = RepairHistoryPageState::empty(dir.path().to_owned());
    page.background_ui = true;
    page.start_io(|_| panic!("synthetic failure"));
    settled(&mut page);
    assert!(page.io_error.as_ref().unwrap().contains("unknown"));
    assert!(page.undo_outcome.is_none());
    page.refresh();
    assert!(
        !page.background_busy(),
        "unknown outcome must not retry automatically"
    );
}

#[test]
fn pending_resume_history_explains_current_plan_authority_without_offering_execution() {
    for checkpoint in ["pending", "failed", "interrupted"] {
        let dir = TestDir::new(checkpoint);
        let mut transaction = bare_transaction(
            "resume-review",
            TransactionState::ApplyFailed,
            EntryState::Planned,
        );
        transaction.unknown.insert(
            "emuwiz_exact_resume_state".into(),
            serde_json::json!(checkpoint),
        );
        let mut page = RepairHistoryPageState::empty(dir.path().to_owned());
        page.hide_settled = false;
        page.transactions.push(transaction);
        page.presentation = presentation::Snapshot::build(
            &page.transactions,
            &[(
                "resume-review".into(),
                RecoveryCleanupClassification::Uncertain,
            )],
        );
        let before = page.transactions.clone();
        let output = render(&mut page);
        assert!(rendered_text_contains(
            &output,
            "matching current approved plan"
        ));
        assert!(rendered_text_contains(&output, "Review Undo if available"));
        assert!(!rendered_text_contains(
            &output,
            "Resume approved transaction"
        ));
        assert_eq!(page.transactions, before);
        assert!(page.undo_confirm.is_none());
        assert!(!page.is_undo_running());
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    }
}

#[test]
fn ordinary_or_settled_records_do_not_claim_matching_plan_resume_is_available() {
    let mut transaction =
        bare_transaction("ordinary", TransactionState::Applied, EntryState::Applied);
    assert!(presentation::history_resume_guidance(&transaction).is_none());
    for checkpoint in ["completed", "not_resumable", "unrecognised"] {
        transaction.unknown.insert(
            "emuwiz_exact_resume_state".into(),
            serde_json::json!(checkpoint),
        );
        assert!(presentation::history_resume_guidance(&transaction).is_none());
    }
}

#[test]
fn native_history_keeps_valid_records_and_unreadable_journal_warning() {
    let dir = TestDir::new("native-unreadable-history");
    let (_roms, journals, _result) = scan_and_apply(dir.path());
    let broken = journals.join("broken.json");
    std::fs::write(&broken, b"{broken").unwrap();
    let mut page = RepairHistoryPageState::load_background(journals);
    settled(&mut page);
    assert!(!page.transactions.is_empty());
    assert!(!page.load_problems.is_empty());
    let output = render(&mut page);
    assert!(rendered_text_contains(
        &output,
        "Some journals could not be read"
    ));
    assert_eq!(std::fs::read(&broken).unwrap(), b"{broken");
    assert!(!page.is_undo_running());
}
