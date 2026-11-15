use super::*;
use crate::gui_v2::activity::{Activity, CancelPolicy, JobProgress};
use crate::gui_v2::routes::{Route, Section};
use archivefs_core::catalogue_health::SourceScanCoverage;

fn run(id: i64, status: &str) -> ScanRunRecord {
    ScanRunRecord {
        scan_run_id: id,
        started_at: "2026-11-13T01:13:27Z".into(),
        finished_at: Some("2026-11-13T01:41:14Z".into()),
        recorded_seconds: Some(1667),
        triggered_by: "cli-source-scan".into(),
        status: status.into(),
        error_message: None,
        counts: None,
        coverage: ScanRunCoverage::NotRecorded,
    }
}

fn counts(errors: i64) -> RecordedScanCounts {
    RecordedScanCounts {
        source_folders_scanned: 1,
        archives_seen: 130_466,
        archives_added: 29_776,
        archives_updated: 0,
        archives_missing: 0,
        errors_count: errors,
        archives_unchanged: Some(100_690),
        skipped_unsupported_extension: Some(277_261),
        skipped_ambiguous_platform: Some(1_592),
    }
}

fn finished(id: i64, status: &str, errors: i64) -> ScanRunRecord {
    ScanRunRecord {
        counts: Some(counts(errors)),
        ..run(id, status)
    }
}

fn source(root: &str, state: ScanCoverageState, diagnostic: Option<&str>) -> SourceScanCoverage {
    SourceScanCoverage {
        source_id: 1,
        root_identity: None,
        root: root.into(),
        state,
        excluded_roots: Vec::new(),
        diagnostic: diagnostic.map(str::to_string),
    }
}

#[test]
fn every_recorded_state_is_classified_without_overstating() {
    assert_eq!(classify(&finished(1, "completed", 0)), RunState::Completed);
    assert_eq!(
        classify(&finished(2, "completed", 1)),
        RunState::CompletedWithErrors
    );
    let reason_only = ScanRunRecord {
        error_message: Some("/mnt/a: unavailable".into()),
        ..finished(3, "completed", 0)
    };
    assert_eq!(classify(&reason_only), RunState::CompletedWithErrors);
    assert_eq!(classify(&finished(4, "partial", 0)), RunState::Partial);
    assert_eq!(classify(&run(5, "failed")), RunState::Failed);
    assert_eq!(classify(&run(6, "interrupted")), RunState::Interrupted);
    let unfinished = ScanRunRecord {
        finished_at: None,
        recorded_seconds: None,
        ..run(7, "running")
    };
    assert_eq!(classify(&unfinished), RunState::NoFinishRecord);
    assert_eq!(
        classify(&run(8, "cancelled")),
        RunState::Unknown("cancelled".into())
    );
    // Only a clean completed run reads as complete.
    for state in [
        RunState::CompletedWithErrors,
        RunState::Partial,
        RunState::Failed,
        RunState::Interrupted,
        RunState::NoFinishRecord,
        RunState::Unknown("x".into()),
    ] {
        assert_ne!(state.label(), "Completed");
        assert!(state.explanation().is_some(), "{state:?}");
        assert_ne!(state.tone(), StatusTone::Success, "{state:?}");
    }
}

#[test]
fn unfinished_runs_never_claim_a_finish_or_a_count() {
    let unfinished = ScanRunRecord {
        finished_at: None,
        recorded_seconds: None,
        ..run(7, "running")
    };
    let timing = timing_text(&unfinished, &classify(&unfinished));
    assert!(timing.contains("No finish recorded"), "{timing}");
    assert!(!timing.contains("Finished"), "{timing}");
    // The record's finished_at for an interrupted run is when it was noticed.
    let interrupted = run(6, "interrupted");
    let timing = timing_text(&interrupted, &classify(&interrupted));
    assert!(
        timing.contains("Found unfinished 2026-11-13 01:41:14 UTC"),
        "{timing}"
    );
    assert!(timing.contains("not its real end"), "{timing}");
    assert!(!timing.contains("Finished"), "{timing}");
    assert!(
        !timing.contains("27m"),
        "no duration for an interrupted run: {timing}"
    );
    let text = counts_text(None);
    assert!(text.contains("not recorded"), "{text}");
    assert!(!text.contains('0'), "{text}");
}

#[test]
fn a_finished_run_shows_its_recorded_counts_and_time() {
    let done = finished(209, "completed", 0);
    assert_eq!(
        timing_text(&done, &classify(&done)),
        "Started 2026-11-13 01:13:27 UTC · Finished 2026-11-13 01:41:14 UTC (27m 47s)"
    );
    assert_eq!(
        counts_text(done.counts.as_ref()),
        "1 source scanned · 130,466 seen · 29,776 added · 0 updated · 0 marked missing · 100,690 unchanged · 278,853 skipped · 0 errors"
    );
    let failed = run(5, "failed");
    assert!(timing_text(&failed, &classify(&failed)).contains("Failed 2026-11-13 01:41:14 UTC"));
    let no_finish = ScanRunRecord {
        finished_at: None,
        recorded_seconds: None,
        ..finished(3, "completed", 0)
    };
    assert!(timing_text(&no_finish, &classify(&no_finish)).contains("Finish time not recorded"));
}

