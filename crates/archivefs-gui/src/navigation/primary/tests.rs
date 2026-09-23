use super::*;

fn text_rect(output: &egui::FullOutput, label: &str) -> Option<egui::Rect> {
    output.shapes.iter().find_map(|shape| match &shape.shape {
        egui::Shape::Text(text) if text.galley.text() == label => {
            Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
        }
        _ => None,
    })
}

fn render(
    context: &egui::Context,
    view: MainView,
    overlay: ToolsOverlay,
    events: Vec<egui::Event>,
    sidebar: bool,
) -> (egui::FullOutput, Option<NavClick>) {
    let mut clicked = None;
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(if sidebar { 218.0 } else { 1400.0 }, 500.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                clicked = if sidebar {
                    show_sidebar(ui, view, overlay)
                } else {
                    show_subnavigation(ui, view, overlay, true)
                };
            });
        },
    );
    (output, clicked)
}

fn click(label: &str, view: MainView, overlay: ToolsOverlay, expected: NavClick) {
    let ctx = egui::Context::default();
    let _ = render(&ctx, view, overlay, vec![], false);
    let (output, _) = render(&ctx, view, overlay, vec![], false);
    let pos = text_rect(&output, label)
        .unwrap_or_else(|| panic!("missing {label}"))
        .center();
    let (_, actual) = render(
        &ctx,
        view,
        overlay,
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ],
        false,
    );
    assert_eq!(actual, Some(expected), "{label}");
}

#[test]
fn eight_primary_destinations_are_visible_and_legacy_rows_are_absent() {
    let expected = [
        "Home",
        "Library",
        "Setup",
        "Organise & Export",
        "Tools",
        "Enhance",
        "Health & Recovery",
        "Settings",
    ];
    assert_eq!(PRIMARY.map(|(_, label, _)| label), expected);
    let ctx = egui::Context::default();
    let (output, _) = render(&ctx, MainView::Home, ToolsOverlay::None, vec![], true);
    for label in expected {
        let rect = text_rect(&output, label).unwrap_or_else(|| panic!("missing {label}"));
        assert!(rect.bottom() < 500.0, "{label} is below the fold");
    }
    for group in ADVANCED_NAV_GROUPS {
        for entry in group.entries {
            if !expected.contains(&entry.label) {
                assert!(
                    text_rect(&output, entry.label).is_none(),
                    "old primary row: {}",
                    entry.label
                );
            }
        }
    }
}

#[test]
fn workshop_exposes_native_museum_converter_and_tape_destinations() {
    let expected = [
        ("Converter", MainView::DiscConversion),
        ("Tape Inspector", MainView::TapeInspector),
        ("Museum", MainView::Museum),
    ];
    for (label, target) in expected {
        click(
            label,
            MainView::DiscConversion,
            ToolsOverlay::None,
            NavClick::View(target),
        );
    }
    assert_eq!(
        destination(MainView::Museum, ToolsOverlay::None),
        Destination::Workshop
    );
    assert_eq!(
        destination(MainView::TapeInspector, ToolsOverlay::None),
        Destination::Workshop
    );
}

#[test]
fn library_dat_and_readiness_entries_are_real_click_targets() {
    for entry in entries(Destination::Library) {
        click(
            entry.label,
            MainView::Library,
            ToolsOverlay::None,
            entry.click,
        );
    }
    assert_eq!(
        entries(Destination::Library)[1].label,
        "DATs & Verification"
    );
}

#[test]
fn setup_emulators_and_bios_are_real_click_targets() {
    for entry in entries(Destination::Setup) {
        click(
            entry.label,
            MainView::EmulatorSetup,
            ToolsOverlay::None,
            entry.click,
        );
    }
    for entry in subviews(MainView::EmulatorSetup, ToolsOverlay::None) {
        click(
            entry.label,
            MainView::EmulatorSetup,
            ToolsOverlay::None,
            entry.click,
        );
    }
}

#[test]
fn build_libraries_exposes_both_existing_workflows() {
    for entry in entries(Destination::Organise) {
        click(
            entry.label,
            MainView::CanonicalOrganisation,
            ToolsOverlay::None,
            entry.click,
        );
    }
    let targets = subviews(MainView::PublisherProfiles, ToolsOverlay::None);
    assert_eq!(
        targets[0].click,
        NavClick::View(MainView::CanonicalOrganisation)
    );
    assert_eq!(
        targets[1].click,
        NavClick::View(MainView::PublisherProfiles)
    );
    for entry in targets {
        click(
            entry.label,
            MainView::PublisherProfiles,
            ToolsOverlay::None,
            entry.click,
        );
    }
    assert_eq!(
        destination(MainView::Sources, ToolsOverlay::None),
        Destination::Setup,
        "RomM connections must not become export routes"
    );
}

