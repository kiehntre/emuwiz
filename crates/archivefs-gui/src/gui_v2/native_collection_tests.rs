use super::*;
use archivefs_core::{SourceFolderConfig, SourceRole};

struct Fixture {
    _root: tempfile::TempDir,
    config: PathBuf,
    database: PathBuf,
    sources: Vec<SourceFolderConfig>,
    summary: archivefs_core::ScanPersistSummary,
}

impl Fixture {
    fn new() -> Self {
        let isolated = archivefs_core::test_environment::isolated_root()
            .expect("these tests require private HOME/XDG roots");
        assert!(
            archivefs_core::default_config_path()
                .unwrap()
                .starts_with(isolated)
        );
        assert!(
            archivefs_core::default_database_path()
                .unwrap()
                .starts_with(isolated)
        );
        let root = tempfile::tempdir().unwrap();
        let games = root.path().join("games");
        let disabled = root.path().join("excluded");
        std::fs::create_dir_all(&games).unwrap();
        std::fs::create_dir_all(&disabled).unwrap();
        for index in 0..225 {
            std::fs::write(games.join(format!("unknown{index}.xyz")), b"synthetic").unwrap();
        }
        std::fs::write(disabled.join("must-not-scan.xyz"), b"excluded").unwrap();
        let config = root.path().join("config.toml");
        let database = root.path().join("library.sqlite3");
        let sources = vec![
            SourceFolderConfig {
                path: games,
                enabled: true,
                created_at: None,
            },
            SourceFolderConfig {
                path: disabled,
                enabled: false,
                created_at: None,
            },
        ];
        archivefs_core::save_source_folder_configs_to(
            &config,
            &sources,
            &root.path().join("mount"),
            "",
            None,
        )
        .unwrap();
        let summary =
            archivefs_core::scan_all_enabled_sources_at(&config, &database, "blue-synthetic-test")
                .unwrap();
        Self {
            _root: root,
            config,
            database,
            sources,
            summary,
        }
    }

    fn views(&self) -> Vec<SourceFolderView> {
        archivefs_core::list_source_folder_views_at(&self.config, &self.database).unwrap()
    }

    fn review(&self, action: SourceAction) -> ScanReview {
        ScanReview {
            action,
            generation: 7,
            database_path: self.database.clone(),
            database_existed: true,
            sources: self.views(),
            known_systems: BTreeMap::new(),
        }
    }

    fn workflows(&self, context: &egui::Context, session: bool) -> NativeWorkflows {
        let mut workflows = NativeWorkflows::new(context.clone());
        let db = Database::open_read_only(&self.database).unwrap();
        let snapshot = crate::database_load::load_snapshot_from(&db, &self.database, &self.config)
            .ok()
            .unwrap();
        workflows.app.database_state = DatabaseState::Ready {
            snapshot: Box::new(snapshot),
            last_scan_summary: session.then(|| self.summary.clone()),
        };
        workflows.app.database_generation = crate::DatabaseGeneration(7);
        workflows
    }
}

fn frame(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    show: impl FnMut(&egui::Context),
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 600.0),
            )),
            events,
            ..Default::default()
        },
        show,
    )
}

