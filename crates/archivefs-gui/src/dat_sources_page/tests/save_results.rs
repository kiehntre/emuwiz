use super::*;
use std::os::unix::ffi::OsStrExt;

fn database(fixture: &Fixture, roms: Option<&Path>) -> PathBuf {
    let path = fixture.root.join("catalogue.sqlite3");
    archivefs_core::Database::open_or_create(&path)
        .unwrap()
        .close()
        .unwrap();
    if let Some(roms) = roms {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute(
            "INSERT INTO source_folders(id,path,first_seen_at,last_seen_in_config_at) VALUES(1,?1,'now','now')",
            [roms.as_os_str().as_bytes()],
        ).unwrap();
        for (index, name) in ["super.bin", "mystery.bin"].iter().enumerate() {
            connection.execute(
                "INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,archive_kind,display_name,normalized_name,first_seen_at,last_seen_at,created_at,updated_at)
                 VALUES(?1,1,?2,?3,?2,'zip',?4,?4,'now','now','now','now')",
                rusqlite::params![index as i64 + 1, name.as_bytes(), roms.join(name).as_os_str().as_bytes(), name],
            ).unwrap();
        }
    }
    path
}

fn audit(page: &mut DatSourcesPageState, id: &str, roms: &Path) {
    page.apply(DatSourcesPageAction::Audit {
        id: id.to_string(),
        scan_root: roms.to_path_buf(),
    });
    run_to_completion(page);
}

fn result(page: &DatSourcesPageState, operation: DatSaveOperation, id: &str) -> DatSaveResult {
    page.view()
        .save_results
        .into_iter()
        .find(|result| result.operation == operation && result.source_id == id)
        .expect("source's final result")
}

#[test]
fn successful_audit_saves_real_identity_rows_and_retains_success() {
    let (fixture, mut page, roms) = audit_fixture();
    let path = database(&fixture, Some(&roms));
    page.database_path = Some(path.clone());
    audit(&mut page, "collection", &roms);
    assert_eq!(
        result(&page, DatSaveOperation::Audit, "collection").outcome,
        DatSaveOutcome::Success
    );
    assert!(page.view().running.is_none());
    assert!(page.view().audit.is_some());
    let connection = rusqlite::Connection::open(path).unwrap();
    let count: i64 = connection
        .query_row(
            "SELECT count(*) FROM library_dat_identities WHERE dat_source_id='collection'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn save_open_failure_survives_successful_parsing_and_job_teardown() {
    let (fixture, mut page, roms) = audit_fixture();
    let private_path = fixture
        .write("private-location", "a file cannot contain a database")
        .join("catalogue.sqlite3");
    page.database_path = Some(private_path);
    audit(&mut page, "collection", &roms);
    let saved = result(&page, DatSaveOperation::Audit, "collection");
    assert_eq!(saved.outcome, DatSaveOutcome::PersistenceFailure);
    assert!(saved.explanation.contains("run this operation again"));
    assert!(!saved.explanation.contains("private-location"));
    assert!(!saved.technical_details.is_empty());
    assert!(
        page.view().audit.is_some(),
        "parsed evidence remains useful"
    );
    assert!(page.view().running.is_none());
    for _ in 0..3 {
        page.poll();
    }
    assert_eq!(result(&page, DatSaveOperation::Audit, "collection"), saved);
}

#[test]
fn mid_save_database_error_is_failure_and_keeps_technical_detail() {
    let (fixture, mut page, roms) = audit_fixture();
    let path = database(&fixture, Some(&roms));
    rusqlite::Connection::open(&path).unwrap().execute_batch(
        "CREATE TRIGGER fail_dat_identity BEFORE INSERT ON library_dat_identities BEGIN SELECT RAISE(ABORT, 'fixture identity write rejected'); END;"
    ).unwrap();
    page.database_path = Some(path);
    audit(&mut page, "collection", &roms);
    let saved = result(&page, DatSaveOperation::Audit, "collection");
    assert_eq!(saved.outcome, DatSaveOutcome::PersistenceFailure);
    assert!(saved.explanation.contains("saving library DAT identities"));
    assert!(
        saved
            .technical_details
            .iter()
            .any(|detail| detail.contains("fixture identity write rejected"))
    );
    assert!(page.view().identity_enrichment.is_none());
}

#[test]
fn platform_save_failure_reports_failure_after_identity_rows_committed() {
    let (fixture, mut page, roms) = audit_fixture();
    let path = database(&fixture, Some(&roms));
    rusqlite::Connection::open(&path).unwrap().execute_batch(
        "CREATE TRIGGER fail_dat_platform BEFORE INSERT ON platform_assignments BEGIN SELECT RAISE(ABORT, 'fixture platform write rejected'); END;"
    ).unwrap();
    page.database_path = Some(path.clone());
    page.apply(DatSourcesPageAction::SetPlatform {
        id: "collection".to_string(),
        platform: Some("NES".to_string()),
    });
    audit(&mut page, "collection", &roms);
    let saved = result(&page, DatSaveOperation::Audit, "collection");
    assert_eq!(
        saved.outcome,
        DatSaveOutcome::PersistenceFailure,
        "{saved:?}"
    );
    assert!(saved.explanation.contains("saving platform enrichment"));
    assert!(
        saved
            .technical_details
            .iter()
            .any(|detail| detail.contains("fixture platform write rejected"))
    );
    let count: i64 = rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM library_dat_identities WHERE dat_source_id='collection'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 2,
        "earlier committed rows do not imply overall save success"
    );
}

