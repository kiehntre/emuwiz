//! Novice task shell. Routes only: feature state and operations stay with their owners.
use crate::app_shell::ShellRequest;
use crate::navigation::NavClick;
use crate::ui::components as widgets;
use crate::{MainView, ToolsOverlay};
use eframe::egui;

pub(crate) const TASKS: [(MainView, &str, &str); 6] = [
    (
        MainView::Sources,
        "Add My Games",
        "Choose folders containing your games; your files stay where they are.",
    ),
    (
        MainView::CheckGames,
        "Check My Games",
        "Check a platform's games for missing or damaged files without changing them.",
    ),
    (
        MainView::Problems,
        "Fix Problems",
        "Review problems and choose which suggested fixes to approve.",
    ),
    (
        MainView::CanonicalOrganisation,
        "Make a Playing Library",
        "Prepare an organised selection of games for playing.",
    ),
    (
        MainView::CheatsMods,
        "Add Mods & Cheats",
        "Choose a game to add supported mods or cheats.",
    ),
    (
        MainView::ReadyToPlay,
        "Play",
        "Find a ready game and start playing, or see what it still needs.",
    ),
];

pub(crate) fn show_home(ui: &mut egui::Ui) -> Option<MainView> {
    widgets::workflow_header(ui, "Welcome to EmuWiz", "What would you like to do?");
    let mut target = None;
    // One column stays legible with large text and on small displays.
    for (view, title, explanation) in TASKS {
        widgets::full_width_card(ui, |ui| {
            ui.heading(title);
            ui.label(explanation);
            ui.add_space(8.0);
            if primary_button(ui, title, true).clicked() {
                target = Some(view);
            }
        });
        ui.add_space(16.0);
    }
    target
}

pub(crate) fn primary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(
            egui::RichText::new(label)
                .size(18.0)
                .color(egui::Color32::WHITE),
        )
        .fill(crate::ui::theme::ACCENT)
        .min_size(egui::vec2(220.0, 44.0)),
    )
}

pub(crate) fn show_shell(
    ctx: &egui::Context,
    view: MainView,
    overlay: ToolsOverlay,
) -> Option<ShellRequest> {
    let mut request = None;
    egui::TopBottomPanel::top("simple_location").show(ctx, |ui| {
        readability(ui);
        let id = egui::Id::new("simple-back-trail");
        let mut trail = ctx
            .data_mut(|data| data.get_temp::<Vec<MainView>>(id))
            .unwrap_or_else(|| vec![MainView::Home]);
        if trail.last() != Some(&view) {
            trail.push(view);
        }
        if trail.len() > 20 {
            trail.remove(0);
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    view != MainView::Home || overlay != ToolsOverlay::None,
                    egui::Button::new("← Back"),
                )
                .clicked()
            {
                let target = if overlay != ToolsOverlay::None {
                    view
                } else {
                    trail.pop();
                    trail.last().copied().unwrap_or(MainView::Home)
                };
                request = Some(ShellRequest::Navigate(NavClick::View(target)));
            }
            if ui.button("Home").clicked() {
                trail.clear();
                trail.push(MainView::Home);
                request = Some(ShellRequest::Navigate(NavClick::View(MainView::Home)));
            }
            ui.label(format!("› {}", task_title(view)));
        });
        if let Some((_, _, explanation)) = TASKS.iter().find(|(target, _, _)| *target == view) {
            ui.label(*explanation);
        }
        ctx.data_mut(|data| data.insert_temp(id, trail));
    });
    egui::SidePanel::left("simple_navigation")
        .resizable(false)
        .exact_width(260.0)
        .show(ctx, |ui| {
            ui.add_space(16.0);
            ui.heading("EmuWiz");
            ui.label("Simple Mode");
            ui.add_space(16.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (target, label) in [(MainView::Home, "Home"), (MainView::Library, "My Games")]
                    .into_iter()
                    .chain(TASKS.map(|(target, label, _)| (target, label)))
                {
                    if ui
                        .add(
                            egui::Button::selectable(
                                view == target && overlay == ToolsOverlay::None,
                                egui::RichText::new(label).size(18.0),
                            )
                            .min_size(egui::vec2(ui.available_width(), 44.0)),
                        )
                        .clicked()
                    {
                        request = Some(ShellRequest::Navigate(NavClick::View(target)));
                    }
                }
                ui.add_space(20.0);
                if ui.button("Set up emulators").clicked() {
                    request = Some(ShellRequest::Navigate(NavClick::View(
                        MainView::EmulatorSetup,
                    )));
                }
                ui.menu_button("Advanced / More tools", |ui| {
                    for (target, label) in [
                        (MainView::DatSources, "DATs & Verification"),
                        (MainView::IdentifyRename, "Identify & Rename"),
                        (MainView::SourcesDiscovery, "Sources / Discovery"),
                        (MainView::Settings, "Settings"),
                    ] {
                        if ui.button(label).clicked() {
                            request = Some(ShellRequest::Navigate(NavClick::View(target)));
                            ui.close();
                        }
                    }
                    if ui.button("Advanced View — all tools").clicked() {
                        request = Some(ShellRequest::GamerAdvanced);
                        ui.close();
                    }
                    if ui.button("Gamer View").clicked() {
                        request = Some(ShellRequest::ReturnToGamerView);
                        ui.close();
                    }
                });
            });
        });
    request
}