fn text_rect(output: &egui::FullOutput, needle: &str) -> Option<egui::Rect> {
    fn find(shape: &egui::Shape, needle: &str) -> Option<egui::Rect> {
        match shape {
            egui::Shape::Text(text) if text.galley.text().contains(needle) => {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, needle)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|shape| find(&shape.shape, needle))
}

fn modal_frame(
    workflows: &mut NativeWorkflows,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    frame(ctx, events, |ctx| workflows.show_source_scan_review(ctx))
}

fn click_modal(workflows: &mut NativeWorkflows, ctx: &egui::Context, label: &str) {
    // egui Areas measure their first frame before painting at their position.
    let _ = modal_frame(workflows, ctx, vec![]);
    let output = modal_frame(workflows, ctx, vec![]);
    let rect = text_rect(&output, label).unwrap();
    assert!(
        rect.min.y >= 0.0 && rect.max.y < 600.0 && rect.max.x < 1024.0,
        "primary control must fit the small display"
    );
    for pressed in [true, false] {
        modal_frame(
            workflows,
            ctx,
            vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
}

#[test]
fn review_shows_exact_scope_exclusions_and_backend_limits_without_writes() {
    let fixture = Fixture::new();
    let review = fixture.review(SourceAction::ScanAll);
    assert!(review.selected(&review.sources[0]));
    assert!(!review.selected(&review.sources[1]));
    let before = std::fs::read(&fixture.database).unwrap();
    let ctx = egui::Context::default();
    let output = frame(&ctx, vec![], |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| review.show(ui));
    });
    for label in [
        "Selected roots",
        "Excluded roots",
        "disabled",
        "cannot filter by system",
        "no cancellation",
    ] {
        assert!(text_rect(&output, label).is_some(), "missing {label}");
    }
    review
        .validate_saved_at(&fixture.config, &fixture.database)
        .unwrap();
    assert_eq!(before, std::fs::read(&fixture.database).unwrap());
}

#[test]
fn enable_requires_review_and_explicit_cancel_never_starts_worker() {
    let fixture = Fixture::new();
    let ctx = egui::Context::default();
    let mut workflows = fixture.workflows(&ctx, true);
    let before = std::fs::read(&fixture.database).unwrap();
    workflows.review_or_start_source_action(
        ctx.clone(),
        SourceAction::SetEnabled {
            path: fixture.sources[1].path.clone(),
            enabled: true,
        },
    );
    assert!(workflows.app.sources_ui.source_action.is_none());
    let review = workflows.collection.review.as_ref().unwrap();
    assert!(review.selected(&review.sources[1]));
    assert!(!review.selected(&review.sources[0]));
    click_modal(&mut workflows, &ctx, "Cancel");
    assert!(workflows.collection.review.is_none());
    assert!(workflows.app.sources_ui.source_action.is_none());
    assert_eq!(before, std::fs::read(&fixture.database).unwrap());
}

#[test]
fn changed_generation_or_source_metadata_invalidates_confirmation() {
    let fixture = Fixture::new();
    let review = fixture.review(SourceAction::ScanAll);
    assert!(review.current(7, &review.sources));
    assert!(!review.current(8, &review.sources));
    for field in 0..3 {
        let mut changed = review.sources.clone();
        match field {
            0 => changed[0].enabled = false,
            1 => changed[0].role = SourceRole::ArtworkMedia,
            _ => changed[0].assigned_platform = Some("NES".into()),
        }
        assert!(!review.current(7, &changed));
    }
    let ctx = egui::Context::default();
    let mut workflows = fixture.workflows(&ctx, true);
    workflows.collection.review = Some(review);
    workflows.app.database_generation.0 += 1;
    click_modal(&mut workflows, &ctx, "Confirm scan and update library");
    assert!(workflows.app.sources_ui.source_action.is_none());
    assert!(workflows.collection.review.is_some());
}

#[test]
fn only_confirm_starts_the_worker_and_a_preflight_error_never_mutates_the_library() {
    let fixture = Fixture::new();
    let before = std::fs::read(&fixture.database).unwrap();
    let ctx = egui::Context::default();
    let mut workflows = fixture.workflows(&ctx, true);
    workflows.review_or_start_source_action(
        ctx.clone(),
        SourceAction::ScanOne(fixture.sources[0].path.clone()),
    );
    assert!(workflows.app.sources_ui.source_action.is_none());
    click_modal(&mut workflows, &ctx, "Confirm scan and update library");
    assert!(workflows.collection.review.is_none());
    let running = workflows
        .app
        .sources_ui
        .source_action
        .as_mut()
        .expect("explicit confirmation starts the checked worker");
    assert_eq!(
        running.action,
        SourceAction::ScanOne(fixture.sources[0].path.clone())
    );
    running.worker.take().unwrap().join().unwrap();
    // The fixture deliberately differs from the isolated process's default DB.
    // Revalidation must refuse rather than send a fixture approval elsewhere.
    workflows.app.poll_source_action(&ctx);
    let feedback = workflows.app.feedback.as_ref().unwrap();
    assert!(!feedback.succeeded);
    assert!(feedback.message.contains("library location changed"));
    assert_eq!(before, std::fs::read(&fixture.database).unwrap());
}

#[test]
fn saved_scope_changes_or_unreadable_database_refuse_before_mutation() {
    let fixture = Fixture::new();
    let review = fixture.review(SourceAction::ScanAll);
    let before = std::fs::read(&fixture.database).unwrap();
    let mut changed = fixture.sources.clone();
    changed[1].enabled = true;
    archivefs_core::save_source_folder_configs_to(
        &fixture.config,
        &changed,
        &fixture._root.path().join("mount"),
        "",
        None,
    )
    .unwrap();
    assert!(
        review
            .validate_saved_at(&fixture.config, &fixture.database)
            .unwrap_err()
            .contains("changed after review")
    );
    assert_eq!(before, std::fs::read(&fixture.database).unwrap());
    std::fs::remove_file(&fixture.database).unwrap();
    assert!(
        review
            .validate_saved_at(&fixture.config, &fixture.database)
            .is_err()
    );
    assert!(
        !fixture.database.exists(),
        "review must not create a database"
    );
}

#[test]
fn checked_worker_refusal_never_runs_existing_mutating_action() {
    let ctx = egui::Context::default();
    let mut workflows = NativeWorkflows::new(ctx.clone());
    let error = "synthetic stale review refusal";
    workflows.app.start_source_action_checked(
        ctx.clone(),
        SourceAction::Add(PathBuf::from("/never-created-by-blue-test")),
        move || Err(error.into()),
    );
    workflows
        .app
        .sources_ui
        .source_action
        .as_mut()
        .unwrap()
        .worker
        .take()
        .unwrap()
        .join()
        .unwrap();
    workflows.app.poll_source_action(&ctx);
    assert!(workflows.app.sources_ui.source_action.is_none());
    assert_eq!(workflows.app.feedback.as_ref().unwrap().message, error);
    assert!(!workflows.app.feedback.as_ref().unwrap().succeeded);
}

#[test]
fn blue_report_uses_durable_counts_and_survives_restart_without_scanning() {
    let fixture = Fixture::new();
    let ctx = egui::Context::default();
    let before = std::fs::read(&fixture.database).unwrap();
    for session in [true, false] {
        let mut workflows = fixture.workflows(&ctx, session);
        let output = frame(&ctx, vec![], |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| workflows.show_collection_report(ui));
        });
        assert!(text_rect(&output, "Collection Discovery").is_some());
        assert!(text_rect(&output, "Saved scan").is_some());
        assert!(text_rect(&output, "Library records:").is_some());
        assert!(workflows.app.sources_ui.source_action.is_none());
    }
    let db = Database::open_read_only(&fixture.database).unwrap();
    let page = db
        .query_discovery_details(
            fixture.summary.scan_run_id,
            0,
            200,
            archivefs_core::DiscoveryDetailFilter::All,
        )
        .unwrap();
    assert_eq!(page.total_matching, 225);
    assert!(
        !page
            .rows
            .iter()
            .any(|row| row.path.starts_with(&fixture.sources[1].path))
    );
    assert_eq!(before, std::fs::read(&fixture.database).unwrap());
}

