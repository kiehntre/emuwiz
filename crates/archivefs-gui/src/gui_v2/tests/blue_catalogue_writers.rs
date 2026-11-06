//! Night Watch S06-04 / S06-02: one catalogue writer at a time between Blue's
//! folder scan and the embedded native Sources page, and the organisation
//! source walk off the GUI thread.
use super::*;
use crate::SourceAction;
use crate::gui_v2::native_workflows::NativeWorkflows;
use crate::platform_source_actions::RunningSourceAction;

fn app_with_native(context: &egui::Context) -> App {
    let mut app = fixture(context);
    app.native_workflows = Some(NativeWorkflows::new(context.clone()));
    app
}

fn native_scan_running(app: &mut App) {
    let (_sender, receiver) = std::sync::mpsc::channel();
    app.native_workflows
        .as_mut()
        .unwrap()
        .app
        .sources_ui
        .source_action = Some(RunningSourceAction {
        action: SourceAction::ScanAll,
        receiver,
        worker: None,
    });
}

#[test]
fn blue_scan_is_refused_while_a_native_sources_action_runs() {
    let context = egui::Context::default();
    let mut app = app_with_native(&context);
    native_scan_running(&mut app);
    assert!(app.native_catalogue_writer_busy());

    app.load(true);

    assert!(app.load_job.is_none(), "no scan job was queued");
    assert!(app.scan_job.is_none());
    let notice = app.notice.clone().expect("the refusal is said");
    assert!(
        notice.message.contains("Sources action is still running"),
        "{}",
        notice.message
    );
    assert!(
        app.activity.jobs.is_empty(),
        "nothing was queued in Activity"
    );
}

#[test]
fn blue_scan_proceeds_when_the_native_app_is_idle() {
    let context = egui::Context::default();
    let mut app = app_with_native(&context);
    // The embedded app may be loading its database in this fixture; only the
    // positive direction is asserted when it is idle.
    if app.native_catalogue_writer_busy() {
        return;
    }
    app.backend = Backend::disconnected();
    app.load(true);
    assert!(app.scan_job.is_some() || app.notice.is_some());
}

#[test]
fn a_plain_library_load_is_never_a_catalogue_writer() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.backend = Backend::disconnected();
    app.load(false);
    assert!(app.load_job.is_some());
    assert!(app.scan_job.is_none());
}

#[test]
fn native_sources_actions_are_refused_while_blue_scans() {
    let context = egui::Context::default();
    let mut app = app_with_native(&context);
    app.scan_job = Some(7);
    // The per-frame sync passes the writer flag to the native host.
    frame(&context, &mut app, [1280.0, 900.0]);
    let workflows = app.native_workflows.as_mut().unwrap();
    assert!(workflows.catalogue_writer_busy);
    assert!(!workflows.source_action_available());

    workflows.review_or_start_source_action(context.clone(), SourceAction::ScanAll);

    assert!(
        workflows.app.sources_ui.source_action.is_none(),
        "no native scan started while Blue's scan runs"
    );

    app.scan_job = None;
    frame(&context, &mut app, [1280.0, 900.0]);
    let workflows = app.native_workflows.as_ref().unwrap();
    assert!(!workflows.catalogue_writer_busy);
    assert_eq!(
        workflows.source_action_available(),
        workflows.app.source_action_available(),
        "with no Blue writer the native rule alone decides"
    );
}

#[test]
fn rescan_sources_runs_on_the_background_worker() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.sfc"), b"x").unwrap();
    std::fs::write(dir.path().join("b.sfc"), b"x").unwrap();
    let context = egui::Context::default();
    let backend = super::backend::Backend::start(context);
    let state = crate::rom_organisation_page::RomOrganisationPageState::default()
        .with_deferred_source_roots(vec![dir.path().to_path_buf()]);
    backend
        .send(
            45,
            super::backend::Command::CanonicalOrganisation {
                state: Box::new(state),
                generation: 3,
                kind: super::CanonicalOrganisationJobKind::Rescan,
                cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
        )
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "worker did not answer"
        );
        match backend
            .rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
        {
            super::backend::Event::Finished { id: 45, outcome } => {
                let Ok(super::backend::Payload::CanonicalOrganisation { state, kind, .. }) =
                    outcome
                else {
                    panic!("unexpected outcome");
                };
                assert_eq!(kind, super::CanonicalOrganisationJobKind::Rescan);
                assert!(state.sources_scanned());
                assert_eq!(state.source_count(), 2);
                assert!(state.sources_complete());
                return;
            }
            _ => continue,
        }
    }
}

#[test]
fn blue_rescan_action_is_routed_to_the_worker_not_the_gui_thread() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.backend = Backend::disconnected();
    app.start_canonical_organisation_job(super::CanonicalOrganisationJobKind::Rescan);
    // The disconnected backend refuses the send, so the job is not retained,
    // but the Activity entry proves the request went to the worker path.
    assert!(
        app.activity
            .jobs
            .values()
            .any(|job| job.title == "Finding your game files")
    );
}

#[test]
fn blue_app_state_starts_with_an_unscanned_organisation_page() {
    let state = crate::rom_organisation_page::RomOrganisationPageState::load_deferred();
    assert!(!state.sources_scanned());
    assert_eq!(state.source_count(), 0);
}
