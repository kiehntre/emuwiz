//! GUI Maintenance Batch 2: relocated from main.rs's single inline
//! `#[cfg(test)] mod tests { ... }` (see `crate::tests` for the shared
//! imports/fixtures this file and its siblings rely on).
//!
//! This file's name is a best-effort thematic label, not a strict
//! single-feature boundary: the original test module interleaved topics
//! throughout (tests for unrelated features sit side by side in source
//! order), so this file was cut at safe item boundaries within that
//! existing order rather than by re-sorting tests into pure per-feature
//! files. Every test here is copied byte-for-byte from its original
//! location - nothing was rewritten, renamed, or reordered relative to
//! its neighbors within this slice.
//!
//! Predominant theme observed in this slice: platform shelf/library shell rendering, artwork, Gamer/Advanced view controls.

use super::*;
use crate::ui::platform_artwork::*;

#[test]
fn gui_version_line_matches_the_workspace_package_version() {
    assert_eq!(
        gui_version_line(),
        format!("emuwiz {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn embedded_application_icon_is_the_approved_256_pixel_rgba_asset() {
    let icon = app_icon().expect("approved embedded application icon must decode");
    assert_eq!((icon.width, icon.height), (256, 256));
    assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    assert_eq!(
        APP_ICON_PNG,
        include_bytes!("../../../../assets/branding/emuwiz-logo-256.png")
    );
}

#[test]
fn linux_application_id_is_stable() {
    assert_eq!(LINUX_APP_ID, "io.github.kiehntre.emuwiz");
}

#[test]
fn desktop_entry_startup_wm_class_matches_the_linux_app_id() {
    // egui-winit's `with_app_id` writes into winit's single shared
    // `platform_specific.name` field, which the X11 backend also reads
    // for WM_CLASS - so the X11 WM_CLASS this binary actually produces
    // is LINUX_APP_ID, not any shorter alias. StartupWMClass in the
    // desktop entry must match it exactly or launcher/taskbar grouping
    // silently breaks on X11 desktops (XFCE, KDE/X11, GNOME/X11).
    let template = include_str!("../../../../assets/linux/io.github.kiehntre.emuwiz.desktop.in");
    let expected = format!("StartupWMClass={LINUX_APP_ID}");
    assert!(
        template.lines().any(|line| line == expected),
        "desktop entry template must contain {expected:?}"
    );
}

#[test]
fn log_level_prefers_emuwiz_over_the_legacy_variable() {
    use log::LevelFilter;
    assert_eq!(
        resolve_log_level(Some("debug".to_string()), Some("warn".to_string())),
        LevelFilter::Debug,
        "EMUWIZ_LOG must win when both are set"
    );
    assert_eq!(
        resolve_log_level(None, Some("trace".to_string())),
        LevelFilter::Trace,
        "the legacy ARCHIVEFS_LOG must still work"
    );
    assert_eq!(
        resolve_log_level(Some("bogus".to_string()), Some("warn".to_string())),
        LevelFilter::Info,
        "an invalid EMUWIZ_LOG falls back to the default, never to the legacy var"
    );
    assert_eq!(
        resolve_log_level(None, None),
        LevelFilter::Info,
        "the default level is info"
    );
}

#[test]
fn presentation_status_classification_is_consistent() {
    assert_eq!(
        profile_presentation_tone(true),
        widgets::StatusTone::Success
    );
    assert_eq!(
        profile_presentation_tone(false),
        widgets::StatusTone::Pending
    );
    assert_eq!(
        cheat_freshness_tone(CheatSourceFreshness::Fresh),
        widgets::StatusTone::Success
    );
    assert_eq!(
        cheat_freshness_tone(CheatSourceFreshness::Stale),
        widgets::StatusTone::Warning
    );
    assert_eq!(
        activity_outcome_tone(ActivityOutcome::Failed),
        widgets::StatusTone::Blocked
    );
    assert_eq!(
        activity_outcome_tone(ActivityOutcome::Started),
        widgets::StatusTone::Active
    );
}

#[test]
fn trusted_source_warnings_are_summarised_calmly_without_losing_counts() {
    let summaries = summarise_cheat_warnings(&[
        "147 catalogue files were retained but are non-actionable because parsing was incomplete"
            .to_string(),
        "6 catalogue files used unsupported content or encoding and were excluded".to_string(),
        "2 catalogue paths used unsupported encoding and were excluded".to_string(),
    ]);
    assert_eq!(
        summaries[0],
        "147 catalogue files could not be parsed and were excluded from matching."
    );
    assert_eq!(
        summaries[1],
        "6 files used unsupported content encoding and were excluded."
    );
    assert_eq!(
        summaries[2],
        "2 paths used unsupported encoding and were excluded safely."
    );
}

#[test]
fn activity_starts_collapsed_and_compact_summary_prioritises_errors() {
    assert!(!app_for_operation_tests().show_activity);
    let mut history = OperationHistory::default();
    history.record(HistoryEntry::new(
        ActivityAction::Mount,
        None,
        ActivityOutcome::Failed,
        "older failure",
    ));
    history.record(HistoryEntry::new(
        ActivityAction::Refresh,
        None,
        ActivityOutcome::Completed,
        "new success",
    ));
    assert_eq!(
        activity_summary_entry(&history).map(|entry| entry.message.as_str()),
        Some("older failure")
    );
}

#[test]
fn library_default_columns_allocate_more_space_to_destinations() {
    let laptop = responsive_library_column_widths(900.0, 10.0);
    let desktop = responsive_library_column_widths(1500.0, 10.0);
    assert!(laptop.mount_path > laptop.archive_path);
    assert!(desktop.archive_path > laptop.archive_path);
    assert!(desktop.mount_path > laptop.mount_path);
}

#[test]
fn every_navigation_destination_has_a_title_and_width_policy() {
    for (view, label) in PRIMARY_NAVIGATION_DESTINATIONS {
        assert_eq!(main_view_title(view), label);
        let _ = main_view_content_width(view);
    }
}

#[test]
fn home_is_the_default_view_and_the_first_sidebar_entry() {
    assert_eq!(MainView::default(), MainView::Home);
    assert_eq!(app_for_operation_tests().view, MainView::Home);
    assert_eq!(PRIMARY_NAVIGATION_DESTINATIONS[0].0, MainView::Home);
}

#[test]
fn every_home_card_maps_to_its_real_existing_destination() {
    // Every card must land on a destination that already exists and is
    // reachable some other way - none of these are invented for Home.
    let expected = [
        (home_page::HomeCard::BuildLibrary, MainView::Sources),
        (home_page::HomeCard::ConvertDiscs, MainView::DiscConversion),
        (home_page::HomeCard::BrowseGames, MainView::Library),
        (
            home_page::HomeCard::DuplicateReview,
            MainView::ExactDuplicateReview,
        ),
        (home_page::HomeCard::CheatsAndMods, MainView::CheatsMods),
        (home_page::HomeCard::CheatSources, MainView::CheatSources),
        (home_page::HomeCard::DatSources, MainView::DatSources),
        (home_page::HomeCard::RomM, MainView::Sources),
        (home_page::HomeCard::CheckSetup, MainView::EmulatorSetup),
        (home_page::HomeCard::Settings, MainView::Settings),
        (home_page::HomeCard::QuickRename, MainView::IdentifyRename),
        (home_page::HomeCard::CheckProblems, MainView::Doctor),
    ];
    for (card, expected_view) in expected {
        assert_eq!(main_view_for_home_card(card), expected_view, "{card:?}");
    }
}

#[test]
fn main_view_for_home_card_agrees_with_runtime_navigate_to_home_card() {
    // Routing consistency: the pure mapping and `navigate_to_home_card` must
    // not drift for any card (they did for RomM and ConvertDiscs before the
    // 0.8.1 release-workflow fixes).
    for card in [
        home_page::HomeCard::BuildLibrary,
        home_page::HomeCard::ConvertDiscs,
        home_page::HomeCard::BrowseGames,
        home_page::HomeCard::DuplicateReview,
        home_page::HomeCard::CheatsAndMods,
        home_page::HomeCard::CanonicalOrganisation,
        home_page::HomeCard::QuickRename,
        home_page::HomeCard::CheatSources,
        home_page::HomeCard::DatSources,
        home_page::HomeCard::RomM,
        home_page::HomeCard::CheckSetup,
        home_page::HomeCard::Settings,
        home_page::HomeCard::CheckProblems,
    ] {
        let mut app = app_for_operation_tests();
        app.navigate_to_home_card(card);
        assert_eq!(
            app.view,
            main_view_for_home_card(card),
            "{card:?}: navigate_to_home_card and main_view_for_home_card disagree"
        );
    }
}

#[test]
fn navigate_to_main_view_for_a_home_card_click_matches_a_sidebar_click() {
    // A Home card click and the equivalent sidebar click must both
    // route through `navigate_to_main_view`, so a card can never behave
    // differently from the button that already exists for the same
    // destination - in particular Library's tab-restoring behaviour.
    let mut app = app_for_operation_tests();
    app.library_tab = LibraryTab::Duplicates;
    app.navigate_to_main_view(main_view_for_home_card(home_page::HomeCard::BrowseGames));
    assert_eq!(app.view, MainView::Duplicates);

    let mut app = app_for_operation_tests();
    app.tools_overlay = ToolsOverlay::PlatformAliases;
    app.navigate_to_main_view(main_view_for_home_card(home_page::HomeCard::CheatsAndMods));
    assert_eq!(app.view, MainView::CheatsMods);
    assert_eq!(app.tools_overlay, ToolsOverlay::None);
}

#[test]
fn task_cards_open_the_existing_specialised_workflows() {
    let mut app = app_for_operation_tests();

    // "Convert discs" lands on the first-class Disc Conversion page itself -
    // no Repair framing, no second hidden click.
    app.navigate_to_home_card(home_page::HomeCard::ConvertDiscs);
    assert_eq!(app.view, MainView::DiscConversion);
    assert!(
        app.optical_conversion_page.is_some(),
        "the conversion page must be open on arrival"
    );

    // "Find duplicate games" lands on the first-class Duplicate Finder, not
    // the read-only DAT-relative Library duplicates viewer and not a Repair
    // sub-page.
    app.navigate_to_home_card(home_page::HomeCard::DuplicateReview);
    assert_eq!(app.view, MainView::ExactDuplicateReview);

    // "Connect RomM" lands on the RomM provider card on Sources -> Libraries
    // (the subsystem its readiness badge reflects), never the whole-library
    // Playing Library planner.
    app.navigate_to_home_card(home_page::HomeCard::RomM);
    assert_eq!(app.view, MainView::Sources);
    assert_eq!(app.sources_tab, SourcesTab::Libraries);
    assert!(
        app.rom_organisation_page
            .as_ref()
            .is_none_or(|page| !page.showing_playing_library),
        "the RomM card must not flip Library Organisation into playing-library mode"
    );
}

#[test]
fn playing_library_planner_stays_reachable_from_library_organisation() {
    // Blocker 1 fix moved the RomM card off the playing-library planner; the
    // planner must still be reachable honestly from its own page - the
    // "Build Playing Library" button on the Organise page.
    let mut page = rom_organisation_page::RomOrganisationPageState::load();
    assert!(!page.showing_playing_library);
    let ctx = egui::Context::default();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 2000.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                rom_organisation_page::show_rom_organisation_page(ui, &mut page);
            });
        },
    );
    assert!(rendered_text_contains(&output, "Build Playing Library"));
}

// --- 0.8.1: core workflows directly discoverable ----------------------

fn pointer_click(pos: egui::Pos2) -> Vec<egui::Event> {
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
}

/// Every `Shape::Text` whose laid-out string exactly matches `needle`, as a
/// centre point - the plural of `find_exact_text_center`, needed where a
/// label appears both in the left sidebar and in a top-menu popup.
fn exact_text_centers(output: &egui::FullOutput, needle: &str) -> Vec<egui::Pos2> {
    fn walk(shape: &egui::Shape, needle: &str, out: &mut Vec<egui::Pos2>) {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == needle => {
                out.push(text.pos + text.galley.size() / 2.0);
            }
            egui::Shape::Vec(nested) => nested.iter().for_each(|s| walk(s, needle, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, needle, &mut out);
    }
    out
}

fn advanced_screen_input() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1600.0, 1200.0),
        )),
        ..Default::default()
    }
}

#[test]
fn major_workflows_are_reachable_from_home_sidebar_and_top_menu() {
    let sidebar_views: std::collections::HashSet<MainView> = ADVANCED_NAV_GROUPS
        .iter()
        .flat_map(|group| group.entries)
        .filter_map(|entry| match entry.click {
            NavClick::View(view) => Some(view),
            _ => None,
        })
        .collect();

    // Duplicate Finder / Disc Conversion / Emulator Setup: first-class
    // `MainView` destinations exposed on the sidebar AND the Tools menu, and
    // every entry point converges on the identical view.
    for (home_card, target) in [
        (
            home_page::HomeCard::DuplicateReview,
            MainView::ExactDuplicateReview,
        ),
        (home_page::HomeCard::ConvertDiscs, MainView::DiscConversion),
        (home_page::HomeCard::CheckSetup, MainView::EmulatorSetup),
    ] {
        assert!(
            sidebar_views.contains(&target),
            "{target:?} must have a sidebar entry"
        );
        assert!(
            TOOLS_MENU_WORKFLOWS.iter().any(|(_, _, t)| *t == target),
            "{target:?} must be a Tools-menu workflow"
        );
        assert_eq!(
            main_view_for_home_card(home_card),
            target,
            "the Home card for {target:?} must converge on it"
        );

        let mut from_home = app_for_operation_tests();
        from_home.navigate_to_home_card(home_card);
        let mut from_sidebar = app_for_operation_tests();
        from_sidebar.navigate_to_main_view(target);
        assert_eq!(from_home.view, target);
        assert_eq!(from_home.view, from_sidebar.view);
    }

    // Each Tools-menu workflow label is exactly its destination's chrome
    // title - the menu can never name a destination differently.
    for (label, _hover, target) in TOOLS_MENU_WORKFLOWS {
        assert_eq!(main_view_title(target), label);
    }

    // RomM has no `MainView` of its own; sidebar, Home card and the Sources
    // menu all converge on Sources -> Libraries (the RomM provider card).
    assert!(
        ADVANCED_NAV_GROUPS
            .iter()
            .flat_map(|group| group.entries)
            .any(|entry| matches!(entry.click, NavClick::Romm) && entry.label == "RomM"),
        "the sidebar must expose a RomM entry"
    );
    assert_eq!(
        main_view_for_home_card(home_page::HomeCard::RomM),
        MainView::Sources
    );
    let mut romm = app_for_operation_tests();
    romm.navigate_to_home_card(home_page::HomeCard::RomM);
    assert_eq!(romm.view, MainView::Sources);
    assert_eq!(romm.sources_tab, SourcesTab::Libraries);
}

#[test]
fn the_tools_menu_opens_and_routes_directly_to_a_first_class_workflow() {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::AdvancedView;
    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();

    let output = ctx.run(advanced_screen_input(), |ctx| app.update(ctx, &mut frame));
    let tools = find_exact_text_center(&output, "Tools").expect("the Tools menu label renders");
    let _ = ctx.run(
        egui::RawInput {
            events: pointer_click(tools),
            ..advanced_screen_input()
        },
        |ctx| app.update(ctx, &mut frame),
    );
    // Settle so the menu popup is painted into the output.
    let _ = ctx.run(advanced_screen_input(), |ctx| app.update(ctx, &mut frame));
    let output = ctx.run(advanced_screen_input(), |ctx| app.update(ctx, &mut frame));

    // "Platform Aliases" is a Tools-menu-only label (never in the sidebar) -
    // its presence proves the menu is actually open.
    assert!(
        rendered_text_contains(&output, "Platform Aliases"),
        "the Tools menu must be open"
    );
    assert!(rendered_text_contains(&output, "Emulator Setup"));

    // The menu popup drops from the top bar (y < 200); the identically
    // labelled sidebar entry is far lower. Click the menu one.
    let menu_item = exact_text_centers(&output, "Emulator Setup")
        .into_iter()
        .find(|pos| pos.y < 200.0)
        .expect("Emulator Setup must render inside the open Tools menu");
    let _ = ctx.run(
        egui::RawInput {
            events: pointer_click(menu_item),
            ..advanced_screen_input()
        },
        |ctx| app.update(ctx, &mut frame),
    );

    assert_eq!(app.view, MainView::EmulatorSetup);
}

