//! Native Blue host for the established Library Views editor and worker.
//! Definitions, plans, mutations and history remain owned by the existing engine.
use super::{
    App,
    native_workflows::NativeWorkflows,
    routes::{Route, Section, ToolPage},
};
use crate::library_view_controller::LibraryViewAction;
use archivefs_core::{LibraryViewConfig, SourceFolderView};
use eframe::egui;
use std::sync::mpsc::{self, Receiver, TryRecvError};

type Loaded = Result<(Vec<LibraryViewConfig>, Vec<SourceFolderView>), String>;

#[derive(Default)]
pub(super) struct State {
    started: bool,
    receiver: Option<Receiver<Loaded>>,
    sources: Vec<SourceFolderView>,
    ready: bool,
    error: Option<String>,
    confirmation: Option<LibraryViewAction>,
}

impl State {
    fn refresh(&mut self, context: &egui::Context) {
        if self.receiver.is_some() {
            return;
        }
        self.started = true;
        self.ready = false;
        self.error = None;
        self.confirmation = None;
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        let context = context.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let views = archivefs_core::load_library_view_configs_default()
                    .map_err(|error| format!("Saved views could not be read: {error}"))?;
                let sources = archivefs_core::list_source_folder_views_default()
                    .map_err(|error| format!("Library sources unavailable: {error}. Open Browse & Play or Sources to check the catalogue, then Refresh."))?;
                Ok((views, sources))
            })();
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    fn poll(&mut self, app: &mut crate::ArchiveFsApp) {
        let result = match self.receiver.as_ref().map(Receiver::try_recv) {
            Some(Ok(result)) => result,
            Some(Err(TryRecvError::Disconnected)) => Err(
                "Library Views loader stopped before returning a result. Refresh to retry.".into(),
            ),
            _ => return,
        };
        self.receiver = None;
        match result {
            Ok((views, sources)) => {
                app.library_views = views;
                self.sources = sources;
                self.error = None;
                self.ready = true;
                app.library_view_last_plan = None;
            }
            Err(error) => {
                self.ready = false;
                self.error = Some(error);
                app.library_view_last_plan = None;
            }
        }
    }
}

fn confirmation_is_current(action: &LibraryViewAction, app: &crate::ArchiveFsApp) -> bool {
    let (LibraryViewAction::Apply(id) | LibraryViewAction::Repair(id)) = action else {
        return false;
    };
    app.library_view_last_plan
        .as_ref()
        .is_some_and(|(previewed, plan)| {
            previewed.id == *id
                && plan.is_safe_to_apply()
                && app.library_views.iter().any(|view| view == previewed)
        })
}