#[test]
fn audit_outside_library_reports_warning_without_inventing_saved_identities() {
    let (fixture, mut page, roms) = audit_fixture();
    page.database_path = Some(database(&fixture, None));
    audit(&mut page, "collection", &roms);
    let saved = result(&page, DatSaveOperation::Audit, "collection");
    assert_eq!(saved.outcome, DatSaveOutcome::SuccessWithWarnings);
    assert!(
        saved
            .technical_details
            .iter()
            .any(|detail| detail.contains("2 item(s) outside the library"))
    );
}

#[test]
fn partial_source_failure_is_not_downgraded_to_warning() {
    let fixture = Fixture::new();
    let path = database(&fixture, None);
    let mut outcome = minimal_outcome();
    outcome
        .unreadable_catalogues
        .push("broken.dat: invalid XML".to_string());
    let (saved, _) = save_result::persist_audit(Some(&path), &outcome, 1, &AtomicBool::new(false));
    assert_eq!(saved.outcome, DatSaveOutcome::PartialFailure);
    assert!(
        saved
            .technical_details
            .iter()
            .any(|detail| detail.contains("broken.dat"))
    );
}

#[test]
fn navigation_ui_recreation_coverage_refresh_and_revert_keep_completed_failure() {
    let (fixture, mut page, roms) = audit_fixture();
    page.database_path = Some(fixture.dir("not-a-database"));
    audit(&mut page, "collection", &roms);
    let saved = result(&page, DatSaveOperation::Audit, "collection");
    page.apply(DatSourcesPageAction::OpenDatSources);
    page.apply(DatSourcesPageAction::OpenAdvancedIdentifyRename);
    page.apply(DatSourcesPageAction::RefreshCoverage {
        id: "collection".to_string(),
    });
    page.apply(DatSourcesPageAction::Revert);
    assert_eq!(result(&page, DatSaveOperation::Audit, "collection"), saved);
    for render in [render, render_identify_rename, render_quick_rename] {
        let output = render(&page.view(), &mut DatSourcesPageUi::default());
        assert!(rendered_text_contains(&output, "Save failed"));
        assert!(!rendered_text_contains(
            &output,
            &fixture.root.to_string_lossy()
        ));
    }
}