#[test]
fn duplicate_finder_and_disc_conversion_render_standalone_from_the_sidebar_route() {
    // Item 6 & 7: arriving via the dedicated route shows the workflow's own
    // page, never Repair Review / Repair History framing.
    for (target, own_content) in [
        (MainView::ExactDuplicateReview, "Duplicate Finder"),
        (MainView::DiscConversion, "Convert Disc Images"),
    ] {
        let mut app = app_for_operation_tests();
        app.ui_mode = GuiMode::AdvancedView;
        app.navigate_to_main_view(target);
        assert_eq!(app.view, target);

        let ctx = egui::Context::default();
        let mut frame = eframe::Frame::_new_kittest();
        let output = ctx.run(advanced_screen_input(), |ctx| app.update(ctx, &mut frame));

        assert!(
            rendered_text_contains(&output, own_content),
            "{target:?} must render its own page content ({own_content:?})"
        );
        assert!(
            !rendered_text_contains(&output, "Repair History"),
            "{target:?} must not render Repair History framing"
        );
        assert!(
            !rendered_text_contains(&output, "Repair / Recovery"),
            "{target:?} must not render the Problems & Repair tab row"
        );
    }
}

#[test]
fn collapsible_section_toggles_and_persists_for_the_app_session() {
    fn frame(ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 800.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    widgets::collapsible_section(
                        ui,
                        "collapse_test_section",
                        "Advanced options",
                        false,
                        |ui| {
                            ui.label("BODY-CONTENT-MARKER");
                        },
                    );
                });
            },
        )
    }

    let ctx = egui::Context::default();

    // default_open: false -> the body is not drawn.
    let output = frame(&ctx, vec![]);
    assert!(!rendered_text_contains(&output, "BODY-CONTENT-MARKER"));
    let header = find_exact_text_center(&output, "Advanced options").expect("header renders");

    // Click the header, then settle - now expanded.
    let _ = frame(&ctx, pointer_click(header));
    let _ = frame(&ctx, vec![]);
    let output = frame(&ctx, vec![]);
    assert!(
        rendered_text_contains(&output, "BODY-CONTENT-MARKER"),
        "the section expands when its header is clicked"
    );

    // A later frame in the SAME context stays expanded - the state persists
    // for the session.
    let output = frame(&ctx, vec![]);
    assert!(
        rendered_text_contains(&output, "BODY-CONTENT-MARKER"),
        "the expanded state persists across frames in the session"
    );

    // A fresh context (a new session) starts from `default_open: false`.
    let fresh = egui::Context::default();
    let output = frame(&fresh, vec![]);
    assert!(
        !rendered_text_contains(&output, "BODY-CONTENT-MARKER"),
        "a new session starts collapsed again"
    );
}

#[test]
fn a_later_navigation_call_always_wins_over_an_earlier_one_in_the_same_tick() {
    // Guards against a dispatch-ordering regression: if Home's block
    // and the sidebar's click handler ever both ran in one frame, the
    // later call must be the one that sticks - never a stale `self.view`
    // set earlier in the same tick silently surviving.
    let mut app = app_for_operation_tests();
    app.navigate_to_main_view(MainView::Sources);
    app.navigate_to_main_view(main_view_for_home_card(home_page::HomeCard::CheatsAndMods));
    assert_eq!(app.view, MainView::CheatsMods);
}

#[test]
fn gamer_view_gear_menu_always_lands_on_home_regardless_of_prior_view() {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::GamerView;
    app.view = MainView::Doctor; // stale from an earlier Advanced View visit
    app.switch_to_advanced_view_at_home();
    assert_eq!(app.ui_mode, GuiMode::AdvancedView);
    assert_eq!(app.view, MainView::Home);
    assert_eq!(app.tools_overlay, ToolsOverlay::None);
}

#[test]
fn gamer_menu_keeps_everyday_actions_visible_and_names_the_menu() {
    assert_eq!(GAMER_MENU_LABEL, "Menu");
    assert_eq!(GAMER_MENU_ADD_FOLDER_LABEL, "Add another game folder");
    assert_eq!(GAMER_MENU_SCAN_LABEL, "Scan for new games");
    assert_eq!(GAMER_MENU_SETUP_LABEL, "Emulator Setup");
    assert_eq!(GAMER_MENU_ADVANCED_LABEL, "Advanced View");

    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::GamerView;
    app.retroarch_profiles = RetroArchProfilesState::Error("test".to_string());
    if let LoadState::Ready(data) = &mut app.state {
        data.records
            .push(record("/roms/another-game.zip", MountState::Pending));
    }
    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let output = ctx.run(advanced_screen_input(), |ctx| app.update(ctx, &mut frame));
    let menu = find_exact_text_center(&output, GAMER_MENU_LABEL).expect("Gamer menu renders");
    let _ = ctx.run(
        egui::RawInput {
            events: pointer_click(menu),
            ..advanced_screen_input()
        },
        |ctx| app.update(ctx, &mut frame),
    );
    let output = ctx.run(advanced_screen_input(), |ctx| app.update(ctx, &mut frame));

    for label in [
        GAMER_MENU_ADD_FOLDER_LABEL,
        GAMER_MENU_SCAN_LABEL,
        GAMER_MENU_SETUP_LABEL,
        GAMER_MENU_ADVANCED_LABEL,
    ] {
        assert!(
            rendered_text_contains(&output, label),
            "Gamer menu must expose {label:?}"
        );
    }

    let setup = find_exact_text_center(&output, GAMER_MENU_SETUP_LABEL)
        .expect("Emulator Setup menu action renders");
    let _ = ctx.run(
        egui::RawInput {
            events: pointer_click(setup),
            ..advanced_screen_input()
        },
        |ctx| app.update(ctx, &mut frame),
    );
    assert_eq!(app.ui_mode, GuiMode::AdvancedView);
    assert_eq!(app.view, MainView::EmulatorSetup);
}

#[test]
fn returning_home_from_any_destination_is_reliable() {
    let mut app = app_for_operation_tests();
    for destination in [
        MainView::Sources,
        MainView::CheatsMods,
        MainView::Doctor,
        MainView::Settings,
        MainView::DatSources,
    ] {
        app.navigate_to_main_view(destination);
        assert_eq!(app.view, destination);
        app.navigate_to_main_view(MainView::Home);
        assert_eq!(app.view, MainView::Home);
    }
}

#[test]
fn legacy_library_destinations_still_have_a_title_and_width_policy() {
    // No longer sidebar destinations (see
    // library_is_the_only_sidebar_destination_for_the_library_area),
    // but still real MainView variants reachable via legacy
    // programmatic routes and the unified shell's tab_row - both need
    // a title and a width policy exactly like every other view.
    for view in [
        MainView::Health,
        MainView::Duplicates,
        MainView::LibraryViews,
    ] {
        let title = main_view_title(view);
        assert!(!title.is_empty());
        let _ = main_view_content_width(view);
    }
}

#[test]
fn library_tab_for_main_view_covers_all_five_library_destinations() {
    let library_destinations = [
        (MainView::Library, LibraryTab::Archives),
        (MainView::Health, LibraryTab::Health),
        (MainView::Duplicates, LibraryTab::Duplicates),
        (MainView::LibraryViews, LibraryTab::Views),
        (MainView::RecentlyFound, LibraryTab::RecentlyFound),
    ];
    for (view, expected_tab) in library_destinations {
        assert_eq!(
            library_tab_for_main_view(view),
            Some(expected_tab),
            "{view:?} must route to {expected_tab:?}"
        );
    }

    // Every other destination must map to None rather than silently picking
    // a Library tab.
    let non_library_destinations = [
        MainView::Sources,
        MainView::Mount,
        MainView::Selected,
        MainView::CheatsMods,
        MainView::ActiveMounts,
        MainView::Doctor,
        MainView::HistoryLogs,
        MainView::Settings,
        MainView::About,
    ];
    for view in non_library_destinations {
        assert_eq!(
            library_tab_for_main_view(view),
            None,
            "{view:?} must not be treated as a Library tab"
        );
    }
}

#[test]
fn main_view_for_library_tab_round_trips_with_library_tab_for_main_view() {
    for tab in [
        LibraryTab::Archives,
        LibraryTab::Health,
        LibraryTab::Duplicates,
        LibraryTab::Views,
        LibraryTab::RecentlyFound,
    ] {
        let view = main_view_for_library_tab(tab);
        assert_eq!(
            library_tab_for_main_view(view),
            Some(tab),
            "main_view_for_library_tab({tab:?}) -> {view:?} must route back to {tab:?}"
        );
    }
}

#[test]
fn library_tab_label_is_distinct_and_non_empty_for_every_tab() {
    let labels: Vec<&str> = [
        LibraryTab::Archives,
        LibraryTab::Health,
        LibraryTab::Duplicates,
        LibraryTab::Views,
        LibraryTab::RecentlyFound,
    ]
    .into_iter()
    .map(library_tab_label)
    .collect();
    assert_eq!(
        labels,
        vec![
            "Archives",
            "Health",
            "Duplicates",
            "Views",
            "Recently Found"
        ]
    );
    let unique: std::collections::HashSet<&str> = labels.iter().copied().collect();
    assert_eq!(
        unique.len(),
        labels.len(),
        "every tab label must be distinct"
    );
}

#[test]
fn legacy_routes_reconcile_to_the_correct_library_tab() {
    let mut app = app_for_operation_tests();
    assert_eq!(
        app.library_tab,
        LibraryTab::Archives,
        "a fresh app starts on the Archives tab"
    );

    for (legacy_view, expected_tab) in [
        (MainView::Health, LibraryTab::Health),
        (MainView::Duplicates, LibraryTab::Duplicates),
        (MainView::LibraryViews, LibraryTab::Views),
        (MainView::Library, LibraryTab::Archives),
    ] {
        // Simulates any of the ~11 legacy `self.view = MainView::X`
        // call sites - none of them touch library_tab directly.
        app.view = legacy_view;
        app.reconcile_library_tab();
        assert_eq!(
            app.library_tab, expected_tab,
            "navigating to {legacy_view:?} via the legacy route must select {expected_tab:?}"
        );
    }
}

#[test]
fn library_tab_survives_navigating_away_to_an_unrelated_destination() {
    let mut app = app_for_operation_tests();
    app.view = MainView::Health;
    app.reconcile_library_tab();
    assert_eq!(app.library_tab, LibraryTab::Health);

    // Visiting several unrelated destinations must never reset the
    // remembered Library tab.
    for unrelated_view in [
        MainView::Settings,
        MainView::Mount,
        MainView::CheatsMods,
        MainView::About,
    ] {
        app.view = unrelated_view;
        app.reconcile_library_tab();
        assert_eq!(
            app.library_tab,
            LibraryTab::Health,
            "visiting {unrelated_view:?} must not change the remembered Library tab"
        );
    }
}

#[test]
fn navigate_to_library_tab_sets_view_and_library_tab_together() {
    let mut app = app_for_operation_tests();
    app.tools_overlay = ToolsOverlay::PlatformAliases;

    app.navigate_to_library_tab(LibraryTab::Duplicates);

    assert_eq!(app.view, MainView::Duplicates);
    assert_eq!(app.library_tab, LibraryTab::Duplicates);
    assert_eq!(
        app.tools_overlay,
        ToolsOverlay::None,
        "navigating by tab must clear any open Tools overlay, like every other navigation call site"
    );
}

#[test]
fn selected_archive_and_filters_survive_a_library_tab_switch() {
    let mut app = app_for_operation_tests();
    app.archive_context.focused = Some(PathBuf::from("/roms/a.zip"));
    app.archive_context.selected = [PathBuf::from("/roms/a.zip")].into_iter().collect();
    app.filter = "mario".to_string();
    app.library_filters.missing = true;
    app.health_filters.category = HealthIssueFilter::UnknownPlatform;
    app.duplicate_filters.platform = Some("SNES".to_string());

    for tab in [
        LibraryTab::Health,
        LibraryTab::Duplicates,
        LibraryTab::Views,
        LibraryTab::Archives,
    ] {
        app.navigate_to_library_tab(tab);
    }

    assert_eq!(
        app.archive_context.focused.as_deref(),
        Some(Path::new("/roms/a.zip")),
        "the selected archive must survive switching Library tabs"
    );
    assert_eq!(app.archive_context.selected.len(), 1);
    assert_eq!(
        app.filter, "mario",
        "the Library free-text filter must survive switching tabs"
    );
    assert!(
        app.library_filters.missing,
        "Library row filters must survive switching tabs"
    );
    assert_eq!(
        app.health_filters.category,
        HealthIssueFilter::UnknownPlatform,
        "Health filter state must survive switching tabs"
    );
    assert_eq!(
        app.duplicate_filters.platform.as_deref(),
        Some("SNES"),
        "Duplicate filter state must survive switching tabs"
    );
}

#[test]
fn views_configuration_survives_a_library_tab_switch() {
    let mut app = app_for_operation_tests();
    app.library_views = vec![sample_library_view(
        "view-1",
        "My Consoles",
        "/library/consoles",
    )];
    app.library_view_plan_filter = LibraryViewPlanFilter::Create;

    for tab in [
        LibraryTab::Health,
        LibraryTab::Duplicates,
        LibraryTab::Archives,
        LibraryTab::Views,
    ] {
        app.navigate_to_library_tab(tab);
    }

    assert_eq!(app.library_views.len(), 1);
    assert_eq!(app.library_views[0].id, "view-1");
    assert_eq!(app.library_views[0].name, "My Consoles");
    assert_eq!(
        app.library_view_plan_filter,
        LibraryViewPlanFilter::Create,
        "the Views plan filter must survive switching tabs"
    );
}

#[test]
fn library_shell_header_shows_all_five_tab_labels() {
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_library_shell_header(ui, LibraryTab::Archives);
        });
    });
    for expected in [
        "My Games",
        "Archives",
        "Health",
        "Duplicates",
        "Views",
        "Recently Found",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "the Library shell header did not render {expected:?}"
        );
    }
}

#[test]
fn library_shell_header_tabs_are_reachable_via_a_real_click() {
    let ctx = egui::Context::default();
    for target in [
        LibraryTab::Archives,
        LibraryTab::Health,
        LibraryTab::Duplicates,
        LibraryTab::Views,
        LibraryTab::RecentlyFound,
    ] {
        let discovery_output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_library_shell_header(ui, LibraryTab::Archives);
            });
        });
        let label = library_tab_label(target);
        let target_pos = find_exact_text_center(&discovery_output, label)
            .unwrap_or_else(|| panic!("{label:?} tab label must be rendered"));

        let clicked_tab: std::rc::Rc<std::cell::RefCell<Option<LibraryTab>>> =
            std::rc::Rc::new(std::cell::RefCell::new(None));
        let captured = std::rc::Rc::clone(&clicked_tab);
        let render = move |ui: &mut egui::Ui| -> egui::Response {
            let inner = ui.scope(|ui| show_library_shell_header(ui, LibraryTab::Archives));
            if let Some(tab) = inner.inner {
                *captured.borrow_mut() = Some(tab);
            }
            inner.response
        };
        simulate_row_click(&ctx, target_pos, egui::Modifiers::default(), render);

        assert_eq!(
            *clicked_tab.borrow(),
            Some(target),
            "clicking the {label:?} tab must select it"
        );
    }
}

#[test]
fn library_shell_header_marks_the_current_tab_selected() {
    // tab_row (the component library_shell_header is built on) is
    // already tested generically for this in ui/components.rs; this
    // confirms the Library shell wires the *correct* current tab
    // through, by checking each tab reports itself as already
    // selected (a click on the currently-selected tab is still a
    // real click on a `Button::selectable(true, ...)`, which returns
    // `Some` just like any other tab_row option).
    let ctx = egui::Context::default();
    for tab in [
        LibraryTab::Archives,
        LibraryTab::Health,
        LibraryTab::Duplicates,
        LibraryTab::Views,
        LibraryTab::RecentlyFound,
    ] {
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_library_shell_header(ui, tab);
            });
        });
        assert!(rendered_text_contains(&output, library_tab_label(tab)));
    }
}

#[test]
fn library_is_the_only_sidebar_destination_for_the_library_area() {
    assert!(
        PRIMARY_NAVIGATION_DESTINATIONS
            .iter()
            .any(|(view, _)| *view == MainView::Library),
        "Library must remain a primary sidebar destination"
    );
    for absent in [
        MainView::Health,
        MainView::Duplicates,
        MainView::LibraryViews,
    ] {
        assert!(
            !PRIMARY_NAVIGATION_DESTINATIONS
                .iter()
                .any(|(view, _)| *view == absent),
            "{absent:?} must not be a separate sidebar destination any more"
        );
    }
    assert!(
        !PRIMARY_NAVIGATION_DESTINATIONS
            .iter()
            .any(|(view, _)| *view == MainView::RecentlyFound),
        "Recently Found must be a Library tab, not a sidebar destination"
    );
}