#[test]
fn enhancements_and_save_vault_are_real_click_targets() {
    for entry in entries(Destination::Enhance) {
        click(
            entry.label,
            MainView::CheatsMods,
            ToolsOverlay::None,
            entry.click,
        );
    }
    assert_eq!(
        entries(Destination::Enhance)[2].click,
        NavClick::Overlay(ToolsOverlay::SaveVault)
    );
}

#[test]
fn problems_and_distinct_diagnostics_are_reachable() {
    for entry in entries(Destination::Health) {
        click(
            entry.label,
            MainView::Doctor,
            ToolsOverlay::None,
            entry.click,
        );
    }
    for entry in subviews(MainView::Doctor, ToolsOverlay::None) {
        click(
            entry.label,
            MainView::Doctor,
            ToolsOverlay::None,
            entry.click,
        );
    }
    let checks = subviews(MainView::Doctor, ToolsOverlay::None);
    assert!(
        checks
            .iter()
            .any(|e| e.click == NavClick::Overlay(ToolsOverlay::DoctorChecks))
    );
    assert!(
        checks
            .iter()
            .any(|e| e.click == NavClick::Overlay(ToolsOverlay::Diagnostics))
    );
}

#[test]
fn advanced_tools_are_in_real_menus_not_primary_rows() {
    for view in [MainView::Library, MainView::Doctor, MainView::Settings] {
        let group = destination(view, ToolsOverlay::None);
        for entry in advanced_entries(group) {
            let ctx = egui::Context::default();
            let _ = render(&ctx, view, ToolsOverlay::None, vec![], false);
            let (output, _) = render(&ctx, view, ToolsOverlay::None, vec![], false);
            let pos = text_rect(&output, "More tools").unwrap().center();
            let events = |pos| {
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::default(),
                    },
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::default(),
                    },
                ]
            };
            let _ = render(&ctx, view, ToolsOverlay::None, events(pos), false);
            let (output, _) = render(&ctx, view, ToolsOverlay::None, vec![], false);
            let pos = text_rect(&output, entry.label)
                .unwrap_or_else(|| panic!("missing advanced {}", entry.label))
                .center();
            let (_, clicked) = render(&ctx, view, ToolsOverlay::None, events(pos), false);
            assert_eq!(clicked, Some(entry.click));
        }
    }
}

#[test]
fn every_old_catalogue_route_has_a_new_entry_or_an_unchanged_page_tab() {
    let mut clicks: Vec<_> = PRIMARY.iter().map(|(_, _, click)| *click).collect();
    for (group, _, _) in PRIMARY {
        clicks.extend(entries(group).iter().map(|e| e.click));
        clicks.extend(advanced_entries(group).iter().map(|e| e.click));
    }
    for (view, _) in PRIMARY_NAVIGATION_DESTINATIONS {
        clicks.extend(subviews(view, ToolsOverlay::None).iter().map(|e| e.click));
    }
    for entry in ADVANCED_NAV_GROUPS.iter().flat_map(|g| g.entries) {
        assert!(
            clicks.contains(&entry.click) || entry.click == NavClick::View(MainView::CheatsMods),
            "lost route: {}",
            entry.label
        );
    }
    for view in [
        MainView::Health,
        MainView::Duplicates,
        MainView::LibraryViews,
        MainView::RecentlyFound,
    ] {
        assert!(library_tab_for_main_view(view).is_some());
        assert_eq!(destination(view, ToolsOverlay::None), Destination::Library);
    }
    assert!(sources_tab_for_main_view(MainView::SourcesDiscovery).is_some());
    assert!(problems_repair_tab_for_main_view(MainView::RepairHistory).is_some());
}

#[test]
fn subview_and_overlay_highlights_follow_the_visible_content() {
    for (view, overlay, parent) in [
        (
            MainView::EmulatorInventory,
            ToolsOverlay::None,
            MainView::EmulatorSetup,
        ),
        (
            MainView::PublisherProfiles,
            ToolsOverlay::None,
            MainView::CanonicalOrganisation,
        ),
        (
            MainView::Library,
            ToolsOverlay::DoctorChecks,
            MainView::Doctor,
        ),
    ] {
        let matches: Vec<_> = entries(destination(view, overlay))
            .iter()
            .filter(|entry| selected(**entry, view, overlay, EnhancementSection::Cheats, true))
            .collect();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].click, NavClick::View(parent));
    }
    for section in [EnhancementSection::Cheats, EnhancementSection::Mods] {
        assert_eq!(
            entries(Destination::Enhance)
                .iter()
                .filter(|entry| selected(
                    **entry,
                    MainView::CheatsMods,
                    ToolsOverlay::None,
                    section,
                    true
                ))
                .count(),
            1
        );
    }
    assert_eq!(
        destination(MainView::Home, ToolsOverlay::SaveVault),
        Destination::Enhance
    );
}