#[test]
fn stale_session_summary_is_not_presented_as_latest_completed_results() {
    let fixture = Fixture::new();
    let ctx = egui::Context::default();
    let mut workflows = fixture.workflows(&ctx, true);
    if let DatabaseState::Ready {
        last_scan_summary: Some(summary),
        ..
    } = &mut workflows.app.database_state
    {
        summary.scan_run_id += 1;
    }
    let output = frame(&ctx, vec![], |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| workflows.show_collection_report(ui));
    });
    assert!(text_rect(&output, "Inspect skipped files").is_none());
    assert!(text_rect(&output, "Showing details from the saved scan.").is_some());
}

#[test]
fn partial_scan_results_and_errors_survive_restart_in_blue() {
    let mut fixture = Fixture::new();
    fixture.sources[1].enabled = true;
    std::fs::remove_dir_all(&fixture.sources[1].path).unwrap();
    archivefs_core::save_source_folder_configs_to(
        &fixture.config,
        &fixture.sources,
        &fixture._root.path().join("mount"),
        "",
        None,
    )
    .unwrap();
    fixture.summary = archivefs_core::scan_all_enabled_sources_at(
        &fixture.config,
        &fixture.database,
        "partial-blue-test",
    )
    .unwrap();
    assert!(!fixture.summary.folder_errors.is_empty());
    let ctx = egui::Context::default();
    for session in [true, false] {
        let mut workflows = fixture.workflows(&ctx, session);
        let output = frame(&ctx, vec![], |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| workflows.show_collection_report(ui));
        });
        assert!(text_rect(&output, "Partial coverage").is_some());
        assert!(
            text_rect(
                &output,
                &format!("Saved scan {}", fixture.summary.scan_run_id)
            )
            .is_some()
        );
        assert!(text_rect(&output, "1 errors").is_some());
        assert!(workflows.app.sources_ui.source_action.is_none());
    }
}