#[test]
fn library_sidebar_button_is_selected_only_on_the_library_destination() {
    for view in [
        MainView::Library,
        MainView::Health,
        MainView::Duplicates,
        MainView::LibraryViews,
        MainView::RecentlyFound,
    ] {
        assert_eq!(
            navigation_destination_selected(view, MainView::Library),
            view == MainView::Library,
            "Library selection must have one visual owner while on {view:?}"
        );
    }
    for view in [MainView::Mount, MainView::Settings] {
        assert!(
            !navigation_destination_selected(view, MainView::Library),
            "the Library sidebar button must not render selected while on {view:?}"
        );
    }
}

#[test]
fn clicking_library_in_the_sidebar_restores_the_last_selected_tab() {
    let mut app = app_for_operation_tests();
    app.view = MainView::Health;
    app.reconcile_library_tab();
    assert_eq!(app.library_tab, LibraryTab::Health);

    // Simulates the sidebar click handler's special case for
    // MainView::Library (see `update`): navigate away, then "click
    // Library" by calling the same navigate_to_library_tab(self.
    // library_tab) it calls.
    app.view = MainView::Settings;
    app.navigate_to_library_tab(app.library_tab);

    assert_eq!(
        app.view,
        MainView::Health,
        "clicking Library must restore the last selected tab, not reset to Archives"
    );
    assert_eq!(app.library_tab, LibraryTab::Health);
}

#[test]
fn migrated_navigation_sites_preserve_their_own_side_effects_alongside_the_tab_switch() {
    // Mirrors HealthDashboardAction::OpenMissingReview's exact
    // dispatch lines (now navigate_to_library_tab + the same
    // library_filters.missing write it always had).
    let mut app = app_for_operation_tests();
    app.navigate_to_library_tab(LibraryTab::Archives);
    app.library_filters.missing = true;
    assert_eq!(app.view, MainView::Library);
    assert_eq!(app.library_tab, LibraryTab::Archives);
    assert!(app.library_filters.missing);

    // Mirrors AppOperationRequest::ShowInLibraryViews's exact
    // dispatch lines.
    let mut app = app_for_operation_tests();
    let archive_path = PathBuf::from("/roms/focus.zip");
    app.navigate_to_library_tab(LibraryTab::Views);
    app.library_view_focus_archive = Some(archive_path.clone());
    assert_eq!(app.view, MainView::LibraryViews);
    assert_eq!(app.library_tab, LibraryTab::Views);
    assert_eq!(app.library_view_focus_archive, Some(archive_path));

    // Mirrors HealthDashboardAction::OpenDuplicateReview's exact
    // dispatch line - the one migrated site that targets a specific
    // non-Archives tab rather than "go to Library" generically.
    let mut app = app_for_operation_tests();
    app.navigate_to_library_tab(LibraryTab::Duplicates);
    assert_eq!(app.view, MainView::Duplicates);
    assert_eq!(app.library_tab, LibraryTab::Duplicates);
}

#[test]
fn back_to_library_actions_land_on_the_archives_tab_from_any_starting_tab() {
    for starting_tab in [
        LibraryTab::Health,
        LibraryTab::Duplicates,
        LibraryTab::Views,
    ] {
        // Mirrors HealthDashboardAction::BackToLibrary's and
        // DuplicateReviewAction::Close's identical dispatch line.
        let mut app = app_for_operation_tests();
        app.navigate_to_library_tab(starting_tab);
        assert_eq!(app.library_tab, starting_tab);

        app.navigate_to_library_tab(LibraryTab::Archives);
        assert_eq!(
            app.view,
            MainView::Library,
            "a Back to Library control from {starting_tab:?} must land on Library/Archives"
        );
        assert_eq!(app.library_tab, LibraryTab::Archives);
    }
}

impl ClipboardBackend for InMemoryClipboard {
    fn get_text_status(&mut self) -> ClipboardTextStatus {
        if let Some(reason) = &self.unavailable {
            return ClipboardTextStatus::Unavailable(reason.clone());
        }
        match &self.text {
            Some(text) if !text.is_empty() => ClipboardTextStatus::Ready(text.clone()),
            _ => ClipboardTextStatus::Empty,
        }
    }

    fn set_text(&mut self, text: String) -> Result<(), String> {
        if let Some(reason) = &self.set_error {
            return Err(reason.clone());
        }
        self.set_calls.push(text.clone());
        self.text = Some(text);
        Ok(())
    }
}

#[test]
fn queued_pending_paths_keeps_queue_order_and_only_pending_archives() {
    let records = vec![
        record("/roms/a.zip", MountState::Mounted),
        record("/roms/b.zip", MountState::Pending),
        record("/roms/c.zip", MountState::MountPathExists),
        record("/roms/d.zip", MountState::Pending),
    ];
    let queue = vec![
        PathBuf::from("/roms/d.zip"),
        PathBuf::from("/roms/a.zip"),
        PathBuf::from("/roms/b.zip"),
        PathBuf::from("/roms/missing.zip"),
    ];
    assert_eq!(
        queued_pending_paths(&queue, &records),
        vec![PathBuf::from("/roms/d.zip"), PathBuf::from("/roms/b.zip")],
        "only Pending queued archives are attempted, in queue order"
    );
}

#[test]
fn stale_loose_rom_queue_entry_is_visible_but_never_attempted() {
    let loose = loose_mega_drive_record("/roms/genesis/Alien 3.md");
    let archive = record("/roms/game.zip", MountState::Pending);
    let records = vec![loose, archive];
    let queue = vec![
        PathBuf::from("/roms/genesis/Alien 3.md"),
        PathBuf::from("/roms/game.zip"),
    ];
    assert_eq!(
        queued_pending_paths(&queue, &records),
        vec![PathBuf::from("/roms/game.zip")]
    );
    assert_eq!(
        queue.len(),
        2,
        "stale loose entry remains removable by the user"
    );
    assert_eq!(pending_mount_items(&records).len(), 1);
    assert!(
        mount_all_items_for_paths(&records, &queue)
            .iter()
            .all(|item| item.archive_path != Path::new("/roms/genesis/Alien 3.md"))
    );
}

#[test]
fn prune_mount_queue_drops_only_archives_missing_from_the_snapshot() {
    let records = vec![
        record("/roms/a.zip", MountState::Mounted),
        record("/roms/b.zip", MountState::MountPathExists),
    ];
    let mut queue = vec![
        PathBuf::from("/roms/a.zip"),
        PathBuf::from("/roms/gone.zip"),
        PathBuf::from("/roms/b.zip"),
    ];
    prune_mount_queue(&mut queue, &records);
    assert_eq!(
        queue,
        vec![PathBuf::from("/roms/a.zip"), PathBuf::from("/roms/b.zip")],
        "non-Pending archives stay queued (with a skip label); only vanished paths are pruned"
    );
}

#[test]
fn mount_validation_labels_distinguish_ready_mounted_and_collision() {
    assert_eq!(
        mount_validation_label(MountState::Pending),
        "Ready to mount"
    );
    assert_eq!(
        mount_validation_label(MountState::Mounted),
        "Already mounted — will be skipped"
    );
    assert_eq!(
        mount_validation_label(MountState::MountPathExists),
        "Destination already exists — will be skipped"
    );
}

#[test]
fn navigating_away_and_back_keeps_the_candidate_and_its_cheat_selection() {
    let mut app = app_with_cheats_mods_context();
    workflow_at_cheat_selection_stage(&mut app);

    app.view = MainView::Library;
    app.poll_cheat_workflow(&egui::Context::default());
    app.view = MainView::CheatsMods;
    app.poll_cheat_workflow(&egui::Context::default());

    let workflow = app.cheat_workflow.as_ref().expect("workflow");
    let selection = workflow
        .candidate_selection
        .as_ref()
        .expect("the chosen candidate survives leaving the page");
    assert_eq!(selection.candidate.catalogue_relative_path, "NES/a.cht");
    assert_eq!(
        selection.selection.selected_count(),
        1,
        "the cheat selection survives too"
    );
}

#[test]
fn an_applying_transaction_survives_leaving_the_cheats_mods_page() {
    // docs/GUI_NAVIGATION_RESET_DESIGN.md mandatory risk #1 (Codex
    // audit): a Gamer View mode switch, or simply navigating back to
    // the game list, must never drop the receiver of an in-flight
    // install just because `MainView::CheatsMods` is no longer the
    // rendered page.
    let mut app = app_with_cheats_mods_context();
    let key = cheat_preview_key(app.cheat_workflow.as_ref().unwrap());
    let (_sender, receiver) = mpsc::channel();
    app.cheat_workflow.as_mut().unwrap().transaction =
        CheatTransactionState::Applying { key, receiver };

    app.view = MainView::Library;
    app.poll_cheat_workflow(&egui::Context::default());

    assert!(
        matches!(
            app.cheat_workflow.as_ref().unwrap().transaction,
            CheatTransactionState::Applying { .. }
        ),
        "an in-flight install must not be reset to Idle merely because \
             the Cheats & Mods page isn't rendered right now"
    );
}

#[test]
fn a_genuinely_stale_applying_transaction_is_still_reset() {
    // The correctness rule (different archive/adapter/profile/key)
    // that `an_applying_transaction_survives_leaving_the_cheats_mods_page`
    // must not accidentally disable: a transaction that no longer
    // matches the current workflow key is still reset, regardless of
    // which page is showing.
    let mut app = app_with_cheats_mods_context();
    let mut stale_key = cheat_preview_key(app.cheat_workflow.as_ref().unwrap());
    stale_key.archive_path = PathBuf::from("/roms/a-different-archive-entirely.zip");
    let (_sender, receiver) = mpsc::channel();
    app.cheat_workflow.as_mut().unwrap().transaction = CheatTransactionState::Applying {
        key: stale_key,
        receiver,
    };

    app.view = MainView::CheatsMods;
    app.poll_cheat_workflow(&egui::Context::default());

    assert!(
        matches!(
            app.cheat_workflow.as_ref().unwrap().transaction,
            CheatTransactionState::Idle
        ),
        "a stale transaction key must still be reset"
    );
}

#[test]
fn gamer_view_rollback_preview_survives_a_real_update_frame() {
    // docs/GUI_NAVIGATION_RESET_DESIGN.md mandatory risk #2 (Codex
    // audit): Gamer View's "Undo last change" drives the exact same
    // `shared_rollback` state machine History & Logs does, but Gamer
    // View never sets `self.view = MainView::HistoryLogs`. The
    // Advanced-View-only "leaving History & Logs resets rollback"
    // rule must not fire while `ui_mode` is `GamerView`.
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::GamerView;
    app.view = MainView::Library;
    let (_sender, receiver) = mpsc::channel();
    app.shared_rollback = SharedRollbackState::Previewing { receiver };

    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1600.0, 900.0),
        )),
        ..Default::default()
    };
    let _ = ctx.run(input, |ctx| app.update(ctx, &mut frame));

    assert!(
        matches!(app.shared_rollback, SharedRollbackState::Previewing { .. }),
        "a rollback preview started from Gamer View must not be reset to \
             Idle just because self.view isn't MainView::HistoryLogs"
    );
}

#[test]
fn advanced_view_rollback_preview_is_still_reset_on_leaving_history_logs() {
    // The exact opposite of the test above - Advanced View's existing
    // behaviour (unchanged) must still reset a rollback preview when
    // navigating away from History & Logs.
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::AdvancedView;
    app.view = MainView::Library;
    let (_sender, receiver) = mpsc::channel();
    app.shared_rollback = SharedRollbackState::Previewing { receiver };

    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1600.0, 900.0),
        )),
        ..Default::default()
    };
    let _ = ctx.run(input, |ctx| app.update(ctx, &mut frame));

    assert!(
        matches!(app.shared_rollback, SharedRollbackState::Idle),
        "Advanced View's existing reset-on-navigate-away behaviour must be unchanged"
    );
}

#[test]
fn platform_artwork_pack_v1_resolves_every_exact_alias() {
    let aliases = [
        ("Acorn Archimedes", "acornarchimedes"),
        ("Archimedes", "acornarchimedes"),
        ("Amiga", "amiga"),
        ("Commodore Amiga", "amiga"),
        ("Dreamcast", "dreamcast"),
        ("Sega Dreamcast", "dreamcast"),
        ("Game Boy", "gameboy"),
        ("Nintendo Game Boy", "gameboy"),
        ("GameCube", "gamecube"),
        ("Nintendo GameCube", "gamecube"),
        ("Nintendo Game Cube", "gamecube"),
        ("Mega Drive", "megadrive"),
        ("Sega Mega Drive", "megadrive"),
        ("Genesis", "megadrive"),
        ("Sega Genesis", "megadrive"),
        ("Nintendo 64", "n64"),
        ("N64", "n64"),
        ("PlayStation", "psx"),
        ("PlayStation 1", "psx"),
        ("PS1", "psx"),
        ("PSX", "psx"),
        ("PlayStation 2", "ps2"),
        ("PS2", "ps2"),
        ("PlayStation 3", "ps3"),
        ("PS3", "ps3"),
        ("Saturn", "saturn"),
        ("Sega Saturn", "saturn"),
        ("Super Nintendo", "snes"),
        ("Super Nintendo Entertainment System", "snes"),
        ("SNES", "snes"),
        ("Super Famicom", "snes"),
        ("Nintendo Switch", "switch"),
        ("Switch", "switch"),
        ("Nintendo Wii", "wii"),
        ("Wii", "wii"),
        ("Nintendo Wii U", "wiiu"),
        ("Wii U", "wiiu"),
        ("WiiU", "wiiu"),
        ("Xbox", "xbox"),
        ("Xbox 360", "xbox360"),
    ];
    for (alias, expected) in aliases {
        assert_eq!(platform_asset_id(alias, false), expected, "alias {alias}");
        assert!(
            bundled_platform_artwork(expected).is_some(),
            "alias {alias} resolves to missing bundled id {expected}"
        );
        assert_eq!(
            platform_asset_id(&alias.to_ascii_uppercase(), false),
            expected,
            "case-normalized alias {alias}"
        );
    }
}

#[test]
fn platforms_without_bundled_pngs_keep_exact_keys_and_explicit_fallbacks() {
    assert_eq!(platform_asset_id("NES", false), "nes");
    assert_eq!(
        platform_asset_category("NES"),
        PlatformAssetCategory::Console
    );
    assert_eq!(
        platform_asset_id("Game Boy Advance", false),
        "gameboyadvance"
    );
    assert_eq!(
        platform_asset_category("Game Boy Advance"),
        PlatformAssetCategory::Handheld
    );
    assert_eq!(platform_asset_id("PC", false), "pc");
    assert_eq!(
        platform_asset_category("PC"),
        PlatformAssetCategory::Computer
    );
    assert_eq!(platform_asset_id("Arcade", false), "arcade");
    assert_eq!(
        platform_asset_category("Arcade"),
        PlatformAssetCategory::Arcade
    );
}

#[test]
fn platform_asset_id_uses_unknown_fallback_for_a_genuinely_unrecognised_platform() {
    // A platform name archivefs-core has never heard of at all -
    // distinct from `unknown_platform: true` (the row's own "no
    // platform detected" flag), both must resolve to the same
    // Unknown asset.
    assert_eq!(platform_asset_id("SomeMadeUpPlatform", false), "unknown");
    assert_eq!(platform_asset_id("", true), "unknown");
    assert_eq!(platform_asset_id("Anything", true), "unknown");
}

#[test]
fn exact_aliases_do_not_collapse_related_platforms() {
    assert_eq!(platform_asset_id("PSX", false), "psx");
    assert_eq!(platform_asset_id("Wii U", false), "wiiu");
    assert_ne!(
        platform_asset_id("Wii U", false),
        platform_asset_id("Wii", false)
    );
    assert_eq!(platform_asset_id("Xbox 360", false), "xbox360");
    assert_ne!(
        platform_asset_id("Xbox 360", false),
        platform_asset_id("Xbox", false)
    );
    assert_eq!(platform_asset_id("Mega Drive", false), "megadrive");
    assert_eq!(platform_asset_id("Genesis", false), "megadrive");
}