#[test]
fn retry_replaces_only_its_source_and_operation_after_completion() {
    let (fixture, mut page, roms) = audit_fixture();
    page.apply(DatSourcesPageAction::AddFile {
        path: fixture.write("other.dat", LOGIQX),
    });
    page.database_path = Some(fixture.dir("bad-database"));
    audit(&mut page, "collection", &roms);
    audit(&mut page, "other", &roms);
    let other = result(&page, DatSaveOperation::Audit, "other");
    page.database_path = Some(database(&fixture, Some(&roms)));
    page.apply(DatSourcesPageAction::Audit {
        id: "collection".to_string(),
        scan_root: roms,
    });
    assert_eq!(
        result(&page, DatSaveOperation::Audit, "collection").outcome,
        DatSaveOutcome::PersistenceFailure
    );
    run_to_completion(&mut page);
    assert_eq!(
        result(&page, DatSaveOperation::Audit, "collection").outcome,
        DatSaveOutcome::Success
    );
    assert_eq!(result(&page, DatSaveOperation::Audit, "other"), other);
    page.apply(DatSourcesPageAction::Validate {
        id: "collection".to_string(),
    });
    run_to_completion(&mut page);
    assert_eq!(page.view().save_results.len(), 3);
    assert_eq!(result(&page, DatSaveOperation::Audit, "other"), other);
}

#[test]
fn validate_all_retains_each_sources_save_failure_and_retry() {
    let (fixture, mut page, _) = audit_fixture();
    page.apply(DatSourcesPageAction::AddFile {
        path: fixture.write("other.dat", LOGIQX),
    });
    page.database_path = Some(fixture.dir("bad-database"));
    page.apply(DatSourcesPageAction::ValidateAll);
    run_to_completion(&mut page);
    assert_eq!(page.view().save_results.len(), 2);
    for id in ["collection", "other"] {
        assert_eq!(
            result(&page, DatSaveOperation::Validation, id).outcome,
            DatSaveOutcome::PersistenceFailure
        );
    }
    page.database_path = Some(database(&fixture, None));
    page.apply(DatSourcesPageAction::Validate {
        id: "collection".to_string(),
    });
    run_to_completion(&mut page);
    assert!(matches!(
        result(&page, DatSaveOperation::Validation, "collection").outcome,
        DatSaveOutcome::Success | DatSaveOutcome::SuccessWithWarnings
    ));
    assert_eq!(
        result(&page, DatSaveOperation::Validation, "other").outcome,
        DatSaveOutcome::PersistenceFailure
    );
}

#[test]
fn no_database_and_combined_audit_never_claim_persistence() {
    let (_fixture, mut page, roms) = audit_fixture();
    audit(&mut page, "collection", &roms);
    assert_eq!(
        result(&page, DatSaveOperation::Audit, "collection").outcome,
        DatSaveOutcome::NotSaved
    );
    page.apply(DatSourcesPageAction::AuditAllEnabled { scan_root: roms });
    run_to_completion(&mut page);
    let combined = result(
        &page,
        DatSaveOperation::CombinedAudit,
        archivefs_core::dat::sources::audit_run::COMBINED_AUDIT_SOURCE_ID,
    );
    assert_eq!(combined.outcome, DatSaveOutcome::NotSaved);
    assert!(
        combined
            .technical_details
            .iter()
            .any(|detail| detail.contains("collection"))
    );
    assert_eq!(page.view().save_results.len(), 2);
}

#[test]
fn retry_without_a_database_cannot_clear_a_previous_save_failure() {
    let (fixture, mut page, roms) = audit_fixture();
    page.database_path = Some(fixture.dir("bad-database"));
    audit(&mut page, "collection", &roms);
    let failure = result(&page, DatSaveOperation::Audit, "collection");
    page.database_path = None;
    audit(&mut page, "collection", &roms);
    assert_eq!(
        result(&page, DatSaveOperation::Audit, "collection"),
        failure
    );
}