#[test]
fn enabling_a_source_retains_its_scan_summary_for_discovery_and_reports_errors() {
    let fixture = Fixture::new();
    let ctx = egui::Context::default();
    let mut workflows = fixture.workflows(&ctx, true);
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut summary = fixture.summary.clone();
    summary.folder_errors.push((
        fixture.sources[1].path.clone(),
        "synthetic permission denied".into(),
    ));
    let outcome = crate::platform_source_actions::SourceActionOutcome::SetEnabled(
        archivefs_core::SetSourceFolderEnabledOutcome {
            source: fixture.sources[1].clone(),
            scan: Some(summary.clone()),
        },
    );
    let message = crate::platform_source_actions::source_action_success_message(&outcome);
    assert!(message.contains("failed"), "{message}");
    workflows.app.sources_ui.source_action =
        Some(crate::platform_source_actions::RunningSourceAction {
            action: SourceAction::SetEnabled {
                path: fixture.sources[1].path.clone(),
                enabled: true,
            },
            receiver,
            worker: None,
        });
    sender.send(Ok(outcome)).unwrap();
    workflows.app.poll_source_action(&ctx);
    assert_eq!(
        workflows.app.sources_ui.pending_source_scan_summary,
        Some(summary)
    );
    assert!(matches!(
        workflows
            .app
            .sources_ui
            .sources_last_scan
            .as_ref()
            .unwrap()
            .scope,
        crate::platform_source_actions::SourcesScanScope::One(_)
    ));
}