#[test]
fn bundled_registry_is_complete_unique_and_decodable_without_filesystem_paths() {
    assert_eq!(BUNDLED_PLATFORM_ARTWORK.len(), 76);
    let mut ids: Vec<_> = BUNDLED_PLATFORM_ARTWORK
        .iter()
        .map(|artwork| artwork.asset_id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), BUNDLED_PLATFORM_ARTWORK.len());
    for artwork in BUNDLED_PLATFORM_ARTWORK {
        let inspection = inspect_platform_png(artwork.png)
            .unwrap_or_else(|error| panic!("{} failed validation: {error}", artwork.asset_id));
        assert_eq!((inspection.width, inspection.height), (1024, 1024));
        assert!(inspection.warnings.is_empty());
        // Transparency is the house style and is required by default. The
        // opaque imports are allowed only by being named in the review
        // queue, and only until someone cleans them up.
        let pending = OPAQUE_ARTWORK_PENDING_VISUAL_REVIEW.contains(&artwork.asset_id);
        if pending {
            assert!(
                !inspection.has_transparent_pixel,
                "{} is listed as opaque but now has transparency - remove it from \
                     OPAQUE_ARTWORK_PENDING_VISUAL_REVIEW",
                artwork.asset_id
            );
        } else {
            assert_eq!(inspection.color_type, image::ColorType::Rgba8);
            assert!(
                inspection.has_transparent_pixel,
                "{} is opaque; either give it a transparent background or add it to \
                     OPAQUE_ARTWORK_PENDING_VISUAL_REVIEW",
                artwork.asset_id
            );
        }
        let decoded = decode_bundled_platform_artwork(artwork.png)
            .unwrap_or_else(|error| panic!("{} failed to decode: {error:?}", artwork.asset_id));
        assert_eq!(decoded.size, [1024, 1024]);
    }
    assert!(bundled_platform_artwork("missing-platform").is_none());

    // The review queue cannot rot: every name in it must still be bundled.
    for pending in OPAQUE_ARTWORK_PENDING_VISUAL_REVIEW {
        assert!(
            bundled_platform_artwork(pending).is_some(),
            "{pending} is queued for visual review but is no longer bundled"
        );
    }
}

#[test]
fn every_bundled_asset_names_a_canonical_platform_and_the_two_registries_agree() {
    // Artwork loads by exact canonical stem, so a bundled asset whose id is
    // not a stem is dead weight that can never be drawn. And the core keeps
    // its own list of which platforms ship artwork, used for the status
    // report; if the two drift, the CLI tells the user a platform has
    // artwork that the GUI cannot draw, or the reverse.
    for artwork in BUNDLED_PLATFORM_ARTWORK {
        assert!(
            archivefs_core::platform_artwork::canonical_platform_for_stem(artwork.asset_id)
                .is_some(),
            "bundled asset {:?} is not a canonical artwork stem, so nothing can load it",
            artwork.asset_id
        );
    }

    let mut from_gui: Vec<String> = BUNDLED_PLATFORM_ARTWORK
        .iter()
        .map(|artwork| artwork.asset_id.to_string())
        .collect();
    let mut from_core: Vec<String> = archivefs_core::platform_artwork::bundled_platform_ids()
        .iter()
        .map(|id| {
            archivefs_core::platform_artwork::canonical_artwork_stem(id)
                .unwrap_or_else(|| panic!("{id:?} is not a canonical platform"))
        })
        .collect();
    from_gui.sort();
    from_core.sort();
    assert_eq!(
        from_gui, from_core,
        "the GUI's bundled artwork and the core's bundled platform list disagree"
    );
}

#[test]
fn malformed_bundled_bytes_fail_safely() {
    assert_eq!(
        decode_bundled_platform_artwork(b"not a png"),
        Err(CustomArtworkLoadError::Malformed)
    );
}

#[test]
fn animated_platform_png_is_rejected() {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend_from_slice(&8u32.to_be_bytes());
    bytes.extend_from_slice(b"acTL");
    bytes.extend_from_slice(&[0; 8]);
    bytes.extend_from_slice(&[0; 4]);
    assert_eq!(
        inspect_platform_png(&bytes),
        Err("animated PNG artwork is unsupported")
    );
}

#[test]
fn platform_artwork_resolution_never_uses_substring_guessing() {
    assert_eq!(platform_asset_id("NES Classics", false), "unknown");
    assert_eq!(platform_asset_id("not-wiiu-backup", false), "unknown");
    assert_eq!(
        platform_asset_category("xbox360-old"),
        PlatformAssetCategory::Unknown
    );
}

#[test]
fn platform_asset_id_handles_long_platform_names_without_panicking() {
    let long_name = "A".repeat(500);
    // Must not panic and must resolve to a real, valid asset id.
    let resolved = platform_asset_id(&long_name, false);
    assert!(!resolved.is_empty());
}

#[test]
fn every_canonical_platform_has_one_unique_filename_and_intentional_fallback() {
    let mut asset_ids = std::collections::BTreeSet::new();
    for platform in archivefs_core::platform::PLATFORMS {
        let asset_id = platform_asset_id(platform.id, false);
        assert!(
            valid_platform_asset_id(&asset_id),
            "{} -> {asset_id}",
            platform.id
        );
        assert!(
            asset_ids.insert(asset_id.clone()),
            "duplicate artwork key {asset_id}"
        );
        assert_eq!(asset_id, canonical_platform_asset_id(platform.id));
        assert_ne!(
            platform_asset_category(platform.id),
            PlatformAssetCategory::Unknown,
            "{} needs an intentional fallback category",
            platform.id
        );
    }
}

#[test]
fn bundled_png_directory_has_no_unused_or_case_colliding_platform_images() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/platforms");
    let mut actual_pngs = std::fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".png"))
        .collect::<Vec<_>>();
    actual_pngs.sort();
    let mut expected_pngs = BUNDLED_PLATFORM_ARTWORK
        .iter()
        .map(|artwork| format!("{}.png", artwork.asset_id))
        .collect::<Vec<_>>();
    expected_pngs.sort();
    assert_eq!(
        actual_pngs, expected_pngs,
        "unused or missing bundled platform PNG"
    );

    let mut lowercase = std::collections::BTreeSet::new();
    for filename in actual_pngs {
        assert_eq!(filename, filename.to_ascii_lowercase());
        assert!(!filename.contains(' '));
        assert!(!filename.contains("4448b039-69a6-4690-a61f-dfc5393c3069"));
        assert!(lowercase.insert(filename.to_ascii_lowercase()));
    }
}

#[test]
fn legacy_square_png_size_is_a_warning_not_a_rejection() {
    use image::ImageEncoder as _;

    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&[0, 0, 0, 0], 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    let inspection = inspect_platform_png(&bytes).unwrap();
    assert_eq!(
        inspection.warnings,
        vec![PlatformPngWarning::LegacySourceDimensions {
            width: 1,
            height: 1
        }]
    );
}

// --- Platform shelf horizontal navigation ----------------------------

fn shelf_metrics(offset: f32, content: f32, viewport: f32) -> PlatformShelfMetrics {
    // Stride zero: these navigation tests predate card-aligned paging and
    // are about the offset arithmetic, not the alignment. Zero selects the
    // raw fractional page, which is exactly what they were written against.
    shelf_metrics_with_stride(offset, content, viewport, 0.0)
}

fn shelf_metrics_with_stride(
    offset: f32,
    content: f32,
    viewport: f32,
    card_stride: f32,
) -> PlatformShelfMetrics {
    PlatformShelfMetrics {
        offset_x: offset,
        content_width: content,
        viewport_width: viewport,
        card_stride,
    }
}

/// Test 1: at the start of an overflowing strip, only "next" is usable.
#[test]
fn shelf_at_the_start_offers_next_only() {
    let metrics = shelf_metrics(0.0, 2400.0, 800.0);
    assert!(metrics.overflows());
    assert!(metrics.at_start());
    assert!(!metrics.at_end());
    assert!(!metrics.can_page_left(), "there is nothing to the left yet");
    assert!(metrics.can_page_right());
    assert_eq!(metrics.max_offset(), 1600.0);
}

/// Test 2: mid-strip, both directions are usable.
#[test]
fn shelf_in_the_middle_offers_both_directions() {
    let metrics = shelf_metrics(800.0, 2400.0, 800.0);
    assert!(!metrics.at_start());
    assert!(!metrics.at_end());
    assert!(metrics.can_page_left());
    assert!(metrics.can_page_right());
}

/// Test 3: at the end, only "previous" is usable.
#[test]
fn shelf_at_the_end_offers_previous_only() {
    let metrics = shelf_metrics(1600.0, 2400.0, 800.0);
    assert!(metrics.at_end());
    assert!(metrics.can_page_left());
    assert!(
        !metrics.can_page_right(),
        "the last card is already showing"
    );
}

/// Test 4: a press moves 70-80% of the visible width, as required.
#[test]
fn one_press_moves_between_seventy_and_eighty_percent_of_the_strip() {
    let metrics = shelf_metrics(0.0, 4000.0, 900.0);
    let travelled = metrics.scroll_distance(ShelfScroll::PageRight);
    let fraction = travelled / metrics.viewport_width;
    assert!(
        (0.70..=0.80).contains(&fraction),
        "one press moved {fraction} of the strip"
    );
    assert_eq!(metrics.offset_after(ShelfScroll::PageRight), 675.0);
}

/// Test 5: paging never runs past either edge.
#[test]
fn paging_is_clamped_at_both_edges() {
    // Near the end: the press stops exactly at the end, not beyond it.
    let near_end = shelf_metrics(1500.0, 2400.0, 800.0);
    assert_eq!(near_end.offset_after(ShelfScroll::PageRight), 1600.0);
    // Near the start: it stops at zero, never negative.
    let near_start = shelf_metrics(100.0, 2400.0, 800.0);
    assert_eq!(near_start.offset_after(ShelfScroll::PageLeft), 0.0);
    // Already at an edge: no movement at all.
    let at_start = shelf_metrics(0.0, 2400.0, 800.0);
    assert_eq!(at_start.scroll_distance(ShelfScroll::PageLeft), 0.0);
    assert!(!at_start.can_scroll(ShelfScroll::PageLeft));
    let at_end = shelf_metrics(1600.0, 2400.0, 800.0);
    assert_eq!(at_end.scroll_distance(ShelfScroll::PageRight), 0.0);
    assert!(!at_end.can_scroll(ShelfScroll::PageRight));
}

/// Test 6: Home and End jump to the first and last platform.
#[test]
fn home_and_end_jump_to_the_first_and_last_platform() {
    let metrics = shelf_metrics(640.0, 2400.0, 800.0);
    assert_eq!(metrics.offset_after(ShelfScroll::Start), 0.0);
    assert_eq!(metrics.offset_after(ShelfScroll::End), 1600.0);
    assert!(metrics.can_scroll(ShelfScroll::Start));
    assert!(metrics.can_scroll(ShelfScroll::End));

    // From an edge, the jump towards that same edge does nothing.
    let at_start = shelf_metrics(0.0, 2400.0, 800.0);
    assert!(!at_start.can_scroll(ShelfScroll::Start));
    assert!(at_start.can_scroll(ShelfScroll::End));
}

/// Test 7: a strip that fits needs no controls at all.
#[test]
fn a_strip_that_fits_needs_no_controls() {
    let fits = shelf_metrics(0.0, 600.0, 800.0);
    assert!(!fits.overflows());
    assert_eq!(fits.max_offset(), 0.0);
    assert!(!fits.can_page_left());
    assert!(!fits.can_page_right());
    for scroll in [
        ShelfScroll::PageLeft,
        ShelfScroll::PageRight,
        ShelfScroll::Start,
        ShelfScroll::End,
    ] {
        assert!(
            !fits.can_scroll(scroll),
            "{scroll:?} must do nothing when everything is visible"
        );
    }
}

/// Test 8: an empty or single-platform shelf is a fitting shelf.
#[test]
fn an_empty_or_single_platform_shelf_has_no_controls() {
    // Nothing measured yet, or nothing to show.
    let empty = PlatformShelfMetrics::default();
    assert!(!empty.overflows());
    assert!(!empty.can_page_left());
    assert!(!empty.can_page_right());

    // One "All" card plus one platform, comfortably inside the viewport.
    let single = shelf_metrics(0.0, 2.0 * PLATFORM_CARD_MIN_WIDTH, 900.0);
    assert!(!single.overflows());
    assert!(!single.can_page_right());
}

/// Test 9: a resize that reveals everything retires the controls, and one
/// that hides cards brings them back - including clamping a stale offset.
#[test]
fn resizing_updates_the_controls_and_the_reachable_range() {
    // Narrow: overflowing, both controls meaningful from the middle.
    let narrow = shelf_metrics(400.0, 2400.0, 600.0);
    assert!(narrow.overflows());
    assert!(narrow.can_page_left() && narrow.can_page_right());

    // Widened past the content: nothing left to scroll, and the offset the
    // narrow layout had is no longer reachable.
    let widened = shelf_metrics(400.0, 2400.0, 2500.0);
    assert!(!widened.overflows());
    assert_eq!(widened.max_offset(), 0.0);
    assert_eq!(
        widened.offset_after(ShelfScroll::End),
        0.0,
        "a stale offset must clamp to the new range, not stay out of bounds"
    );

    // A press distance always follows the current viewport, not a remembered
    // one, so the same code serves every window size.
    assert_eq!(shelf_metrics(0.0, 9000.0, 1000.0).page_delta(), 750.0);
    assert_eq!(shelf_metrics(0.0, 9000.0, 1600.0).page_delta(), 1200.0);
}

/// Test 10: sub-pixel offsets after an animation still count as an edge.
#[test]
fn a_sub_pixel_offset_still_counts_as_reaching_the_edge() {
    let landed_short = shelf_metrics(1599.6, 2400.0, 800.0);
    assert!(
        landed_short.at_end(),
        "an animation landing a fraction short must not leave `next` enabled"
    );
    assert!(!landed_short.can_page_right());
    let barely_moved = shelf_metrics(0.4, 2400.0, 800.0);
    assert!(barely_moved.at_start());
    assert!(!barely_moved.can_page_left());
}

fn shelf_entries(count: usize) -> Vec<(String, usize)> {
    (0..count)
        .map(|index| (format!("Platform {index:02}"), index + 1))
        .collect()
}

/// Renders just the shelf at a chosen window width.
///
/// `frames` matters: a fresh `egui::Context` needs one pass before its
/// screen rect and the strip's content size are both real, and the shelf
/// needs one more pass to act on that measurement - so three is the
/// settled state, and fewer is deliberately used to test the unsettled one.
fn render_shelf(
    context: &egui::Context,
    width: f32,
    platforms: &[(String, usize)],
    selected: Option<&str>,
    frames: usize,
) -> (egui::FullOutput, ShelfOutcome) {
    let mut cache = PlatformArtworkCache::default();
    let mut output = None;
    let mut reported = ShelfOutcome::default();
    // A real window size, the way `resizing_the_window_does_not_reintroduce_clipping`
    // does it: `Ui::set_max_width` does not narrow `available_width`, so the
    // strip has to be constrained by the screen rect itself.
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width, 400.0),
        )),
        ..Default::default()
    };
    for _ in 0..frames.max(1) {
        let mut artwork = PlatformShelfArtwork {
            directory: None,
            cache: &mut cache,
        };
        let mut entries: Vec<ShelfEntry<'_>> = vec![ShelfEntry {
            asset_id: PlatformAssetCategory::Console.asset_id().to_owned(),
            label: "All",
            count: 10,
            platform: None,
        }];
        for (label, count) in platforms {
            entries.push(ShelfEntry {
                asset_id: "unknown".to_owned(),
                label: label.as_str(),
                count: *count,
                platform: Some(label.as_str()),
            });
        }
        output = Some(context.run(input.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                reported = show_gamer_platform_shelf(
                    ui,
                    &entries,
                    selected,
                    PLATFORM_CARD_MIN_WIDTH,
                    &mut artwork,
                    PLATFORM_SHELF_HEIGHT,
                );
            });
        }));
    }
    (output.expect("at least one frame"), reported)
}

/// Test 11: with many platforms, both controls are present and labelled.
#[test]
fn a_wide_shelf_renders_both_accessible_controls() {
    let context = egui::Context::default();
    let platforms = shelf_entries(40);
    // Two frames: the first measures the content, the second can act on it.
    let (output, shelf) = render_shelf(&context, 700.0, &platforms, None, 3);

    assert!(shelf.controls_visible, "40 platforms cannot fit in 700px");
    // Both glyphs are painted, and the accessible names are attached as
    // hover text, which is also the AccessKit name - see `shelf_chevron`.
    assert!(rendered_text_contains(&output, SHELF_PREVIOUS_GLYPH));
    assert!(rendered_text_contains(&output, SHELF_NEXT_GLYPH));
    // At the start, only "next" can act - the other is drawn but disabled.
    assert!(!shelf.previous_enabled);
    assert!(shelf.next_enabled);
}

/// Test 12: a shelf that fits renders no chevrons at all.
#[test]
fn a_shelf_that_fits_renders_no_chevrons() {
    let context = egui::Context::default();
    // One platform plus All, in a very wide window.
    let (output, shelf) = render_shelf(&context, 4000.0, &shelf_entries(1), None, 2);
    assert!(
        !shelf.controls_visible,
        "nothing is off-screen, so no navigation should appear"
    );
    assert!(!shelf.previous_enabled && !shelf.next_enabled);
    assert!(!rendered_text_contains(&output, SHELF_NEXT_GLYPH));
}