#[test]
fn diagnostic_disclosures_are_private_by_default_and_independent_per_source() {
    let mut first =
        save_result::persist_audit(None, &minimal_outcome(), 1, &AtomicBool::new(false)).0;
    first.outcome = DatSaveOutcome::PersistenceFailure;
    first.technical_details = vec!["first private database error".to_string()];
    let mut second = first.clone();
    second.source_id = "second".to_string();
    second.technical_details = vec!["second private database error".to_string()];
    let results = [first, second];
    let context = egui::Context::default();
    let base = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1000.0, 1000.0),
        )),
        ..Default::default()
    };
    let render = |input| {
        context.run(input, |context| {
            egui::CentralPanel::default().show(context, |ui| save_result::show(ui, &results));
        })
    };
    let closed = render(base.clone());
    assert!(!rendered_text_contains(&closed, "private database error"));
    let pos =
        find_exact_text_center(&closed, "Technical details (may include local paths)").unwrap();
    let mut click = base.clone();
    click.events = vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        },
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        },
    ];
    render(click);
    let opened = render(base);
    assert!(rendered_text_contains(
        &opened,
        "first private database error"
    ));
    assert!(!rendered_text_contains(
        &opened,
        "second private database error"
    ));
}

#[test]
fn stale_save_result_cannot_replace_a_current_attempt() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    let (sender, messages) = sync_channel(PROGRESS_QUEUE_DEPTH);
    page.audit_generation = 2;
    page.job = Some(RunningJob {
        kind: JobKind::Audit,
        source_id: "collection".to_string(),
        cancel: Arc::new(AtomicBool::new(false)),
        cancel_requested: false,
        messages,
        latest: "Starting".to_string(),
        started_at: Instant::now(),
        audit_progress: None,
        platform_display: None,
        bulk: None,
    });
    let stale = save_result::persist_audit(None, &minimal_outcome(), 1, &AtomicBool::new(false)).0;
    sender
        .send(JobMessage::SaveResult {
            generation: Some(1),
            result: stale,
        })
        .unwrap();
    sender.send(JobMessage::Cancelled).unwrap();
    page.poll();
    assert!(page.view().save_results.is_empty());
    assert!(!page.is_busy());
}

#[test]
fn full_progress_queue_cannot_drop_a_final_result_and_cancellation_keeps_it() {
    let fixture = Fixture::new();
    let mut page = fixture.page();
    let (sender, messages) = sync_channel(PROGRESS_QUEUE_DEPTH);
    page.audit_generation = 7;
    page.job = Some(RunningJob {
        kind: JobKind::Audit,
        source_id: "collection".to_string(),
        cancel: Arc::new(AtomicBool::new(false)),
        cancel_requested: false,
        messages,
        latest: "Starting".to_string(),
        started_at: Instant::now(),
        audit_progress: Some(AuditProgressTracker::new()),
        platform_display: None,
        bulk: None,
    });
    for _ in 0..PROGRESS_QUEUE_DEPTH {
        send_progress(&sender, JobMessage::Progress("Hashing".to_string()));
    }
    let mut saved =
        save_result::persist_audit(None, &minimal_outcome(), 7, &AtomicBool::new(false)).0;
    saved.outcome = DatSaveOutcome::PersistenceFailure;
    let expected = saved.clone();
    let worker = std::thread::spawn(move || {
        sender
            .send(JobMessage::SaveResult {
                generation: Some(7),
                result: saved,
            })
            .unwrap();
        send_progress(
            &sender,
            JobMessage::Progress("Building rename plan…".to_string()),
        );
        sender.send(JobMessage::Cancelled).unwrap();
    });
    page.apply(DatSourcesPageAction::CancelJob);
    run_to_completion(&mut page);
    worker.join().unwrap();
    assert_eq!(
        result(&page, DatSaveOperation::Audit, "collection"),
        expected
    );
    assert!(page.view().audit.is_none());
    assert!(page.view().running.is_none());
}