#[test]
fn sources_navigation_and_scan_controls_work_at_1024_by_600() {
    let fixture = Fixture::new();
    let before = std::fs::read(&fixture.database).unwrap();
    let ctx = egui::Context::default();
    let mut workflows = fixture.workflows(&ctx, true);
    let mut activity = crate::gui_v2::activity::Activity::default();
    let mut render = |workflows: &mut NativeWorkflows, events| {
        frame(&ctx, events, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    assert!(workflows.show_native_sources(ui, &mut activity).is_none());
                });
            });
        })
    };
    let output = render(&mut workflows, vec![]);
    for label in ["Scan all enabled", "Discovery", "Configured folders"] {
        let rect = text_rect(&output, label).unwrap();
        assert!(rect.max.x < 1024.0 && rect.max.y < 600.0);
    }
    let pos = text_rect(&output, "Discovery").unwrap().center();
    for pressed in [true, false] {
        render(
            &mut workflows,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    assert!(workflows.sources_discovery);
    let output = render(&mut workflows, vec![]);
    assert!(text_rect(&output, "Collection Discovery").is_some());
    assert!(workflows.app.sources_ui.source_action.is_none());
    assert_eq!(before, std::fs::read(&fixture.database).unwrap());
}

#[test]
fn first_scan_of_configured_roots_can_request_a_read_only_scope_review() {
    let fixture = Fixture::new();
    let ctx = egui::Context::default();
    let mut workflows = fixture.workflows(&ctx, false);
    std::fs::remove_file(&fixture.database).unwrap();
    workflows.app.database_state = DatabaseState::NotCreated {
        database_path: fixture.database.clone(),
    };
    workflows
        .review_or_start_source_action(ctx, SourceAction::ScanOne(fixture.sources[0].path.clone()));
    assert!(
        workflows.collection.refusal.is_none(),
        "a configured root must still support the existing first scan"
    );
    assert!(
        workflows.app.sources_ui.source_action.is_none(),
        "scope review must not start a scan"
    );
    assert!(!fixture.database.exists());
}

#[test]
fn initial_scope_preserves_exclusions_and_refuses_a_database_that_appears_later() {
    let fixture = Fixture::new();
    std::fs::remove_file(&fixture.database).unwrap();
    let review = ScanReview::without_database_at(
        &fixture.config,
        fixture.database.clone(),
        7,
        SourceAction::ScanAll,
    )
    .unwrap();
    assert!(!review.database_existed);
    assert!(review.selected(&review.sources[0]));
    assert!(!review.selected(&review.sources[1]));
    review
        .validate_saved_at(&fixture.config, &fixture.database)
        .unwrap();
    assert!(!fixture.database.exists());
    std::fs::write(&fixture.database, b"another writer created this").unwrap();
    assert!(
        review
            .validate_saved_at(&fixture.config, &fixture.database)
            .unwrap_err()
            .contains("appeared")
    );
    assert_eq!(
        std::fs::read(&fixture.database).unwrap(),
        b"another writer created this"
    );
    std::fs::remove_file(&fixture.database).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            fixture._root.path().join("absent-target"),
            &fixture.database,
        )
        .unwrap();
        assert!(
            review
                .validate_saved_at(&fixture.config, &fixture.database)
                .unwrap_err()
                .contains("appeared")
        );
    }
}

#[test]
fn late_initial_scope_result_cannot_replace_a_newer_library_generation() {
    let fixture = Fixture::new();
    let ctx = egui::Context::default();
    let mut workflows = fixture.workflows(&ctx, false);
    std::fs::remove_file(&fixture.database).unwrap();
    let review = ScanReview::without_database_at(
        &fixture.config,
        fixture.database.clone(),
        7,
        SourceAction::ScanAll,
    )
    .unwrap();
    let (sender, receiver) = mpsc::channel();
    workflows.collection.scope_job = Some(receiver);
    workflows.app.database_state = DatabaseState::NotCreated {
        database_path: fixture.database.clone(),
    };
    workflows.app.database_generation.0 = 8;
    sender.send(Ok(review)).unwrap();
    modal_frame(&mut workflows, &ctx, vec![]);
    assert!(workflows.collection.review.is_none());
    assert!(
        workflows
            .collection
            .refusal
            .as_ref()
            .unwrap()
            .contains("changed while loading")
    );
    assert!(workflows.app.sources_ui.source_action.is_none());
    assert!(!fixture.database.exists());
}

#[test]
fn running_source_scan_reports_indeterminate_progress_without_offering_false_cancellation() {
    let fixture = Fixture::new();
    let ctx = egui::Context::default();
    let mut workflows = fixture.workflows(&ctx, true);
    let (_sender, receiver) = mpsc::channel();
    workflows.app.sources_ui.source_action =
        Some(crate::platform_source_actions::RunningSourceAction {
            action: SourceAction::ScanOne(fixture.sources[0].path.clone()),
            receiver,
            worker: None,
        });
    workflows.sources_discovery = true;
    let mut activity = crate::gui_v2::activity::Activity::default();
    let output = frame(&ctx, vec![], |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            assert!(workflows.show_native_sources(ui, &mut activity).is_none());
        });
    });
    assert!(text_rect(&output, "No incremental counts or cancellation").is_some());
    assert!(text_rect(&output, "Scanning source").is_some());
    assert!(text_rect(&output, "Cancel").is_none());
}