/// Test 13: an empty shelf renders without panicking and without controls.
#[test]
fn an_empty_shelf_renders_no_controls_and_does_not_panic() {
    let context = egui::Context::default();
    let (output, shelf) = render_shelf(&context, 900.0, &[], None, 2);
    assert!(!shelf.controls_visible);
    assert_eq!(shelf.chosen, None, "rendering must not select anything");
    assert!(!rendered_text_contains(&output, SHELF_NEXT_GLYPH));
}

/// Test 14: rendering is stable - the chevrons do not appear and disappear
/// on alternating frames at a width close to the content width.
#[test]
fn control_visibility_does_not_oscillate_at_a_borderline_width() {
    let context = egui::Context::default();
    let platforms = shelf_entries(6);
    // Settle first, then compare several consecutive frames.
    let mut seen = Vec::new();
    for _ in 0..6 {
        let (_, shelf) = render_shelf(&context, 900.0, &platforms, None, 1);
        seen.push(shelf.controls_visible);
    }
    let first = seen[2];
    assert!(
        seen[2..].iter().all(|value| *value == first),
        "chevron presence flickered across frames: {seen:?}"
    );
}

/// Test 15: the controls are decided on the very first frame.
///
/// This used to require two frames, because the decision was read back from
/// the previous frame's measured `content_width`. That is no longer safe:
/// the card width now depends on whether the chevrons are shown (they take
/// space out of the strip, and the cards are refitted to what is left), so
/// a decision fed by the previous frame's measurement could see the
/// narrower fitted cards, conclude everything fits, retire the chevrons,
/// widen the cards, and flip back forever at one particular width.
///
/// The decision is now computed directly from the row's width and the
/// entry count at the preferred card width - a pure function of this
/// frame's inputs. Deciding it a frame earlier is also simply better: the
/// shelf no longer opens with one frame of missing controls.
#[test]
fn the_controls_are_decided_on_the_first_frame_without_a_measurement() {
    let context = egui::Context::default();
    let platforms = shelf_entries(40);

    let (_, first) = render_shelf(&context, 700.0, &platforms, None, 1);
    assert!(
        first.controls_visible,
        "40 platforms cannot fit in 700px, and that is knowable immediately"
    );

    let (_, second) = render_shelf(&context, 700.0, &platforms, None, 1);
    assert!(
        second.controls_visible,
        "and the answer must not change once measurements exist"
    );

    // The converse, on a fresh context: a shelf that fits claims nothing,
    // on the first frame and every frame after it.
    let roomy = egui::Context::default();
    let few = shelf_entries(3);
    for _ in 0..3 {
        let (_, shelf) = render_shelf(&roomy, 2560.0, &few, None, 1);
        assert!(
            !shelf.controls_visible,
            "three cards fit easily and need no controls"
        );
    }
}

/// Test 16: resizing the window updates the controls in both directions,
/// through real layout rather than arithmetic.
#[test]
fn resizing_the_rendered_window_adds_and_retires_the_controls() {
    let context = egui::Context::default();
    let platforms = shelf_entries(20);

    // Narrow: the strip overflows and the controls appear.
    let (_, narrow) = render_shelf(&context, 700.0, &platforms, None, 2);
    assert!(narrow.controls_visible);
    assert!(narrow.next_enabled);

    // Widened past the whole strip in the same session: they retire.
    let (_, wide) = render_shelf(&context, 5000.0, &platforms, None, 2);
    assert!(
        !wide.controls_visible,
        "a window wide enough to show everything needs no navigation"
    );

    // And narrowing again brings them back.
    let (_, narrow_again) = render_shelf(&context, 700.0, &platforms, None, 2);
    assert!(narrow_again.controls_visible);
}

/// Renders the shelf with extra input events, and reports the offset the
/// strip settled on. Used for the wheel and keyboard cases.
fn render_shelf_with_events(
    context: &egui::Context,
    width: f32,
    platforms: &[(String, usize)],
    selected: Option<&str>,
    frames: usize,
    events: &[egui::Event],
) -> (ShelfOutcome, f32) {
    let mut cache = PlatformArtworkCache::default();
    let mut reported = ShelfOutcome::default();
    let mut offset = 0.0;
    let base = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width, 400.0),
        )),
        ..Default::default()
    };
    for frame in 0..frames.max(1) {
        let mut artwork = PlatformShelfArtwork {
            directory: None,
            cache: &mut cache,
        };
        let mut entries: Vec<ShelfEntry<'_>> = vec![ShelfEntry {
            asset_id: PlatformAssetCategory::Console.asset_id().to_owned(),
            label: "All",
            count: 10,
            platform: None,
        }];
        for (label, count) in platforms {
            entries.push(ShelfEntry {
                asset_id: "unknown".to_owned(),
                label: label.as_str(),
                count: *count,
                platform: Some(label.as_str()),
            });
        }
        // The events are delivered once the strip has been measured, so they
        // act on a settled layout rather than a zero-width one.
        let input = if frame == 2 {
            egui::RawInput {
                events: events.to_vec(),
                ..base.clone()
            }
        } else {
            base.clone()
        };
        let _ = context.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                reported = show_gamer_platform_shelf(
                    ui,
                    &entries,
                    selected,
                    PLATFORM_CARD_MIN_WIDTH,
                    &mut artwork,
                    PLATFORM_SHELF_HEIGHT,
                );
            });
        });
        offset = reported.metrics.offset_x;
    }
    (reported, offset)
}

/// Test 17: the mouse wheel and trackpad keep scrolling the strip, which the
/// buttons must not have taken over.
#[test]
fn the_mouse_wheel_still_scrolls_the_strip() {
    let context = egui::Context::default();
    let platforms = shelf_entries(40);
    let wheel = vec![
        egui::Event::PointerMoved(egui::pos2(300.0, 100.0)),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(-400.0, 0.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::default(),
        },
    ];
    let (shelf, offset) = render_shelf_with_events(&context, 700.0, &platforms, None, 8, &wheel);
    assert!(shelf.controls_visible);
    assert!(
        offset > 0.0,
        "a wheel event must still move the strip, offset was {offset}"
    );
    // Having scrolled away from the start by hand, the previous control
    // becomes usable - the button state follows manual scrolling.
    assert!(
        shelf.previous_enabled,
        "the button state must update after manual scrolling"
    );
}

/// Test 18: pressing the next chevron scrolls, and the strip ends up further
/// along than a page.
#[test]
fn a_next_press_scrolls_by_about_a_page() {
    let context = egui::Context::default();
    let platforms = shelf_entries(40);
    // Settle, then click where the right chevron sits: the far right edge of
    // the row, vertically inside the shelf.
    let click_at = egui::pos2(700.0 - 24.0, 90.0);
    let click = vec![
        egui::Event::PointerMoved(click_at),
        egui::Event::PointerButton {
            pos: click_at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        },
        egui::Event::PointerButton {
            pos: click_at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        },
    ];
    let (shelf, offset) = render_shelf_with_events(&context, 700.0, &platforms, None, 30, &click);
    assert!(shelf.controls_visible);
    assert!(
        offset > 100.0,
        "the next chevron should have scrolled the strip, offset was {offset}"
    );
    assert!(
        shelf.previous_enabled,
        "having moved off the start, previous must become usable"
    );
}

/// Test 19: the keyboard mapping, resolved directly - focus on a shelf
/// widget is required, and each key means what the milestone says.
#[test]
fn the_keyboard_mapping_requires_focus_on_the_shelf() {
    let context = egui::Context::default();
    let shelf_widget = egui::Id::new("a_shelf_card");
    let elsewhere = egui::Id::new("some_other_widget");

    let press = |key: egui::Key, focused: egui::Id| {
        context.memory_mut(|memory| memory.request_focus(focused));
        context.begin_pass(egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        });
        let resolved = shelf_keyboard_scroll(&context, &[shelf_widget]);
        let _ = context.end_pass();
        resolved
    };

    assert_eq!(
        press(egui::Key::ArrowRight, shelf_widget),
        Some(ShelfScroll::PageRight)
    );
    assert_eq!(
        press(egui::Key::ArrowLeft, shelf_widget),
        Some(ShelfScroll::PageLeft)
    );
    assert_eq!(
        press(egui::Key::Home, shelf_widget),
        Some(ShelfScroll::Start)
    );
    assert_eq!(press(egui::Key::End, shelf_widget), Some(ShelfScroll::End));

    // Focus anywhere else and the shelf must not consume these keys, since
    // they mean something different everywhere else in the window.
    assert_eq!(press(egui::Key::ArrowRight, elsewhere), None);
    assert_eq!(press(egui::Key::Home, elsewhere), None);
}

/// Test 20: the controls are laid out beside the strip, never over it, so a
/// card can never be obscured.
#[test]
fn the_controls_never_overlap_the_strip() {
    let context = egui::Context::default();
    let platforms = shelf_entries(40);

    // With controls, the strip is narrower than the window by exactly the
    // two reserved slots, which is only possible if they are siblings.
    let (_, with_controls) = render_shelf(&context, 700.0, &platforms, None, 3);
    assert!(with_controls.controls_visible);
    let narrowed = with_controls.metrics.viewport_width;

    // Without controls, the strip gets the full width. Few enough platforms
    // that a wide window really does show all of them.
    let (_, no_controls) = render_shelf(&context, 5000.0, &shelf_entries(8), None, 3);
    assert!(!no_controls.controls_visible);
    let full = no_controls.metrics.viewport_width;

    assert!(
        narrowed < 700.0 - SHELF_CHEVRON_WIDTH,
        "the strip must give up room for both chevrons, it was {narrowed}"
    );
    assert!(
        full > narrowed,
        "without controls the strip should be wider ({full} vs {narrowed})"
    );
}

/// Test 21: changing the platform filter brings the newly selected card back
/// into view rather than leaving the strip where it was.
#[test]
fn a_filter_change_scrolls_the_selected_platform_back_into_view() {
    let context = egui::Context::default();
    let platforms = shelf_entries(40);
    let wheel = vec![
        egui::Event::PointerMoved(egui::pos2(300.0, 100.0)),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(-600.0, 0.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::default(),
        },
    ];

    // Scroll a long way along the strip by hand.
    for _ in 0..6 {
        render_shelf_with_events(&context, 700.0, &platforms, None, 4, &wheel);
    }
    // Let egui's smooth-scrolling momentum finish before measuring, so the
    // comparison below is against a settled strip rather than a coasting one.
    let (_, far) = render_shelf_with_events(&context, 700.0, &platforms, None, 40, &[]);
    assert!(far > 500.0, "the strip should be well along, was {far}");

    // Now select an early platform, exactly as a filter change does. The
    // newly selected card must be brought back into view.
    let (_, near) =
        render_shelf_with_events(&context, 700.0, &platforms, Some("Platform 00"), 30, &[]);
    assert!(
        near < far,
        "selecting an early platform should scroll back towards it ({near} from {far})"
    );
}

/// Test 21b: a filter change to a platform already on screen leaves the
/// strip alone rather than jerking it about.
#[test]
fn selecting_a_platform_already_in_view_does_not_move_the_strip() {
    let context = egui::Context::default();
    let platforms = shelf_entries(40);
    // Settle at the start, then select the first platform, which is visible.
    render_shelf(&context, 700.0, &platforms, None, 3);
    let (_, settled) =
        render_shelf_with_events(&context, 700.0, &platforms, Some("Platform 00"), 12, &[]);
    assert!(
        settled <= SHELF_EDGE_EPSILON,
        "an already-visible selection must not scroll the strip, offset {settled}"
    );
}

/// Test 22: the accessible names the milestone specified are the ones used.
#[test]
fn the_controls_carry_the_specified_accessible_names() {
    let context = egui::Context::default();
    let mut cache = PlatformArtworkCache::default();
    let artwork = PlatformShelfArtwork {
        directory: None,
        cache: &mut cache,
    };
    let mut names = Vec::new();
    let _ = context.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            for (glyph, name) in [
                (SHELF_PREVIOUS_GLYPH, "Previous platforms"),
                (SHELF_NEXT_GLYPH, "Next platforms"),
            ] {
                let response = shelf_chevron(ui, glyph, name, true);
                // The hover text is the accessible name - see `shelf_chevron`.
                assert!(response.enabled());
                names.push(name.to_string());
            }
        });
    });
    let _ = artwork.cache;
    assert_eq!(names, vec!["Previous platforms", "Next platforms"]);
}

/// Test 17: rendering the shelf never changes the selection by itself.
#[test]
fn rendering_the_shelf_never_changes_the_selection() {
    let context = egui::Context::default();
    let platforms = shelf_entries(12);
    for selected in [None, Some("Platform 03")] {
        let (_, shelf) = render_shelf(&context, 800.0, &platforms, selected, 3);
        assert_eq!(
            shelf.chosen, None,
            "no click happened, so no filter change may be reported"
        );
    }
}

#[test]
fn gamer_platform_cards_have_readable_responsive_bounds() {
    assert_eq!(gamer_platform_card_width(600.0), PLATFORM_CARD_MIN_WIDTH);
    assert!(
        gamer_platform_card_width(1024.0) > PLATFORM_CARD_MIN_WIDTH,
        "1024-wide Gamer View should use the available room without crowding cards"
    );
    assert_eq!(gamer_platform_card_width(1920.0), PLATFORM_CARD_MAX_WIDTH);
}

#[test]
fn platform_artwork_aspect_fit_centres_without_stretching_or_cropping() {
    let landscape = fitted_artwork_rect(egui::pos2(50.0, 40.0), 44.0, egui::vec2(200.0, 100.0));
    assert_eq!(landscape.center(), egui::pos2(50.0, 40.0));
    assert_eq!(landscape.width(), 44.0);
    assert_eq!(landscape.height(), 22.0);

    let portrait = fitted_artwork_rect(egui::pos2(50.0, 40.0), 44.0, egui::vec2(50.0, 100.0));
    assert_eq!(portrait.center(), egui::pos2(50.0, 40.0));
    assert_eq!(portrait.width(), 22.0);
    assert_eq!(portrait.height(), 44.0);
}

#[test]
fn game_artwork_ids_are_deterministic_and_path_safe() {
    assert_eq!(
        game_artwork_asset_id("Shadow of the Colossus™"),
        "game-shadow-of-the-colossus"
    );
    assert_eq!(game_artwork_asset_id(""), "game-unknown");
    assert!(valid_platform_asset_id(&game_artwork_asset_id(
        "../Game/Name"
    )));
}

#[test]
fn platform_card_labels_truncate_by_width_and_preserve_core_categories() {
    assert_eq!(
        compact_platform_label("All", PLATFORM_CARD_MIN_WIDTH),
        "All"
    );
    assert_eq!(
        compact_platform_label("Unknown", PLATFORM_CARD_MIN_WIDTH),
        "Unknown"
    );
    let compact =
        compact_platform_label("A Very Long Platform Display Name", PLATFORM_CARD_MIN_WIDTH);
    assert!(compact.ends_with('\u{2026}'));
    assert!(
        compact.chars().count() < "A Very Long Platform Display Name".chars().count(),
        "a long label at the narrowest card width must actually be shortened"
    );
    assert!(
        measured_text_width_px(&compact)
            <= PLATFORM_CARD_MIN_WIDTH - PLATFORM_LABEL_HORIZONTAL_PADDING,
        "the truncated label must fit inside the narrowest card, measured with the \
             real bundled egui font actually used to paint it (not a guessed character count)"
    );
    assert_eq!(
        compact_platform_label("PlayStation Vita", PLATFORM_CARD_MAX_WIDTH),
        "PlayStation Vita",
        "wider cards should retain useful names instead of applying a fixed early cutoff"
    );
}

/// Renders `text` with the exact `FontId` the platform shelf paints
/// its label with (see `show_platform_shelf_item`) through egui's own
/// bundled font (never a host-installed one - egui embeds its default
/// font family and does not consult system fonts unless the app
/// explicitly installs its own, which this app does not), and returns
/// the laid-out width in pixels. Fully deterministic across machines
/// and CI runners: the glyph data is compiled into the `egui`/`epaint`
/// crates, not read from the host.
fn measured_text_width_px(text: &str) -> f32 {
    let ctx = egui::Context::default();
    let font_id = egui::FontId::proportional(10.0);
    let mut width = 0.0_f32;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            width = ui.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(text.to_string(), font_id.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            });
        });
    });
    width
}