#[test]
fn missing_fields_read_as_not_recorded_never_zero() {
    let older = RecordedScanCounts {
        archives_unchanged: None,
        skipped_unsupported_extension: None,
        skipped_ambiguous_platform: None,
        ..counts(1)
    };
    let text = counts_text(Some(&older));
    assert!(text.contains("not recorded unchanged"), "{text}");
    assert!(text.contains("not recorded skipped"), "{text}");
    assert!(text.ends_with("1 error"), "{text}");
    assert_eq!(time_text("yesterday"), "yesterday");
}

#[test]
fn per_source_scope_shows_each_source_and_its_recorded_reason() {
    assert_eq!(
        coverage_lines(&ScanRunCoverage::NotRecorded),
        ["Sources: not recorded for this run."]
    );
    assert!(
        coverage_lines(&ScanRunCoverage::Unreadable("bad json".into()))[0].contains("bad json")
    );
    let mut games = source("/mnt/usbdrive/games", ScanCoverageState::Complete, None);
    games.excluded_roots = vec!["/mnt/usbdrive/games/gba/ratarmount".into(); 5];
    let lines = coverage_lines(&ScanRunCoverage::Recorded(vec![
        games,
        source(
            "/mnt/games/roms",
            ScanCoverageState::Partial,
            Some("incomplete enumeration; missing reconciliation withheld"),
        ),
        source("/mnt/old", ScanCoverageState::Removed, None),
        source("/mnt/older", ScanCoverageState::Removed, None),
    ]));
    assert_eq!(
        lines,
        [
            "/mnt/usbdrive/games — Read completely · 5 folders inside it kept out of its missing-game check",
            "/mnt/games/roms — Partly read: incomplete enumeration; missing reconciliation withheld",
            "2 sources no longer in settings (not scanned).",
        ]
    );
}

#[test]
fn triggers_are_named_without_claiming_which_app_started_them() {
    assert_eq!(trigger_label("cli-scan-all-sources"), "All enabled sources");
    assert_eq!(trigger_label("cli-source-scan"), "One source");
    assert_eq!(
        trigger_label("catalogue-presence-reconciliation"),
        "Presence check (folders not read)"
    );
    assert_eq!(trigger_label("other"), "Recorded as \"other\"");
}

/// A fixture library database with one run of each recorded state.
fn fixture_database(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("library.sqlite3");
    drop(Database::open_or_create(&path).unwrap());
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "INSERT INTO source_folders(id, path, first_seen_at, last_seen_in_config_at)
               VALUES (22, CAST('/mnt/usbdrive/games' AS BLOB), '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
             INSERT INTO scan_runs(id, started_at, finished_at, triggered_by, status)
               VALUES (1, '2026-09-01T00:00:00Z', '2026-09-01T01:00:00Z', 'cli-scan-all-sources', 'interrupted');
             INSERT INTO scan_runs(id, started_at, finished_at, triggered_by, status, error_message)
               VALUES (2, '2026-09-02T00:00:00Z', '2026-09-02T00:00:09Z', 'gui-scan-library', 'failed', 'database is locked');
             INSERT INTO scan_runs(id, started_at, triggered_by, status)
               VALUES (3, '2026-09-03T00:00:00Z', 'cli-scan-all-sources', 'running');
             INSERT INTO scan_runs(id, started_at, finished_at, triggered_by, status, source_folders_scanned,
               archives_seen, errors_count, error_message)
               VALUES (4, '2026-09-04T00:00:00Z', '2026-09-04T00:05:00Z', 'cli-scan-all-sources', 'partial', 1,
               10, 1, '/mnt/usbdrive/games: permission denied');
             INSERT INTO scan_runs(id, started_at, finished_at, triggered_by, status, source_folders_scanned,
               archives_seen, archives_added, archives_unchanged)
               VALUES (5, '2026-11-13T01:13:27Z', '2026-11-13T01:41:14Z', 'cli-source-scan', 'completed', 1,
               130466, 29776, 100690);
             INSERT INTO scan_source_coverage(scan_run_id, source_folder_id, state, excluded_roots_json, diagnostic, root_identity_json)
               VALUES (5, 22, '\"complete\"', '[]', NULL, 'null'),
                      (4, 22, '\"partial\"', '[]', 'incomplete enumeration; missing reconciliation withheld', 'null');",
        )
        .unwrap();
    drop(connection);
    path
}

