use super::super::organisation::OrganisationView;
use super::*;

#[test]
fn blue_frontend_profiles_navigation_and_keyboard_work_at_supported_sizes() {
    for size in [[1440.0, 900.0], [1024.0, 600.0]] {
        let context = egui::Context::default();
        let mut app = organisation_app(&context);
        frame(&context, &mut app, size);
        let output = frame(&context, &mut app, size);
        let target = text_bounds(&output, "Frontend export profiles");
        assert!(!target.is_empty(), "{:?}", text(&output));
        let point = target[0].center();
        frame_with(
            &context,
            &mut app,
            size,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
        );
        frame_with(
            &context,
            &mut app,
            size,
            vec![egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        );
        assert_eq!(app.organisation.view, OrganisationView::FrontendExport);
        settle_recovery_load(&context, &mut app, size);
        let output = frame(&context, &mut app, size);
        let labels = text(&output).join("\n");
        assert!(labels.contains("Create a RomM-ready library"));
        assert!(labels.contains("Create an ES-DE-ready library"));
        assert!(!labels.contains("Open specialist interface"));
        let point = text_bounds(&output, "Create an ES-DE-ready library")[0].center();
        frame_with(
            &context,
            &mut app,
            size,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
        );
        frame_with(
            &context,
            &mut app,
            size,
            vec![egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        );
        assert_eq!(
            app.organisation.frontend_export.target,
            Some(archivefs_core::publisher_profile::PublisherFrontend::EsDe)
        );
        // Tab moves focus into the destination input after the target controls.
        frame_with(
            &context,
            &mut app,
            size,
            vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        assert!(context.memory(|memory| memory.focused().is_some()));
        assert_eq!(app.organisation.view, OrganisationView::FrontendExport);
        assert!(context.content_rect().width() <= size[0]);
        assert!(context.content_rect().height() <= size[1]);
    }
}

#[test]
fn frontend_export_job_completes_after_leaving_organisation() {
    let context = egui::Context::default();
    let mut app = organisation_app(&context);
    app.organisation.frontend_export.start(
        crate::publisher_profile_page::PublisherJob::Preview,
        context.clone(),
    );
    app.router.current = Route::Section(Section::Home);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.organisation.frontend_export.is_busy() {
        // frame() exercises presentation only; mirror the production ui() poll.
        app.poll(&context);
        frame(&context, &mut app, [1024.0, 600.0]);
        assert!(std::time::Instant::now() < deadline);
    }
    assert_eq!(
        app.organisation.frontend_export.error.as_deref(),
        Some("Choose a target profile first.")
    );
}

#[test]
fn frontend_export_retains_terminal_outcome_when_playing_library_changes() {
    use crate::publisher_profile_page::{PublisherExecutionStage, PublisherProfilePageState};
    use archivefs_core::playing_library::{PlayingLibraryPlan, PlayingLibraryPolicy};
    let context = egui::Context::default();
    let mut app = organisation_app(&context);
    app.organisation.view = OrganisationView::FrontendExport;
    let mut page = PublisherProfilePageState::default();
    page.source_plan = Some(PlayingLibraryPlan {
        destination_root: "/synthetic/source".into(),
        policy: PlayingLibraryPolicy::default(),
        archives_examined: 0,
        families_examined: 0,
        elected_games: vec![],
        unresolved_groups: vec![],
        exclusions: vec![],
        singleton_families: 0,
        conflicts: vec![],
        operations: vec![],
        rejected_launchers: vec![],
    });
    page.execution_stage = PublisherExecutionStage::NeedsReconciliation;
    page.execution_error =
        Some("Synthetic partial write: inspect transaction synthetic-123".into());
    app.organisation.frontend_export = page;
    settle_recovery_load(&context, &mut app, [1024.0, 600.0]);
    assert_eq!(
        app.organisation.frontend_export.execution_stage,
        PublisherExecutionStage::NeedsReconciliation
    );
    assert!(
        app.organisation
            .frontend_export
            .execution_error
            .as_ref()
            .unwrap()
            .contains("synthetic-123")
    );
    assert!(app.organisation.frontend_export.source_plan.is_some());
}

/// The page's first frames load the recovery list on a worker.
fn settle_recovery_load(context: &egui::Context, app: &mut App, size: [f32; 2]) {
    frame(context, app, size);
    for _ in 0..500 {
        let page = &app.organisation.frontend_export;
        if page.recovery.loaded() && !page.is_busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        frame(context, app, size);
    }
    panic!("recovery list did not load");
}

#[test]
fn blue_loads_recovered_exports_when_the_page_opens() {
    let context = egui::Context::default();
    let mut app = organisation_app(&context);
    app.organisation.view = OrganisationView::FrontendExport;
    assert!(!app.organisation.frontend_export.recovery.loaded());
    settle_recovery_load(&context, &mut app, [1440.0, 900.0]);
}