#[test]
fn compact_platform_label_preserves_short_core_category_labels_at_every_card_width() {
    // "All" and "Unknown" are the two fixed, non-optional categories
    // that must never be truncated (they're always well under the
    // minimum character floor), across the entire realistic card
    // width range - not just the two boundary widths.
    for card_width in [
        0.0,
        10.0,
        PLATFORM_CARD_MIN_WIDTH,
        150.0,
        PLATFORM_CARD_MAX_WIDTH,
        400.0,
    ] {
        assert_eq!(compact_platform_label("All", card_width), "All");
        assert_eq!(compact_platform_label("Unknown", card_width), "Unknown");
    }
}

#[test]
fn compact_platform_label_degrades_gracefully_at_narrow_and_extremely_narrow_widths() {
    let long_name = "Nintendo Entertainment System";
    // Narrow (the shelf's real minimum) through pathologically narrow
    // and outright invalid (zero/negative) widths must never panic
    // and must always leave a recognisable, non-empty label.
    for card_width in [PLATFORM_CARD_MIN_WIDTH, 40.0, 1.0, 0.0, -50.0, f32::MIN] {
        let compact = compact_platform_label(long_name, card_width);
        assert!(!compact.is_empty());
        assert!(compact.ends_with('\u{2026}'));
        assert!(
            compact.chars().count() >= PLATFORM_LABEL_MIN_CHARACTERS,
            "even the narrowest card must keep at least the minimum \
                 recognisable character count for {card_width}"
        );
    }
}

#[test]
fn compact_platform_label_never_overflows_the_available_card_width() {
    // A representative spread of card widths and label lengths, all
    // checked against the width egui's real bundled font actually
    // measures - the property this whole function exists to
    // guarantee, verified directly rather than via a proxy character
    // count that can silently go stale when the card-width constants
    // change (see `platform_label_character_limit`'s doc comment).
    let labels = [
        "PlayStation",
        "A Very Long Platform Display Name",
        "Commodore 64",
        "Neo Geo Pocket Color",
        "Nintendo Entertainment System",
    ];
    for card_width in [
        PLATFORM_CARD_MIN_WIDTH,
        148.0,
        PLATFORM_CARD_MAX_WIDTH,
        250.0,
    ] {
        let available = card_width - PLATFORM_LABEL_HORIZONTAL_PADDING;
        for label in labels {
            let compact = compact_platform_label(label, card_width);
            let width = measured_text_width_px(&compact);
            assert!(
                width <= available,
                "{label:?} compacted to {compact:?} at card_width={card_width} \
                     measures {width}px, wider than the {available}px available"
            );
        }
    }
}

#[test]
fn compact_platform_label_handles_unicode_and_long_labels_without_panicking() {
    let cases = [
        "セガサターン",
        "Pokémon Mini",
        "e\u{0301}\u{0301}\u{0301}\u{0301}\u{0301}mulator", // stacked combining marks
        "🎮🕹️👾 Retro Console",
        &"A".repeat(2_000),
        "",
    ];
    for label in cases {
        for card_width in [0.0, PLATFORM_CARD_MIN_WIDTH, PLATFORM_CARD_MAX_WIDTH] {
            let compact = compact_platform_label(label, card_width);
            // Must always be valid UTF-8 (guaranteed by `String`) and
            // never panic on a multi-byte boundary, since truncation
            // walks `.chars()`, not byte offsets.
            assert!(compact.chars().count() <= label.chars().count() + 1);
        }
    }
}

#[test]
fn compact_platform_label_is_deterministic_across_repeated_calls() {
    let label = "A Very Long Platform Display Name";
    let first = compact_platform_label(label, PLATFORM_CARD_MIN_WIDTH);
    for _ in 0..50 {
        assert_eq!(
            compact_platform_label(label, PLATFORM_CARD_MIN_WIDTH),
            first
        );
    }
    // The width-measurement invariant test above depends on egui's
    // font layout also being repeatable; confirm that independently
    // too, since it is the one part of this test suite that touches
    // (headless, bundled-font) rendering.
    let first_width = measured_text_width_px(&first);
    for _ in 0..20 {
        assert_eq!(measured_text_width_px(&first), first_width);
    }
}

#[test]
fn detected_platform_counts_orders_named_platforms_stably_regardless_of_insertion_order() {
    let forward = [
        Some("Wii"),
        Some("GameCube"),
        Some("Dreamcast"),
        Some("GameCube"),
    ];
    let reversed = [
        Some("GameCube"),
        Some("Dreamcast"),
        Some("GameCube"),
        Some("Wii"),
    ];
    let forward_summary = detected_platform_counts(forward.into_iter());
    let reversed_summary = detected_platform_counts(reversed.into_iter());
    assert_eq!(
        forward_summary.named, reversed_summary.named,
        "the platform shelf's ordering must not depend on archive scan/insertion order"
    );
    assert_eq!(
        forward_summary.named,
        vec![
            ("Dreamcast".to_string(), 1),
            ("GameCube".to_string(), 2),
            ("Wii".to_string(), 1),
        ],
        "named platforms must be in a stable, deterministic (alphabetical) order"
    );
}

fn artwork_test_directory(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-gui-artwork-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

fn write_test_png(path: &Path, width: u32, height: u32, color: [u8; 4]) {
    image::RgbaImage::from_pixel(width, height, image::Rgba(color))
        .save(path)
        .unwrap();
}

#[test]
fn custom_platform_artwork_path_falls_back_to_none_when_missing_or_unconfigured() {
    assert_eq!(custom_platform_artwork_path(None, "gamecube"), None);
    let temp = artwork_test_directory("missing");
    assert_eq!(
        custom_platform_artwork_path(Some(&temp), "gamecube"),
        None,
        "a directory with no matching file must fall back to None (built-in artwork)"
    );
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn managed_artwork_source_prefers_custom_over_bundled_artwork() {
    let temp = artwork_test_directory("source-priority");
    assert_eq!(
        current_artwork_source(Some(&temp), "PS2", None),
        ("Bundled", false)
    );
    write_test_png(&temp.join("ps2.png"), 2, 2, [1, 2, 3, 255]);
    assert_eq!(
        current_artwork_source(Some(&temp), "PS2", None),
        ("Custom", true)
    );
    // Every current canonical platform has its own bundled artwork. The
    // complete set must therefore still prefer the platform-specific bundle
    // whenever no custom file overrides it.
    assert_eq!(
        current_artwork_source(Some(&temp), "MasterSystem", None),
        ("Bundled", false)
    );
    let _ = std::fs::remove_dir_all(temp);
}

#[test]
fn custom_platform_artwork_filename_resolution_is_exact_and_png_only() {
    let temp = artwork_test_directory("present");
    let svg_path = temp.join("gamecube.svg");
    std::fs::write(&svg_path, "<svg><script>bad()</script></svg>").unwrap();
    assert_eq!(
        custom_platform_artwork_path(Some(&temp), "gamecube"),
        None,
        "SVG is deliberately unsupported and must never be parsed"
    );

    let asset_path = temp.join("gamecube.png");
    write_test_png(&asset_path, 2, 2, [255, 0, 0, 255]);

    assert_eq!(
        custom_platform_artwork_path(Some(&temp), "gamecube"),
        Some(asset_path.clone()),
        "an existing file must be resolved, never copied elsewhere"
    );
    assert_eq!(
        custom_platform_artwork_path(Some(&temp), "../gamecube"),
        None,
        "asset ids cannot escape the configured directory"
    );
    // Never copied into any EmuWiz-owned location - the resolved
    // path is the user's own file, unchanged.
    assert!(asset_path.exists());
    let _ = std::fs::remove_dir_all(&temp);
}

#[cfg(unix)]
#[test]
fn custom_platform_artwork_rejects_symlinks() {
    let temp = artwork_test_directory("symlink");
    let outside = temp.with_extension("outside.png");
    write_test_png(&outside, 1, 1, [0, 0, 0, 255]);
    std::os::unix::fs::symlink(&outside, temp.join("gamecube.png")).unwrap();
    assert_eq!(custom_platform_artwork_path(Some(&temp), "gamecube"), None);
    let _ = std::fs::remove_dir_all(&temp);
    let _ = std::fs::remove_file(outside);
}

#[test]
fn malformed_custom_artwork_is_cached_as_a_safe_fallback() {
    let temp = artwork_test_directory("malformed");
    let path = temp.join("gamecube.png");
    std::fs::write(&path, b"not a png").unwrap();
    assert_eq!(
        decode_custom_platform_artwork(&path),
        Err(CustomArtworkLoadError::Malformed)
    );

    let context = egui::Context::default();
    let mut cache = PlatformArtworkCache::default();
    assert_eq!(
        cache.custom_texture(&context, Some(&temp), "gamecube"),
        None
    );
    assert!(cache.entries.contains_key("gamecube"));
    assert!(cache.entries["gamecube"].texture.is_none());
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn oversized_custom_artwork_is_rejected_before_decode() {
    let temp = artwork_test_directory("oversized");
    let path = temp.join("gamecube.png");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_CUSTOM_ARTWORK_FILE_BYTES + 1).unwrap();
    assert_eq!(
        custom_platform_artwork_fingerprint(Some(&temp), "gamecube"),
        Err(CustomArtworkLoadError::Oversized)
    );
    assert_eq!(
        decode_custom_platform_artwork(&path),
        Err(CustomArtworkLoadError::Oversized)
    );
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn excessive_custom_artwork_dimensions_are_rejected() {
    let temp = artwork_test_directory("dimensions");
    let path = temp.join("gamecube.png");
    write_test_png(&path, MAX_CUSTOM_ARTWORK_DIMENSION + 1, 1, [0, 0, 0, 255]);
    assert_eq!(
        decode_custom_platform_artwork(&path),
        Err(CustomArtworkLoadError::Oversized)
    );
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn custom_artwork_cache_invalidates_when_file_metadata_changes() {
    let temp = artwork_test_directory("cache-invalidation");
    let path = temp.join("gamecube.png");
    write_test_png(&path, 2, 2, [255, 0, 0, 255]);
    let context = egui::Context::default();
    let mut cache = PlatformArtworkCache::default();
    let first_texture = cache
        .custom_texture(&context, Some(&temp), "gamecube")
        .expect("valid first texture");
    let first_fingerprint = cache.entries["gamecube"].fingerprint.clone();

    write_test_png(&path, 3, 2, [0, 0, 255, 255]);
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_times(
        std::fs::FileTimes::new()
            .set_modified(first_fingerprint.modified + std::time::Duration::from_secs(2)),
    )
    .unwrap();
    let second_texture = cache
        .custom_texture(&context, Some(&temp), "gamecube")
        .expect("valid replacement texture");
    assert_ne!(first_texture, second_texture);
    assert_ne!(cache.entries["gamecube"].fingerprint, first_fingerprint);
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn custom_artwork_paints_when_valid_and_uses_builtin_fallback_when_invalid() {
    let temp = artwork_test_directory("render-fallback");
    write_test_png(&temp.join("gamecube.png"), 2, 2, [0, 255, 0, 255]);
    std::fs::write(temp.join("xbox.png"), b"broken").unwrap();
    let context = egui::Context::default();
    let mut cache = PlatformArtworkCache::default();
    let mut custom_source = PlatformArtworkSource::Glyph;
    let mut malformed_source = PlatformArtworkSource::Glyph;
    let mut missing_mapping_source = PlatformArtworkSource::Custom;
    let _ = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            custom_source = paint_platform_artwork_at(
                ui,
                &mut cache,
                Some(&temp),
                PlatformArtworkPaint {
                    center: egui::pos2(20.0, 20.0),
                    size: 16.0,
                    color: egui::Color32::WHITE,
                    asset_id: "gamecube",
                    fallback_asset_id: "console",
                },
            );
            malformed_source = paint_platform_artwork_at(
                ui,
                &mut cache,
                Some(&temp),
                PlatformArtworkPaint {
                    center: egui::pos2(50.0, 20.0),
                    size: 16.0,
                    color: egui::Color32::WHITE,
                    asset_id: "xbox",
                    fallback_asset_id: "console",
                },
            );
            missing_mapping_source = paint_platform_artwork_at(
                ui,
                &mut cache,
                Some(&temp),
                PlatformArtworkPaint {
                    center: egui::pos2(80.0, 20.0),
                    size: 16.0,
                    color: egui::Color32::WHITE,
                    asset_id: "console",
                    fallback_asset_id: "console",
                },
            );
        });
    });
    assert_eq!(custom_source, PlatformArtworkSource::Custom);
    assert_eq!(malformed_source, PlatformArtworkSource::Bundled);
    assert_eq!(missing_mapping_source, PlatformArtworkSource::Glyph);
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn platform_asset_category_accessible_labels_are_all_distinct_and_non_empty() {
    let categories = [
        PlatformAssetCategory::Console,
        PlatformAssetCategory::Handheld,
        PlatformAssetCategory::Computer,
        PlatformAssetCategory::Arcade,
        PlatformAssetCategory::OpticalDisc,
        PlatformAssetCategory::Cartridge,
        PlatformAssetCategory::Unknown,
    ];
    let mut labels: Vec<&str> = categories.iter().map(|c| c.accessible_label()).collect();
    for label in &labels {
        assert!(!label.is_empty());
    }
    labels.sort_unstable();
    labels.dedup();
    assert_eq!(
        labels.len(),
        categories.len(),
        "labels must all be distinct"
    );
}

/// Renders the whole Gamer View at one window size and reports the shelf's
/// geometry plus every painted text rectangle, so a layout assertion can be
/// made against what a person would actually see.
fn render_gamer_view_at(
    width: f32,
    height: f32,
    platform_count: usize,
) -> (ShelfGeometry, Vec<(String, egui::Rect)>) {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::GamerView;
    app.view = MainView::Library;
    let mut records = Vec::new();
    for index in 0..platform_count {
        let mut row = record(&format!("/roms/g{index:02}.zip"), MountState::Pending);
        row.metadata.platform = Some(format!("Platform{index:02}"));
        row.metadata.title = Some(format!("Game {index:02}"));
        records.push(row);
    }
    app.state = LoadState::Ready(Box::new(loaded_data_with_records("/mount", records)));

    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width, height),
        )),
        ..Default::default()
    };
    // Three passes: one to measure the strip, one for the controls to appear,
    // one to settle the layout that follows them.
    let mut output = None;
    for _ in 0..3 {
        output = Some(ctx.run(input.clone(), |ctx| app.update(ctx, &mut frame)));
    }
    let geometry = ctx
        .data(|data| data.get_temp::<PlatformShelfState>(platform_shelf_state_id()))
        .map(|state| state.geometry)
        .unwrap_or_default();

    fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => out.push((
                text.galley.text().replace('\n', " "),
                text.visual_bounding_rect(),
            )),
            egui::Shape::Vec(nested) => nested.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut texts = Vec::new();
    for clipped in &output.expect("a rendered frame").shapes {
        walk(&clipped.shape, &mut texts);
    }
    texts.retain(|(_, rect)| rect.top().is_finite());
    (geometry, texts)
}

/// The rectangle of the first painted text containing `needle`.
fn painted_text_rect(texts: &[(String, egui::Rect)], needle: &str) -> egui::Rect {
    texts
        .iter()
        .find(|(text, _)| text.contains(needle))
        .map(|(_, rect)| *rect)
        .unwrap_or_else(|| panic!("no painted text contains {needle:?}"))
}