#[test]
fn the_history_loads_read_only_from_a_fixture_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture_database(dir.path());
    let before = std::fs::read(&path).unwrap();
    let runs = load(&path).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the history load wrote"
    );
    let states: Vec<_> = runs.iter().map(classify).collect();
    assert_eq!(
        states,
        [
            RunState::Completed,
            RunState::Partial,
            RunState::NoFinishRecord,
            RunState::Failed,
            RunState::Interrupted,
        ]
    );
    assert!(runs[2].counts.is_none() && runs[3].counts.is_none() && runs[4].counts.is_none());
    assert_eq!(runs[0].counts.as_ref().unwrap().archives_added, 29_776);
    assert_eq!(runs[3].error_message.as_deref(), Some("database is locked"));
    assert!(coverage_lines(&runs[1].coverage)[0].contains("Partly read"));
    // No database: nothing is created.
    let absent = dir.path().join("absent.sqlite3");
    assert!(load(&absent).unwrap_err().contains("no library database"));
    assert!(!absent.exists());
}

fn rendered(state: &mut ScanHistoryState, job: Option<&Job>, upgrading: bool) -> String {
    let context = egui::Context::default();
    crate::gui_v2::readable_style(&context);
    let output = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            show(ui, state, job, upgrading);
        });
    });
    fn gather(shape: &egui::Shape, out: &mut String) {
        match shape {
            egui::Shape::Text(text) => {
                out.push_str(text.galley.text());
                out.push('\n');
            }
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| gather(s, out)),
            _ => {}
        }
    }
    let mut out = String::new();
    output
        .shapes
        .iter()
        .for_each(|clipped| gather(&clipped.shape, &mut out));
    out
}

#[test]
fn the_page_shows_each_run_truthfully() {
    let dir = tempfile::tempdir().unwrap();
    let runs = load(&fixture_database(dir.path())).unwrap();
    let mut state = ScanHistoryState::with_runs(runs);
    let text = rendered(&mut state, None, false);
    for wanted in [
        "SCAN HISTORY",
        "No folder scan has run from Blue in this session.",
        UNSAVED_NOTE,
        "Run 5 · One source",
        "Completed",
        "Partial",
        "No finish record",
        "Failed",
        "Did not finish",
        "Recorded reason: database is locked",
        "Counts: not recorded (this run did not finish).",
        "/mnt/usbdrive/games — Read completely",
    ] {
        assert!(text.contains(wanted), "{wanted}:\n{text}");
    }
    // A reload is a reload, never "a scan" (consistent with Database status).
    assert!(!text.contains("A scan is already running"), "{text}");
}

#[test]
fn a_running_blue_scan_uses_the_shared_job_card_and_offers_no_cancel() {
    let mut activity = Activity::default();
    let id = activity.queue(SCAN_JOB_TITLE, Route::Section(Section::Games), false);
    activity.start(id);
    let job = session_scan_job(&activity, Some(id)).unwrap();
    assert_eq!(job.cancel_policy, CancelPolicy::NotCancellable);
    let mut state = ScanHistoryState::with_runs(Vec::new());
    let text = rendered(&mut state, Some(job), false);
    for wanted in [
        "Current scan",
        SCAN_JOB_TITLE,
        "In progress",
        "This scan does not report how far it has got.",
        "It appears below once it has been saved.",
        CancelPolicy::NotCancellable.hint(),
        "No scan has been recorded in this library database.",
    ] {
        assert!(text.contains(wanted), "{wanted}:\n{text}");
    }
    assert!(!text.contains("Cancel\n"), "{text}");
    // Reported progress is shown, never invented.
    activity.update_progress(id, JobProgress::new("Reading folders", "files").at(12_831));
    let job = session_scan_job(&activity, Some(id)).unwrap();
    let text = rendered(&mut state, Some(job), false);
    assert!(text.contains("12,831 files so far"), "{text}");
    assert!(!text.contains("does not report"), "{text}");
}

#[test]
fn the_last_session_scan_is_found_after_it_ends() {
    let mut activity = Activity::default();
    assert!(session_scan_job(&activity, None).is_none());
    let first = activity.queue(SCAN_JOB_TITLE, Route::Section(Section::Games), false);
    activity.start(first);
    activity.finish(first, "Scan finished.".into(), None);
    let other = activity.queue(
        "Loading your library",
        Route::Section(Section::Games),
        false,
    );
    activity.start(other);
    let last = activity.queue(SCAN_JOB_TITLE, Route::Section(Section::Games), false);
    activity.start(last);
    activity.finish(last, "Scan failed.".into(), Some("boom".into()));
    let job = session_scan_job(&activity, None).unwrap();
    assert_eq!(job.summary, "Scan failed.");
    let mut state = ScanHistoryState::with_runs(Vec::new());
    let text = rendered(&mut state, Some(job), false);
    assert!(text.contains("Last scan in this session"), "{text}");
    assert!(text.contains("Needs attention"), "{text}");
    assert!(!text.contains("Current scan"), "{text}");
}

#[test]
fn an_upgrade_scan_is_shown_and_the_history_waits_for_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture_database(dir.path());
    let mut state = ScanHistoryState {
        database: Some(path),
        ..ScanHistoryState::default()
    };
    let text = rendered(&mut state, None, true);
    assert!(
        text.contains("A database upgrade scan is running."),
        "{text}"
    );
    // No read is started while a scan writes the catalogue.
    assert!(!state.loading());
    assert!(state.runs.is_none());
    // Once it ends, the history loads.
    rendered(&mut state, None, false);
    assert!(state.loading());
}