impl App {
    pub(super) fn library_views_page(&mut self, ui: &mut egui::Ui) {
        let mut navigation = None;
        ui.horizontal_wrapped(|ui| {
            if ui.button("Browse & Play").clicked() {
                navigation = Some(Route::Section(Section::Games));
            }
            if ui.button("View History").clicked() {
                navigation = Some(Route::Tool(ToolPage::ViewHistory));
            }
        });
        if let Some(route) = navigation {
            self.go(route);
        }
        ui.label("Saved views create managed symlinks to existing archives. They do not move ROMs, change platform or DAT evidence, or prove launch readiness.");
        let workflows = self
            .native_workflows
            .get_or_insert_with(|| NativeWorkflows::new(ui.ctx().clone()));
        let state = &mut self.tools.library_views;
        if !state.started {
            state.refresh(ui.ctx());
        }
        state.poll(&mut workflows.app);
        let app = &mut workflows.app;
        let busy = !app.library_view_action_available() || self.load_job.is_some();
        if ui
            .add_enabled(
                !busy && state.receiver.is_none(),
                egui::Button::new("Refresh views"),
            )
            .clicked()
        {
            app.library_view_last_plan = None;
            state.refresh(ui.ctx());
        }
        if let Some(error) = &state.error {
            crate::ui::components::banner(
                ui,
                "Library Views unavailable",
                error,
                crate::ui::components::StatusTone::Blocked,
            );
            ui.label("Existing definitions are retained. Editing and Apply stay disabled until Refresh succeeds. Specialist access remains available from All Tools.");
        }
        if state.receiver.is_some() {
            ui.spinner();
            ui.label("Loading saved views and catalogue sources...");
        }
        if let Some(feedback) = &app.feedback {
            // Keep the established report's failed/skipped counts visible without
            // translating an outer Ok into a stronger success claim.
            ui.label(&feedback.message);
            if let Some(details) = &feedback.more_information {
                ui.label(details);
            }
        }
        if state.ready {
            let action = crate::administration_pages::show_library_views_page(
                ui,
                &app.library_views,
                &state.sources,
                busy || state.confirmation.is_some(),
                app.library_view_last_plan.as_ref(),
                app.library_view_focus_archive.as_deref(),
                &mut app.library_view_plan_filter,
                &mut app.library_view_form_dialog,
                &mut app.library_view_remove_dialog,
                &mut app.clipboard,
            );
            if let Some(action) = action {
                if matches!(
                    action,
                    LibraryViewAction::Apply(_) | LibraryViewAction::Repair(_)
                ) {
                    state.confirmation = Some(action);
                } else {
                    app.start_library_view_action(ui.ctx().clone(), action);
                }
            }
        }
        let mut confirmed = false;
        let mut cancelled = false;
        if let Some(action) = &state.confirmation {
            egui::Window::new("Confirm Library View changes")
                .collapsible(false).resizable(false).show(ui.ctx(), |ui| {
                    if let Some((view, plan)) = &app.library_view_last_plan {
                        ui.label(format!("View: {}", view.name));
                        ui.label(format!("Destination: {}", view.destination_root.display()));
                        ui.label(format!("{} to create, {} to repair, {} stale links to remove", plan.counts.create, plan.counts.repair, plan.counts.remove));
                    }
                    if !confirmation_is_current(action, app) {
                        ui.label("This preview is no longer current. Cancel and run Preview again.");
                    }
                    ui.label("Apply the displayed preview's managed symlink changes? The existing engine rechecks its plan and refuses unsafe destinations. Original archives are not rewritten.");
                    ui.horizontal_wrapped(|ui| {
                        confirmed = ui.add_enabled(!busy && state.ready && confirmation_is_current(action, app), egui::Button::new("Confirm changes")).clicked();
                        cancelled = ui.button("Cancel").clicked();
                    });
                });
        }
        if cancelled {
            state.confirmation = None;
        }
        if confirmed
            && let Some(action) = state.confirmation.take()
            && confirmation_is_current(&action, app)
        {
            app.start_library_view_action(ui.ctx().clone(), action);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_load_retains_definitions_and_refuses_ready_state() {
        let mut app = crate::tests::app_for_operation_tests();
        let before = app.library_views.clone();
        let (sender, receiver) = mpsc::channel();
        sender.send(Err("invalid saved view JSON".into())).unwrap();
        let mut state = State {
            receiver: Some(receiver),
            ..Default::default()
        };
        state.poll(&mut app);
        assert!(!state.ready);
        assert_eq!(state.error.as_deref(), Some("invalid saved view JSON"));
        assert_eq!(app.library_views, before);
        assert!(app.library_view_action.is_none());
    }

    #[test]
    fn disconnected_loader_is_not_an_empty_success() {
        let mut app = crate::tests::app_for_operation_tests();
        let (sender, receiver) = mpsc::channel::<Loaded>();
        drop(sender);
        let mut state = State {
            receiver: Some(receiver),
            ..Default::default()
        };
        state.poll(&mut app);
        assert!(!state.ready);
        assert!(state.error.as_deref().unwrap().contains("stopped"));
    }

    #[test]
    fn ready_load_preserves_saved_definitions_without_starting_actions() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("views.json");
        let original = vec![LibraryViewConfig {
            id: "stable-id".into(),
            name: "My handhelds".into(),
            destination_root: dir.path().join("links"),
            enabled: false,
            source_folders: vec![dir.path().join("source")],
            platforms: vec!["Game Boy".into(), "Game Gear".into()],
            layout_template: archivefs_core::LibraryViewLayoutTemplate::PlatformFilename,
            profile: Default::default(),
        }];
        archivefs_core::save_library_view_configs_to(&config, &original).unwrap();
        let loaded = archivefs_core::load_library_view_configs_from(&config).unwrap();
        let bytes = std::fs::read(&config).unwrap();
        let (sender, receiver) = mpsc::channel();
        sender.send(Ok((loaded, Vec::new()))).unwrap();
        let mut app = crate::tests::app_for_operation_tests();
        let mut state = State {
            receiver: Some(receiver),
            ..Default::default()
        };
        state.poll(&mut app);
        assert!(state.ready);
        assert_eq!(app.library_views, original);
        assert!(app.library_view_action.is_none());
        assert_eq!(std::fs::read(&config).unwrap(), bytes);
    }

    fn texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Rect)> {
        fn collect(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push((text.galley.text().into(), text.visual_bounding_rect()))
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in shapes {
            collect(&shape.shape, &mut out);
        }
        out
    }

    fn frame(
        context: &egui::Context,
        app: &mut App,
        size: egui::Vec2,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| app.library_views_page(ui));
            },
        )
    }

    fn ready_app(context: &egui::Context) -> App {
        let mut app = super::super::tests::fixture(context);
        app.native_workflows = Some(NativeWorkflows::new(context.clone()));
        app.tools.library_views.started = true;
        app.tools.library_views.ready = true;
        app
    }

    #[test]
    fn native_empty_editor_and_navigation_fit_small_viewport_without_starting_work() {
        for size in [egui::vec2(1024.0, 600.0), egui::vec2(1440.0, 900.0)] {
            let context = egui::Context::default();
            let mut app = ready_app(&context);
            frame(&context, &mut app, size, vec![]);
            let output = frame(&context, &mut app, size, vec![]);
            let labels = texts(&output.shapes);
            for wanted in [
                "Browse & Play",
                "View History",
                "Add View",
                "No library views are configured yet.",
            ] {
                let (_, bounds) = labels
                    .iter()
                    .find(|(text, _)| text == wanted)
                    .unwrap_or_else(|| panic!("Missing {wanted}: {labels:?}"));
                assert!(
                    bounds.max.x <= size.x && bounds.max.y <= size.y,
                    "{wanted} outside viewport"
                );
            }
            let point = labels
                .iter()
                .find(|(text, _)| text == "Add View")
                .unwrap()
                .1
                .center();
            for pressed in [true, false] {
                frame(
                    &context,
                    &mut app,
                    size,
                    vec![
                        egui::Event::PointerMoved(point),
                        egui::Event::PointerButton {
                            pos: point,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
            assert!(
                app.native_workflows
                    .as_ref()
                    .unwrap()
                    .app
                    .library_view_form_dialog
                    .is_some()
            );
            assert!(
                app.native_workflows
                    .as_ref()
                    .unwrap()
                    .app
                    .library_view_action
                    .is_none()
            );
        }
    }

    #[test]
    fn unavailable_editor_never_offers_an_empty_definition_editor() {
        let context = egui::Context::default();
        let mut app = ready_app(&context);
        app.tools.library_views.ready = false;
        app.tools.library_views.error = Some("permission denied reading saved views".into());
        let output = frame(&context, &mut app, egui::vec2(1024.0, 600.0), vec![]);
        let labels = texts(&output.shapes);
        assert!(
            labels
                .iter()
                .any(|(text, _)| text.contains("permission denied"))
        );
        assert!(!labels.iter().any(|(text, _)| text == "Add View"));
        assert!(
            app.native_workflows
                .as_ref()
                .unwrap()
                .app
                .library_view_action
                .is_none()
        );
    }

    #[test]
    fn all_tools_exposes_native_views_without_specialist_handoff() {
        let tool = super::super::all_tools::groups()
            .into_iter()
            .flat_map(|group| group.tools)
            .find(|tool| tool.route == Route::Tool(ToolPage::LibraryViews))
            .unwrap();
        assert_eq!(tool.safety, super::super::all_tools::ToolSafety::AsksFirst);
        assert_eq!(ToolPage::LibraryViews.home(), Section::Build);
    }

    fn config() -> LibraryViewConfig {
        LibraryViewConfig {
            id: "v1".into(),
            name: "My view".into(),
            destination_root: "/fixture/links".into(),
            enabled: true,
            source_folders: vec![],
            platforms: vec!["SNES".into()],
            layout_template: archivefs_core::LibraryViewLayoutTemplate::PlatformFilename,
            profile: Default::default(),
        }
    }

    fn plan(entries: usize) -> archivefs_core::LibraryViewPlan {
        archivefs_core::LibraryViewPlan {
            view_id: "v1".into(),
            destination_root: "/fixture/links".into(),
            counts: archivefs_core::LibraryViewPlanCounts {
                create: entries,
                ..Default::default()
            },
            entries: (0..entries)
                .map(|i| archivefs_core::LibraryViewPlanEntry {
                    action: archivefs_core::LibraryViewPlanAction::Create,
                    archive_path: Some(format!("/fixture/source/{i}.zip").into()),
                    relative_link_path: Some(format!("SNES/{i}.zip").into()),
                    destination_path: Some(format!("/fixture/links/SNES/{i}.zip").into()),
                    platform: Some("SNES".into()),
                    reason: None,
                    colliding_with: None,
                    source_folder_path: None,
                    archive_identity: None,
                })
                .collect(),
            unsafe_root_error: None,
            profile_fingerprint: String::new(),
            fingerprint_conflict: None,
            profile_error: None,
        }
    }

    #[test]
    fn stale_or_missing_preview_cannot_confirm_and_never_starts_a_worker() {
        let mut app = crate::tests::app_for_operation_tests();
        let action = LibraryViewAction::Apply("v1".into());
        app.library_views = vec![config()];
        assert!(!confirmation_is_current(&action, &app));
        app.library_view_last_plan = Some((config(), plan(2)));
        assert!(confirmation_is_current(&action, &app));
        app.library_views[0].platforms.push("NES".into());
        assert!(!confirmation_is_current(&action, &app));
        app.library_views = vec![config()];
        app.library_view_last_plan
            .as_mut()
            .unwrap()
            .1
            .unsafe_root_error = Some("source overlap".into());
        assert!(!confirmation_is_current(&action, &app));
        assert!(app.library_view_action.is_none());
    }

    #[test]
    fn large_preview_paginates_widgets_without_truncating_plan_or_counts() {
        let context = egui::Context::default();
        let plan = plan(10_000);
        let mut filter = crate::library_view_controller::LibraryViewPlanFilter::All;
        let output = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                crate::administration_pages::show_library_view_plan_summary(ui, &plan, &mut filter)
            });
        });
        let labels = texts(&output.shapes);
        assert!(
            labels
                .iter()
                .any(|(text, _)| text.contains("page 1 of 100"))
        );
        assert_eq!(plan.entries.len(), 10_000);
        assert_eq!(plan.counts.create, 10_000);
        assert!(
            labels
                .iter()
                .filter(|(text, _)| text.starts_with("/fixture/links/"))
                .count()
                <= 100
        );
    }
}