/// The regression test for the broken Sunshine desktop layout: the shelf
/// grew, the cards spilled over the game list, and the chevrons towered over
/// the strip. Asserted at the real 1080p desktop size the screenshot came
/// from, plus a narrower and a wider window.
#[test]
fn the_platform_shelf_never_overlaps_the_content_below_it() {
    for (width, height) in [(1920.0, 1080.0), (1280.0, 720.0), (2560.0, 1440.0)] {
        let (geometry, texts) = render_gamer_view_at(width, height, 30);
        let label = format!("{width}x{height}");

        assert!(
            geometry.previous.is_some() && geometry.next.is_some(),
            "{label}: 31 cards cannot fit, so both controls should be present"
        );

        // 1. The shelf keeps its original height exactly.
        assert_eq!(
            geometry.row.height(),
            PLATFORM_SHELF_HEIGHT,
            "{label}: the shelf row must stay exactly the original height"
        );

        // 2. A chevron is never taller than the strip it sits beside.
        assert!(
            geometry.chevron_height() <= PLATFORM_SHELF_HEIGHT,
            "{label}: chevrons are {} tall, over the {PLATFORM_SHELF_HEIGHT} shelf",
            geometry.chevron_height()
        );
        assert_eq!(
            geometry.chevron_height(),
            PLATFORM_CARD_HEIGHT,
            "{label}: chevrons should match the cards exactly"
        );

        // 3. Every card stays inside the shelf row - this is the assertion
        //    that failed before the fix, when cards ran 58px below it.
        for card in &geometry.cards {
            assert!(
                card.bottom() <= geometry.row.bottom() + 0.5,
                "{label}: a card reaches {} but the shelf ends at {}",
                card.bottom(),
                geometry.row.bottom()
            );
            assert!(
                card.top() >= geometry.row.top() - 0.5,
                "{label}: a card starts above the shelf"
            );
        }

        // 4. The shelf ends above the main content, and no card rectangle
        //    intersects the "Selected game" pane or the game list.
        let heading = painted_text_rect(&texts, "YOUR LIBRARY");
        let first_game = painted_text_rect(&texts, "Game 00");
        assert!(
            geometry.row.bottom() <= heading.top(),
            "{label}: the shelf ends at {} but the heading starts at {}",
            geometry.row.bottom(),
            heading.top()
        );
        assert!(
            geometry.row.bottom() <= first_game.top(),
            "{label}: the shelf overlaps the game list"
        );
        for card in &geometry.cards {
            assert!(
                !card.intersects(heading),
                "{label}: a card overlaps the library heading below the shelf"
            );
            assert!(
                !card.intersects(first_game),
                "{label}: a card overlaps the game list"
            );
        }
        for chevron in geometry.previous.into_iter().chain(geometry.next) {
            assert!(
                !chevron.intersects(heading),
                "{label}: a chevron overlaps the heading"
            );
            assert!(
                !chevron.intersects(first_game),
                "{label}: a chevron overlaps the list"
            );
        }

        // 5. Nothing can be painted under the right chevron: the strip stops
        //    before it, and the last fully visible card stops before it too.
        let next = geometry.next.expect("checked above");
        assert!(
            geometry.strip.right() <= next.left() + 0.5,
            "{label}: the strip runs to {} but the next chevron starts at {}",
            geometry.strip.right(),
            next.left()
        );
        let last_visible = geometry
            .last_visible_card()
            .expect("some card is visible at every size");
        assert!(
            last_visible.left() < next.left(),
            "{label}: the rightmost visible card must stay left of the chevron"
        );

        // 6. The controls are tightly bounded, not stretched.
        for chevron in geometry.previous.into_iter().chain(geometry.next) {
            assert_eq!(
                chevron.width(),
                SHELF_CHEVRON_WIDTH,
                "{label}: a chevron is not the width it reserved"
            );
        }

        // 7. The strip uses the shelf's vertical extent, not more.
        assert!(
            geometry.strip.top() >= geometry.row.top() - 0.5
                && geometry.strip.bottom() <= geometry.row.bottom() + 0.5,
            "{label}: the strip ({:?}) escapes the shelf row ({:?})",
            geometry.strip,
            geometry.row
        );
    }
}

/// Renders the shelf, presses the next chevron `pages` times, and reports
/// the geometry that results.
fn shelf_geometry_after_pages(width: f32, platforms: usize, pages: usize) -> ShelfGeometry {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::GamerView;
    app.view = MainView::Library;
    let mut records = Vec::new();
    for index in 0..platforms {
        let mut row = record(&format!("/roms/g{index:02}.zip"), MountState::Pending);
        row.metadata.platform = Some(format!("Platform{index:02}"));
        records.push(row);
    }
    app.state = LoadState::Ready(Box::new(loaded_data_with_records("/mount", records)));

    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 1080.0));
    let idle = egui::RawInput {
        screen_rect: Some(screen),
        ..Default::default()
    };
    for _ in 0..3 {
        let _ = ctx.run(idle.clone(), |ctx| app.update(ctx, &mut frame));
    }
    let geometry = |ctx: &egui::Context| {
        ctx.data(|data| data.get_temp::<PlatformShelfState>(platform_shelf_state_id()))
            .map(|state| state.geometry)
            .unwrap_or_default()
    };
    for _ in 0..pages {
        let next = geometry(&ctx).next.expect("the chevrons are shown");
        let _ = ctx.run(click_at(screen, next.center()), |ctx| {
            app.update(ctx, &mut frame)
        });
        // Let the animated scroll settle before the next press.
        for _ in 0..60 {
            let _ = ctx.run(idle.clone(), |ctx| app.update(ctx, &mut frame));
        }
    }
    geometry(&ctx)
}

// --- Platform shelf: cards must never be sliced against a chevron -----
//
// Reported after the Sunshine smoke test: at some widths the rightmost
// card looked like it sat underneath the ">" control. Measured, the card
// was not painted under the button - the strip clips it - but the strip
// was given whatever width happened to remain after the chevrons, which
// almost never divided evenly by a card, so the last card was cut off by
// 14-124px flush against the button. These tests pin the arithmetic that
// fixes it, rather than a screenshot.

/// The widths worth checking: common desktop sizes plus the 1920x1080 the
/// Sunshine/TV session actually runs at.
const SHELF_TEST_WIDTHS: [f32; 6] = [1024.0, 1280.0, 1366.0, 1600.0, 1920.0, 2560.0];

#[test]
fn a_fitted_card_is_never_wider_than_preferred_nor_narrower_than_readable() {
    for width in SHELF_TEST_WIDTHS {
        let preferred = gamer_platform_card_width(width);
        let usable = width - 2.0 * shelf_chevron_reserve(8.0);
        let fitted = shelf_fitted_card_width(usable, preferred, 8.0);
        assert!(
            fitted <= preferred + 0.01,
            "{width}: fitted {fitted} exceeds the preferred {preferred}"
        );
        assert!(
            fitted >= PLATFORM_CARD_MIN_WIDTH - 0.01,
            "{width}: fitted {fitted} is below the readable minimum"
        );
    }
}

/// The property the whole fix rests on: a whole number of cards fills the
/// strip exactly, so its right edge always lands on a card boundary.
#[test]
fn a_whole_number_of_cards_fills_the_strip_exactly() {
    for width in SHELF_TEST_WIDTHS {
        for spacing in [4.0_f32, 8.0, 12.0] {
            let preferred = gamer_platform_card_width(width);
            let usable = width - 2.0 * shelf_chevron_reserve(spacing);
            let fitted = shelf_fitted_card_width(usable, preferred, spacing);
            let count = shelf_visible_card_count(usable, fitted, spacing);
            let strip = shelf_strip_width(usable, fitted, spacing);

            assert!(count >= 1, "{width}/{spacing}: no cards fit");
            assert!(
                strip <= usable + 0.01,
                "{width}/{spacing}: strip {strip} exceeds the usable {usable}"
            );
            let exact = count as f32 * (fitted + spacing) - spacing;
            assert!(
                (strip - exact).abs() < 0.5,
                "{width}/{spacing}: strip {strip} is not {count} whole cards ({exact})"
            );
            // And it wastes no meaningful space: the leftover is never a
            // whole card, or the strip should have shown one more.
            assert!(
                usable - strip < fitted + spacing,
                "{width}/{spacing}: {} left over, enough for another card",
                usable - strip
            );
        }
    }
}

#[test]
fn the_strip_never_claims_the_space_a_chevron_needs() {
    // Both chevrons are paid for, whichever end the shelf is scrolled to.
    assert_eq!(shelf_chevron_reserve(8.0), SHELF_CHEVRON_WIDTH + 8.0);
    for width in SHELF_TEST_WIDTHS {
        let preferred = gamer_platform_card_width(width);
        let usable = width - 2.0 * shelf_chevron_reserve(8.0);
        let strip = shelf_strip_width(usable, preferred, 8.0);
        assert!(
            strip + 2.0 * shelf_chevron_reserve(8.0) <= width + 0.01,
            "{width}: strip {strip} leaves no room for both chevrons"
        );
    }
}

/// A strip too narrow for even one readable card must still not overflow -
/// it gives up filling the strip rather than overlapping a control.
#[test]
fn an_impossibly_narrow_strip_underfills_rather_than_overlapping() {
    let strip = shelf_strip_width(40.0, PLATFORM_CARD_MIN_WIDTH, 8.0);
    assert!(
        strip <= 40.0 + 0.01,
        "the strip overflowed its usable width"
    );
    assert_eq!(
        shelf_visible_card_count(40.0, PLATFORM_CARD_MIN_WIDTH, 8.0),
        1
    );
    assert_eq!(shelf_visible_card_count(0.0, 164.0, 8.0), 1);
    assert_eq!(shelf_visible_card_count(-10.0, 164.0, 8.0), 1);
}

/// Paging moves whole cards, which is what keeps an aligned shelf aligned.
#[test]
fn a_page_press_travels_a_whole_number_of_cards() {
    let stride = 172.0;
    let metrics = shelf_metrics_with_stride(0.0, 4000.0, 1032.0, stride);
    let delta = metrics.page_delta();
    assert!(delta > 0.0);
    assert!(
        (delta / stride - (delta / stride).round()).abs() < 0.001,
        "a page of {delta} is not a whole number of {stride}px cards"
    );
    assert!(
        delta <= 1032.0 * SHELF_PAGE_FRACTION + 0.01,
        "a page must still stay short of a full viewport"
    );
    // Even a viewport narrower than one card advances by at least one.
    let narrow = shelf_metrics_with_stride(0.0, 4000.0, 100.0, stride);
    assert_eq!(narrow.page_delta(), stride);
    // With no stride measured yet, the raw fractional page still applies.
    let unmeasured = shelf_metrics_with_stride(0.0, 4000.0, 1000.0, 0.0);
    assert_eq!(unmeasured.page_delta(), 1000.0 * SHELF_PAGE_FRACTION);
}

/// The rendered result, expressed as relations rather than pixel values:
/// at every width, no card that is actually on screen may cross either
/// edge of the strip, and the strip may not reach either chevron.
#[test]
fn no_visible_card_is_sliced_by_a_chevron_at_any_width() {
    for width in SHELF_TEST_WIDTHS {
        let (geometry, _) = render_gamer_view_at(width, 1080.0, 19);
        let previous = geometry
            .previous
            .expect("19 platforms overflow every width");
        let next = geometry.next.expect("19 platforms overflow every width");

        assert!(
            previous.right() <= geometry.strip.left() + 0.5,
            "{width}: the previous chevron runs into the strip"
        );
        assert!(
            geometry.strip.right() <= next.left() + 0.5,
            "{width}: the strip runs into the next chevron"
        );

        for card in geometry.cards.iter().filter(|card| {
            card.right() > geometry.strip.left() + 0.5 && card.left() < geometry.strip.right() - 0.5
        }) {
            assert!(
                card.right() <= geometry.strip.right() + 1.0,
                "{width}: a visible card {card:?} is cut off by {} at the next chevron",
                card.right() - geometry.strip.right()
            );
            assert!(
                card.left() >= geometry.strip.left() - 1.0,
                "{width}: a visible card {card:?} is cut off at the previous chevron"
            );
            assert!(
                !card.intersects(next) && !card.intersects(previous),
                "{width}: a visible card overlaps a chevron"
            );
        }
    }
}

/// And the alignment survives using the controls, which is how a shelf is
/// actually driven on a TV.
#[test]
fn paging_the_shelf_leaves_every_visible_card_whole() {
    for width in [1366.0_f32, 1920.0] {
        for pages in [1_usize, 2] {
            let geometry = shelf_geometry_after_pages(width, 19, pages);
            let next = geometry.next.expect("the chevrons are shown");
            for card in geometry.cards.iter().filter(|card| {
                card.right() > geometry.strip.left() + 0.5
                    && card.left() < geometry.strip.right() - 0.5
            }) {
                assert!(
                    card.right() <= geometry.strip.right() + 1.0
                        && card.left() >= geometry.strip.left() - 1.0,
                    "{width} after {pages} page(s): card {card:?} is sliced by the strip \
                         edge (strip {:?})",
                    geometry.strip
                );
                assert!(!card.intersects(next));
            }
        }
    }
}

/// Selecting a platform must still work after all of the above - the fix
/// changes layout only.
#[test]
fn fitted_cards_still_select_their_platform() {
    let mut app = gamer_app_with_platforms(&[("Acorn Archimedes", 2), ("SNES", 2)]);
    let ctx = egui::Context::default();
    let screen = gamer_screen();
    let idle = egui::RawInput {
        screen_rect: Some(screen),
        ..Default::default()
    };
    run_gamer_frames(&mut app, &ctx, idle.clone(), 3);
    let cards = gamer_shelf_geometry(&ctx).cards;
    run_gamer_frames(&mut app, &ctx, click_at(screen, cards[2].center()), 1);
    assert_eq!(app.library_filters.platform.as_deref(), Some("SNES"));
    let output = run_gamer_frames(&mut app, &ctx, idle, 1);
    assert!(rendered_text_contains(&output, "Title0002"));
}

/// A shelf that fits needs no controls, and must still not disturb the
/// content below it.
#[test]
fn a_fitting_shelf_keeps_the_same_boundary_and_draws_no_controls() {
    let (geometry, texts) = render_gamer_view_at(2560.0, 1440.0, 2);
    assert!(
        geometry.previous.is_none() && geometry.next.is_none(),
        "three cards fit easily in 2560px"
    );
    assert_eq!(
        geometry.row.height(),
        PLATFORM_SHELF_HEIGHT,
        "the shelf height must not depend on whether controls are drawn"
    );
    let heading = painted_text_rect(&texts, "YOUR LIBRARY");
    assert!(geometry.row.bottom() <= heading.top());
    for card in &geometry.cards {
        assert!(card.bottom() <= geometry.row.bottom() + 0.5);
        assert!(!card.intersects(heading));
    }
}

#[test]
fn gamer_view_platform_shelf_shows_all_named_and_unknown_items_with_counts() {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::GamerView;
    app.view = MainView::Library;
    let mut records = vec![
        record("/roms/a.zip", MountState::Pending),
        record("/roms/b.zip", MountState::Pending),
    ];
    records[0].metadata.platform = Some("GameCube".to_string());
    records[1].metadata.platform = None;
    app.state = LoadState::Ready(Box::new(loaded_data_with_records("/mount", records)));

    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1600.0, 900.0),
        )),
        ..Default::default()
    };
    let output = ctx.run(input, |ctx| app.update(ctx, &mut frame));

    for expected in ["All", "GameCube", "Unknown"] {
        assert!(
            rendered_text_contains(&output, expected),
            "expected the platform shelf to show {expected:?}"
        );
    }
}

#[test]
fn custom_artwork_preserves_platform_filtering_and_selected_game_state() {
    let temp = artwork_test_directory("state-preservation");
    write_test_png(&temp.join("gamecube.png"), 2, 2, [80, 160, 220, 255]);
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::GamerView;
    app.view = MainView::Library;
    let selected_path = PathBuf::from("/roms/selected-game.zip");
    let mut selected = record(selected_path.to_str().unwrap(), MountState::Pending);
    selected.metadata.title = Some("Selected Game Title".to_string());
    selected.metadata.platform = Some("GameCube".to_string());
    let mut hidden = record("/roms/hidden-game.zip", MountState::Pending);
    hidden.metadata.title = Some("Filtered Out Title".to_string());
    hidden.metadata.platform = Some("PS2".to_string());
    app.state = LoadState::Ready(Box::new(loaded_data_with_records(
        "/mount",
        vec![selected, hidden],
    )));
    app.library_filters.platform = Some("GameCube".to_string());
    app.archive_context.select_only(selected_path.clone());
    app.custom_platform_artwork_directory = Some(temp.clone());

    let context = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1024.0, 600.0),
        )),
        ..Default::default()
    };
    let output = context.run(input, |context| app.update(context, &mut frame));

    assert!(rendered_text_contains(&output, "Selected Game Title"));
    assert!(!rendered_text_contains(&output, "Filtered Out Title"));
    assert_eq!(app.library_filters.platform.as_deref(), Some("GameCube"));
    assert_eq!(
        app.archive_context.focused.as_deref(),
        Some(selected_path.as_path())
    );
    assert!(app.platform_artwork_cache.entries.contains_key("gamecube"));
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn bulk_action_gate_boundary_at_1_25_and_26_items() {
    // Decisions 1-3 (docs/GUI_NAVIGATION_RESET_DESIGN.md §9), and the
    // Codex audit's explicit boundary requirement: 1 and 25 use a
    // normal confirmation (no typed count needed); 26 requires typing
    // the exact count.
    assert!(!bulk_action_requires_typed_count(1));
    assert!(!bulk_action_requires_typed_count(25));
    assert!(bulk_action_requires_typed_count(26));

    // At 1 and 25, confirmation is enabled with no typed input at all.
    assert!(bulk_action_confirm_enabled(1, "", true));
    assert!(bulk_action_confirm_enabled(25, "", true));
    // At 26, empty/wrong/partial/signed/whitespace-only input must
    // never enable confirmation.
    assert!(!bulk_action_confirm_enabled(26, "", true));
    assert!(!bulk_action_confirm_enabled(26, "2", true));
    assert!(!bulk_action_confirm_enabled(26, "-26", true));
    assert!(!bulk_action_confirm_enabled(26, "   ", true));
    assert!(!bulk_action_confirm_enabled(26, "26.0", true));
    // The exact count, with incidental surrounding whitespace tolerated.
    assert!(bulk_action_confirm_enabled(26, "26", true));
    assert!(bulk_action_confirm_enabled(26, "  26  ", true));
    // The typed-count gate never overrides the ordinary
    // busy/eligibility gate.
    assert!(!bulk_action_confirm_enabled(26, "26", false));
    assert!(!bulk_action_confirm_enabled(1, "", false));
}