fn task_title(view: MainView) -> &'static str {
    TASKS
        .iter()
        .find(|(target, _, _)| *target == view)
        .map(|(_, title, _)| *title)
        .unwrap_or_else(|| {
            if view == MainView::Library {
                "My Games"
            } else {
                crate::navigation::main_view_title(view)
            }
        })
}

pub(crate) fn readability(ui: &mut egui::Ui) {
    let style = ui.style_mut();
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(18.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(17.0));
    style.spacing.interact_size.y = 36.0;
    style.spacing.item_spacing.y = 10.0;
}

pub(crate) fn active(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp::<bool>(egui::Id::new("simple-mode-active")))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn simple_default_tasks_reach_existing_workflows() {
        assert_eq!(crate::GuiMode::default(), crate::GuiMode::Simple);
        assert_eq!(TASKS.len(), 6);
        assert_eq!(TASKS[1].0, MainView::CheckGames);
        assert_eq!(
            crate::navigation::main_view_title(TASKS[1].0),
            "Check My Games"
        );
        assert!(crate::navigation::main_view_uses_page_scroll(TASKS[1].0));
        for (_, title, explanation) in TASKS {
            for jargon in ["DAT", "provider", "snapshot", "provenance", "catalogue"] {
                assert!(!title.contains(jargon) && !explanation.contains(jargon));
            }
        }
    }

    #[test]
    fn simple_home_check_my_games_is_a_real_button() {
        let ctx = egui::Context::default();
        let frame = |events| {
            let mut action = None;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200.0, 1800.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        action = show_home(ui);
                    });
                },
            );
            (output, action)
        };
        let _ = frame(vec![]);
        let (output, _) = frame(vec![]);
        let pos = output
            .shapes
            .iter()
            .rev()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Check My Games" => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .expect("Check My Games button");
        let (_, action) = frame(vec![
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
        ]);
        assert_eq!(action, Some(MainView::CheckGames));
    }
    #[test]
    fn simple_home_and_shell_render_without_panicking() {
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| {
            assert!(show_shell(ctx, MainView::Home, ToolsOverlay::None).is_none());
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(show_home(ui).is_none());
            });
        });
    }

    #[test]
    fn simple_task_navigation_does_not_restore_technical_tabs_or_change_mode() {
        let mut app = crate::tests::app_for_operation_tests();
        app.ui_mode = crate::GuiMode::Simple;
        app.sources_tab = crate::SourcesTab::Dats;
        app.navigate_to_main_view(MainView::Sources);
        assert_eq!(app.view, MainView::Sources);
        app.library_tab = crate::LibraryTab::Health;
        app.navigate_to_main_view(MainView::Library);
        assert_eq!(app.view, MainView::Library);
        app.review_identity("/tmp/example-game.zip".into());
        assert_eq!(app.ui_mode, crate::GuiMode::Simple);
        assert_eq!(app.view, MainView::Selected);
        assert!(app.sources_ui.dat_sources_page.is_none());
        app.navigate_to_main_view(MainView::CheckGames);
        assert_eq!(app.view, MainView::CheckGames);
        assert!(
            app.sources_ui.dat_sources_page.is_none(),
            "navigation itself must not load or mutate data"
        );
    }

    #[test]
    fn simple_sections_have_location_back_and_explanations() {
        for (view, title, explanation) in TASKS {
            let ctx = egui::Context::default();
            let frame = || {
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1200.0, 1000.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        assert!(show_shell(ctx, view, ToolsOverlay::None).is_none());
                    },
                )
            };
            let _ = frame();
            let output = frame();
            let text: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                    _ => None,
                })
                .collect();
            assert!(text.iter().any(|text| text == "← Back"));
            assert!(text.iter().any(|text| text.contains(title)));
            assert!(
                text.iter().any(|text| text == explanation),
                "{title}: {text:?}"
            );
            assert!(text.iter().any(|text| text == "Advanced / More tools"));
        }
    }
}