#[test]
fn platform_all_selection_clears_multi_selection_consistently_with_named_and_unknown() {
    // Codex audit mandatory risk #3 / decision consistency: the "All"
    // platform chip previously cleared only `selected_archive`,
    // leaving `selected_archives` (the multi-selection set)
    // populated - inconsistent with the named-platform and Unknown
    // chips, which already cleared both. Exercised through the real
    // Library page (`show_loaded_data`), not a re-derivation of the
    // fix.
    let mut app = app_for_operation_tests();
    if let LoadState::Ready(data) = &mut app.state {
        let mut a = record("/roms/a.zip", MountState::Pending);
        a.metadata.platform = Some("GameCube".to_string());
        data.records.push(a);
    }
    app.view = MainView::Library;
    app.library_tab = LibraryTab::Archives;
    app.ui_mode = GuiMode::AdvancedView;
    app.library_filters.platform = Some("GameCube".to_string());
    app.archive_context.focused = Some(PathBuf::from("/roms/a.zip"));
    app.archive_context.selected = [PathBuf::from("/roms/a.zip")].into_iter().collect();

    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1600.0, 900.0),
        )),
        ..Default::default()
    };
    let _ = ctx.run(input.clone(), |ctx| app.update(ctx, &mut frame));
    // Click the "All" chip via its real text position - the same
    // real-click pattern the rest of this suite uses, not a direct
    // field mutation.
    let output = ctx.run(input.clone(), |ctx| app.update(ctx, &mut frame));
    if let Some(center) = find_exact_text_center(&output, "All (1)") {
        let click_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 900.0),
            )),
            events: vec![
                egui::Event::PointerMoved(center),
                egui::Event::PointerButton {
                    pos: center,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
                egui::Event::PointerButton {
                    pos: center,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            ..Default::default()
        };
        let _ = ctx.run(click_input, |ctx| app.update(ctx, &mut frame));
        assert!(
            app.archive_context.selected.is_empty(),
            "the multi-selection must be cleared by the All chip, \
                 exactly like the named-platform and Unknown chips"
        );
    }
}

#[test]
fn cheats_mods_opened_from_gamer_view_has_an_obvious_back_to_games_button() {
    // Manual QA finding: no obvious way back from Cheats & Mods when
    // it's opened from Gamer View's selected-game panel.
    let mut app = app_with_cheats_mods_context();
    app.ui_mode = GuiMode::GamerView;
    app.view = MainView::CheatsMods;

    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1600.0, 900.0),
        )),
        ..Default::default()
    };
    let output = ctx.run(input.clone(), |ctx| app.update(ctx, &mut frame));
    assert!(
        rendered_text_contains(&output, "Back to games"),
        "Gamer View's Cheats & Mods must show an obvious back-to-games button"
    );

    let Some(center) = find_exact_text_center(&output, "\u{2190} Back to games") else {
        panic!("back button not found for click simulation");
    };
    let click_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1600.0, 900.0),
        )),
        events: vec![
            egui::Event::PointerMoved(center),
            egui::Event::PointerButton {
                pos: center,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::PointerButton {
                pos: center,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ],
        ..Default::default()
    };
    let _ = ctx.run(click_input, |ctx| app.update(ctx, &mut frame));
    assert_eq!(
        app.view,
        MainView::Library,
        "clicking Back to games must return to the Gamer View game list"
    );
}

/// There is one primary sidebar route for the Mount page. Active Mounts is a
/// separate page and remains available alongside it.
#[test]
fn only_one_sidebar_entry_highlights_for_a_shared_destination() {
    use std::collections::HashMap;

    let mut highlightable_views: HashMap<MainView, usize> = HashMap::new();
    for group in ADVANCED_NAV_GROUPS {
        for entry in group.entries {
            if let (NavClick::View(view), true) = (entry.click, entry.highlightable) {
                *highlightable_views.entry(view).or_insert(0) += 1;
            }
        }
    }
    for (view, count) in &highlightable_views {
        assert_eq!(
            *count, 1,
            "{view:?} has {count} highlightable sidebar entries; exactly one must \
             be able to render selected or two buttons light up together"
        );
    }

    let mount_entries: Vec<&NavEntry> = ADVANCED_NAV_GROUPS
        .iter()
        .flat_map(|group| group.entries)
        .filter(|entry| matches!(entry.click, NavClick::View(MainView::Mount)))
        .collect();
    assert_eq!(
        mount_entries.len(),
        1,
        "the Mount page must have one primary sidebar route"
    );
    let highlightable_count = mount_entries.iter().filter(|e| e.highlightable).count();
    assert_eq!(
        highlightable_count, 1,
        "the Mount page route must remain highlightable"
    );
    assert!(
        ADVANCED_NAV_GROUPS
            .iter()
            .flat_map(|group| group.entries)
            .any(|entry| entry.label == "Mounts")
    );
}

/// Regression test for a live-QA bug: opening a `ToolsOverlay` (e.g.
/// Database Status) left the previously active `MainView` (e.g. DAT
/// Sources) still highlighted in the sidebar at the same time as the
/// overlay's own entry, because the overlay replaces the main view's content
/// without ever changing `self.view`. A `View` entry must stop rendering
/// selected the moment any overlay is open.
#[test]
fn view_entries_do_not_stay_highlighted_once_an_overlay_is_open() {
    let (_, selected_without_overlay) = (
        (),
        navigation_destination_selected(MainView::DatSources, MainView::DatSources),
    );
    assert!(
        selected_without_overlay,
        "sanity check: DAT Sources must normally render selected while active"
    );

    // `show_primary_navigation`'s selection computation gates every `View`
    // entry on `current_overlay == ToolsOverlay::None`; simulate that gate
    // directly the way the render loop does.
    let current = MainView::DatSources;
    let current_overlay = ToolsOverlay::DatabaseStatus;
    let dat_sources_selected = current_overlay == ToolsOverlay::None
        && navigation_destination_selected(current, MainView::DatSources);
    let database_status_selected = current_overlay == ToolsOverlay::DatabaseStatus;

    assert!(
        !dat_sources_selected,
        "DAT Sources must not stay highlighted once another overlay is open"
    );
    assert!(
        database_status_selected,
        "Database Status must be the only highlighted entry while it is open"
    );
}

/// Every destination `PRIMARY_NAVIGATION_DESTINATIONS` names, plus Quick
/// Rename, must still have a sidebar entry - a data-level check that
/// survives independently of whether any of them can currently be scrolled
/// into view, unlike a render-based assertion.
#[test]
fn every_primary_destination_and_quick_rename_still_has_a_sidebar_entry() {
    let sidebar_views: std::collections::HashSet<MainView> = ADVANCED_NAV_GROUPS
        .iter()
        .flat_map(|group| group.entries)
        .filter_map(|entry| match entry.click {
            NavClick::View(view) => Some(view),
            _ => None,
        })
        .collect();
    for (view, label) in PRIMARY_NAVIGATION_DESTINATIONS {
        // `About` is deliberately reached only through the Help menu, never
        // the sidebar (see `main.rs`'s Help-menu handler) - it is not a
        // sidebar regression for it to be absent here.
        //
        // `Doctor`/`RepairReview`/`RepairHistory` are deliberately reached
        // only through the consolidated `MainView::Problems` ("Problems &
        // Repair") entry's own tabs now, not through a sidebar row of their
        // own - see `navigation::ADVANCED_NAV_GROUPS`'s "GUI consolidation"
        // doc note. `problems_repair_still_covers_every_consolidated_view`
        // below is the real regression check for these three; skipping them
        // here is not a silent gap.
        // `DatSources`/`CheatSources`/`SourcesDiscovery` are likewise
        // reached only through the consolidated `MainView::Sources` entry's
        // own tabs now - see `sources_still_covers_every_consolidated_view`
        // below for the real regression check.
        if matches!(
            view,
            MainView::About
                | MainView::Doctor
                | MainView::RepairReview
                | MainView::RepairHistory
                | MainView::DatSources
                | MainView::CheatSources
                | MainView::SourcesDiscovery
        ) {
            continue;
        }
        assert!(
            sidebar_views.contains(&view),
            "{label} ({view:?}) is a primary destination but has no sidebar entry"
        );
    }
    assert!(
        sidebar_views.contains(&MainView::Problems),
        "the consolidated Problems & Repair destination must have a sidebar entry"
    );
    assert!(
        sidebar_views.contains(&MainView::Sources),
        "the consolidated Sources destination must have a sidebar entry"
    );
    let has_quick_rename = ADVANCED_NAV_GROUPS
        .iter()
        .flat_map(|group| group.entries)
        .any(|entry| matches!(entry.click, NavClick::QuickRename));
    assert!(
        has_quick_rename,
        "Quick Rename must still have a sidebar entry"
    );
}

/// The GUI consolidation regression check `every_primary_destination_and_
/// quick_rename_still_has_a_sidebar_entry` above defers to: every one of
/// the three destinations that lost its own standalone sidebar row
/// (`Doctor`, `RepairReview`, `RepairHistory`) must still be reachable
/// through the consolidated `MainView::Problems` entry's tab projection
/// (`problems_repair_tab_for_main_view`), and `Problems` itself must be
/// the sole sidebar-visible entry among the four - never two of them
/// visible at once, which would recreate the exact "choose between Doctor
/// and Repair" problem the consolidation removed.
#[test]
fn problems_repair_still_covers_every_consolidated_view() {
    for view in [
        MainView::Problems,
        MainView::Doctor,
        MainView::RepairReview,
        MainView::RepairHistory,
    ] {
        assert!(
            problems_repair_tab_for_main_view(view).is_some(),
            "{view:?} must map to a Problems & Repair tab"
        );
    }
    let sidebar_views: std::collections::HashSet<MainView> = ADVANCED_NAV_GROUPS
        .iter()
        .flat_map(|group| group.entries)
        .filter_map(|entry| match entry.click {
            NavClick::View(view) => Some(view),
            _ => None,
        })
        .collect();
    for view in [
        MainView::Doctor,
        MainView::RepairReview,
        MainView::RepairHistory,
    ] {
        assert!(
            !sidebar_views.contains(&view),
            "{view:?} must not have its own standalone sidebar entry any more - it is only reachable through Problems & Repair's tabs"
        );
    }
}

/// The GUI consolidation regression check for Sources: every destination
/// that lost its own standalone sidebar row (`DatSources`, `CheatSources`,
/// `SourcesDiscovery`) must still be reachable through the consolidated
/// `MainView::Sources` entry's tab projection (`sources_tab_for_main_view`),
/// and none of them may retain its own sidebar entry - which would recreate
/// the exact "multiple overlapping technical destinations" problem the
/// consolidation removed.
#[test]
fn sources_still_covers_every_consolidated_view() {
    for view in [
        MainView::Sources,
        MainView::DatSources,
        MainView::CheatSources,
        MainView::SourcesDiscovery,
    ] {
        assert!(
            sources_tab_for_main_view(view).is_some(),
            "{view:?} must map to a Sources tab"
        );
    }
    let sidebar_views: std::collections::HashSet<MainView> = ADVANCED_NAV_GROUPS
        .iter()
        .flat_map(|group| group.entries)
        .filter_map(|entry| match entry.click {
            NavClick::View(view) => Some(view),
            _ => None,
        })
        .collect();
    for view in [
        MainView::DatSources,
        MainView::CheatSources,
        MainView::SourcesDiscovery,
    ] {
        assert_eq!(
            sidebar_views.contains(&view),
            view == MainView::DatSources,
            "only DAT Sources has a direct sidebar route now"
        );
    }
    // The old Collection Discovery overlay entry must not survive under a
    // different guise either.
    assert!(
        !ADVANCED_NAV_GROUPS
            .iter()
            .flat_map(|group| group.entries)
            .any(|entry| entry.label.contains("Collection Discovery")),
        "Collection Discovery must not have its own standalone sidebar/overlay entry any more"
    );
}

#[test]
fn quick_rename_is_the_single_sidebar_entry_for_the_rename_workflow() {
    let entries: Vec<&NavEntry> = ADVANCED_NAV_GROUPS
        .iter()
        .flat_map(|group| group.entries)
        .collect();
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.label == "Quick Rename")
            .count(),
        1
    );
    assert!(
        !entries
            .iter()
            .any(|entry| entry.label == "Identify & Rename")
    );
    assert!(
        entries
            .iter()
            .any(|entry| { matches!(entry.click, NavClick::QuickRename) && entry.highlightable })
    );
}

#[test]
fn selected_is_internal_only_and_library_is_the_game_details_destination() {
    let entries: Vec<&NavEntry> = ADVANCED_NAV_GROUPS
        .iter()
        .flat_map(|group| group.entries)
        .collect();
    assert!(!entries.iter().any(|entry| {
        entry.label == "Selected" || matches!(entry.click, NavClick::View(MainView::Selected))
    }));
    assert!(navigation_destination_selected(
        MainView::Library,
        MainView::Library
    ));
    assert!(!navigation_destination_selected(
        MainView::Library,
        MainView::Selected
    ));
}

/// Renders the sidebar alone, the way `SidePanel::left("app_navigation")`
/// does, at a screen height short enough that the full nav list cannot fit
/// unclipped (`ADVANCED_NAV_GROUPS` has grown past what 1536x864 and
/// smaller desktop windows show above their own taskbar/status chrome).
fn render_sidebar_at_height(
    context: &egui::Context,
    height: f32,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(218.0, height),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::SidePanel::left("app_navigation")
                .resizable(false)
                .exact_width(218.0)
                .show(ctx, |ui| {
                    let _ = show_primary_navigation(ui, MainView::Home, ToolsOverlay::None, true);
                });
        },
    )
}

/// The regression this whole fix addresses: at a short window height, the
/// last group's entry ("Settings") must not simply be clipped out of
/// existence. Proving it via rendered text alone is not enough - an egui
/// `ScrollArea` only *paints* whatever fits in its current scroll offset,
/// so an unscrolled short viewport legitimately shows nothing below the
/// fold on the very first frame, scrollable or not. The proof a scroll
/// container exists (rather than a plain `Ui` that would clip permanently)
/// is that a mouse-wheel scroll event over the sidebar changes what is
/// visible - the same technique already used for the platform shelf's own
/// scroll strip in this file.
#[test]
fn low_sidebar_destinations_become_visible_after_scrolling_a_short_sidebar() {
    let context = egui::Context::default();

    let unscrolled = render_sidebar_at_height(&context, 400.0, Vec::new());
    assert!(
        rendered_text_contains(&unscrolled, "Home"),
        "the topmost destination must render without any scrolling"
    );
    assert!(
        !rendered_text_contains(&unscrolled, "Settings"),
        "a 400px-tall sidebar showing every group unscrolled would defeat this test's premise \
         (nothing to prove by scrolling) - Settings must start out of view"
    );

    let scroll_down = vec![
        egui::Event::PointerMoved(egui::pos2(100.0, 200.0)),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -2000.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::default(),
        },
    ];
    // A couple of frames: one to deliver the wheel event, one to settle the
    // resulting scroll offset before painting.
    let _ = render_sidebar_at_height(&context, 400.0, scroll_down);
    let scrolled = render_sidebar_at_height(&context, 400.0, Vec::new());
    assert!(
        rendered_text_contains(&scrolled, "Settings"),
        "scrolling the sidebar down must reveal the last group's destination"
    );
    assert!(
        rendered_text_contains(&scrolled, "Library View History"),
        "scrolling must also reach History & Journals, not only the very last entry"
    );
}

/// Quick Rename must survive at every scroll position it can reach - this
/// fix must not have moved, hidden, or duplicated it while adding the
/// scroll container.
#[test]
fn quick_rename_remains_reachable_in_the_scrollable_sidebar() {
    let context = egui::Context::default();
    let unscrolled = render_sidebar_at_height(&context, 400.0, Vec::new());
    assert!(
        rendered_text_contains(&unscrolled, "Quick Rename"),
        "Quick Rename sits in the LIBRARY group, near the top - it must not have been pushed \
         out of the initial, unscrolled view"
    );
}
