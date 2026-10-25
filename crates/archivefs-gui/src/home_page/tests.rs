use super::*;
use archivefs_core::{SetupDiagnostic, SetupDiagnosticStatus};

fn passing_check(name: &str) -> SetupDiagnostic {
    SetupDiagnostic {
        name: name.to_string(),
        status: SetupDiagnosticStatus::Ready,
        detail: String::new(),
        why_it_matters: String::new(),
        next_step: String::new(),
    }
}

fn error_check(name: &str) -> SetupDiagnostic {
    SetupDiagnostic {
        name: name.to_string(),
        status: SetupDiagnosticStatus::Error,
        detail: "something is broken".to_string(),
        why_it_matters: String::new(),
        next_step: String::new(),
    }
}

fn warning_check(name: &str) -> SetupDiagnostic {
    SetupDiagnostic {
        name: name.to_string(),
        status: SetupDiagnosticStatus::Warning,
        detail: "something looks off".to_string(),
        why_it_matters: String::new(),
        next_step: String::new(),
    }
}

/// The Home "Set up emulators" badge now reflects `ArchiveFsApp::doctor_scan`,
/// not the background `SetupDiagnostics` report. These helpers keep phrasing
/// the fixtures in terms of a Doctor check list and translate it, so the
/// existing tests still read naturally.
fn summary_from_checks(checks: &[SetupDiagnostic]) -> SetupCheckSummary {
    if checks.is_empty() {
        return SetupCheckSummary::NoChecksRun;
    }
    let errors = checks
        .iter()
        .filter(|c| c.status == SetupDiagnosticStatus::Error)
        .count();
    let warnings = checks
        .iter()
        .filter(|c| c.status == SetupDiagnosticStatus::Warning)
        .count();
    if errors > 0 {
        SetupCheckSummary::NeedsAttention(errors)
    } else if warnings > 0 {
        SetupCheckSummary::Warnings(warnings)
    } else {
        SetupCheckSummary::Healthy
    }
}

fn fresh_install_inputs() -> HomeInputs {
    HomeInputs {
        source_folder_count: 0,
        has_database: false,
        // A genuine fresh install: Doctor has never run this session.
        setup_check: SetupCheckSummary::NeverRun,
        config_missing: true,
        first_run: true,
        cheat_sources_enabled_count: None,
        dat_sources_registered_count: None,
        romm_state_label: None,
    }
}

fn established_inputs(checks: &[SetupDiagnostic]) -> HomeInputs {
    HomeInputs {
        source_folder_count: 3,
        has_database: true,
        setup_check: summary_from_checks(checks),
        config_missing: false,
        first_run: false,
        cheat_sources_enabled_count: Some(5),
        dat_sources_registered_count: Some(2),
        romm_state_label: Some(RommReadinessLabel::Ready("Ready")),
    }
}

fn rendered_text_contains(output: &egui::FullOutput, needle: &str) -> bool {
    fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
        match shape {
            egui::Shape::Text(text_shape) => text_shape.galley.text().contains(needle),
            egui::Shape::Vec(nested) => nested.iter().any(|s| shape_contains(s, needle)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|clipped| shape_contains(&clipped.shape, needle))
}

fn exact_text_center(output: &egui::FullOutput, needle: &str) -> Option<egui::Pos2> {
    fn find(shape: &egui::Shape, needle: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == needle => {
                Some(text.pos + text.galley.size() / 2.0)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, needle)),
            _ => None,
        }
    }

    output
        .shapes
        .iter()
        .find_map(|clipped| find(&clipped.shape, needle))
}

fn click_at(pos: egui::Pos2) -> Vec<egui::Event> {
    vec![
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
    ]
}

fn render(view: &HomeView, width: f32) -> (egui::FullOutput, Option<HomeCard>) {
    let ctx = egui::Context::default();
    let mut clicked = None;
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 2000.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                clicked = show_home_page(ui, view);
            });
        },
    );
    (output, clicked)
}

// --- Fresh / empty HOME rendering ---

#[test]
fn fresh_install_shows_the_welcome_banner_and_not_configured_cards() {
    let view = build_home_view(&fresh_install_inputs());
    assert_eq!(view.banner, HomeBanner::FreshInstall);
    for card in &view.cards {
        if let Some(readiness) = &card.readiness {
            assert!(
                !matches!(readiness, CardReadiness::Ready(_)),
                "{:?} claimed Ready on a fresh install: {readiness:?}",
                card.card
            );
        }
    }
    let (output, _) = render(&view, 1100.0);
    assert!(rendered_text_contains(&output, "Welcome to EmuWiz"));
    assert!(rendered_text_contains(&output, "No source folders yet"));
    assert_eq!(view.cards[0].card, HomeCard::BuildLibrary);
    assert_eq!(view.cards[0].title, "Add your games");
    assert_eq!(view.cards[0].action_label, "Add game folder");
}

// --- Established configuration rendering ---

#[test]
fn established_install_shows_no_banner_and_ready_cards() {
    let checks = [passing_check("config file"), passing_check("mount root")];
    let view = build_home_view(&established_inputs(&checks));
    assert_eq!(view.banner, HomeBanner::None);
    let library = view
        .cards
        .iter()
        .find(|c| c.card == HomeCard::BuildLibrary)
        .unwrap();
    assert!(matches!(library.readiness, Some(CardReadiness::Ready(_))));
    let (output, _) = render(&view, 1100.0);
    assert!(rendered_text_contains(
        &output,
        "3 source folders configured"
    ));
    assert!(rendered_text_contains(&output, "All checks passed"));
}

#[test]
fn healthy_home_hides_the_attention_section() {
    let view = build_home_view(&established_inputs(&[passing_check("config file")]));
    assert!(view.attention.is_empty());
    let (output, _) = render(&view, 1100.0);
    assert!(!rendered_text_contains(&output, "WHAT NEEDS ATTENTION"));
}

#[test]
fn current_setup_problem_is_promoted_to_attention_with_a_real_action() {
    let view = build_home_view(&established_inputs(&[error_check("emulator")]));
    assert_eq!(view.attention.len(), 1);
    assert_eq!(view.attention[0].action, HomeCard::CheckSetup);
    let (output, _) = render(&view, 1100.0);
    assert!(rendered_text_contains(&output, "WHAT NEEDS ATTENTION"));
    assert!(rendered_text_contains(
        &output,
        "Emulator setup needs attention"
    ));
}

#[test]
fn attention_is_rendered_before_primary_tasks_when_active() {
    let view = build_home_view(&established_inputs(&[error_check("emulator")]));
    let (output, _) = render(&view, 1500.0);
    let attention_y = exact_text_center(&output, "WHAT NEEDS ATTENTION")
        .expect("attention heading should render")
        .y;
    let primary_y = exact_text_center(&output, "PRIMARY TASKS")
        .expect("primary heading should render")
        .y;
    assert!(attention_y < primary_y);
}

#[test]
fn home_grid_uses_deterministic_three_two_one_breakpoints() {
    assert_eq!(home_grid_columns(1500.0, 8), 3);
    assert_eq!(home_grid_columns(1100.0, 3), 3);
    assert_eq!(home_grid_columns(1099.0, 8), 2);
    assert_eq!(home_grid_columns(800.0, 8), 2);
    assert_eq!(home_grid_columns(699.0, 8), 1);
    assert_eq!(home_grid_columns(650.0, 8), 1);
    assert_eq!(home_grid_columns(1500.0, 0), 0);
}

#[test]
fn eight_secondary_cards_form_a_left_aligned_three_three_two_grid() {
    let columns = home_grid_columns(1500.0, 8);
    let rows: Vec<usize> = (0..8)
        .collect::<Vec<_>>()
        .chunks(columns)
        .map(<[_]>::len)
        .collect();
    assert_eq!(rows, [3, 3, 2]);
    assert!(rows.iter().all(|&width| width > 0 && width <= columns));
}

#[test]
fn grid_geometry_keeps_final_two_cards_at_the_normal_column_width() {
    let view = build_home_view(&established_inputs(&[]));
    let cards: Vec<&HomeCardView> = view
        .cards
        .iter()
        .filter(|card| card.tier == HomeCardTier::Secondary)
        .collect();
    let geometry = home_grid_geometry(1_500.0);
    assert_eq!(geometry.columns, 3);
    assert_eq!(cards.len(), 8);
    let expected = (1_500.0 - HOME_GRID_GAP * 2.0) / 3.0;
    assert!((geometry.column_width - expected).abs() < f32::EPSILON);
    assert_eq!(
        cards
            .chunks(geometry.columns)
            .map(<[_]>::len)
            .collect::<Vec<_>>(),
        [3, 3, 2]
    );
}

#[test]
fn every_row_uses_one_shared_height_even_with_badges_details_and_two_actions() {
    let view = build_home_view(&established_inputs(&[]));
    let primary: Vec<&HomeCardView> = view
        .cards
        .iter()
        .filter(|card| card.tier == HomeCardTier::Primary)
        .collect();
    let secondary: Vec<&HomeCardView> = view
        .cards
        .iter()
        .filter(|card| card.tier == HomeCardTier::Secondary)
        .collect();
    assert_eq!(home_card_row_height(&primary), HOME_PRIMARY_CARD_HEIGHT);
    assert_eq!(home_card_row_height(&secondary), HOME_SECONDARY_CARD_HEIGHT);
    assert!(secondary.iter().any(|card| card.secondary.is_some()));
    assert!(primary.iter().any(|card| card.technical_detail.is_some()));
    assert!(home_grid_geometry(0.0).column_width >= 1.0);
    assert!(home_grid_geometry(650.0).column_width > 0.0);
}

#[test]
fn wide_grid_reuses_exact_column_rects_across_rows() {
    let geometry = home_grid_geometry(1_500.0);
    let row_one = (0..3)
        .map(|column| home_grid_cell_rect(100.0, 20.0, geometry, column, 218.0))
        .collect::<Vec<_>>();
    let row_two = (0..3)
        .map(|column| home_grid_cell_rect(100.0, 256.0, geometry, column, 188.0))
        .collect::<Vec<_>>();
    for column in 0..3 {
        assert_eq!(row_one[column].left(), row_two[column].left());
        assert_eq!(row_one[column].width(), row_two[column].width());
    }
    assert_eq!(row_one[0].right() + geometry.gap, row_one[1].left());
    assert_eq!(row_one[1].right() + geometry.gap, row_one[2].left());
    assert!(row_one[2].right() <= 100.0 + geometry.content_width + 0.01);
}

#[test]
fn final_two_card_row_uses_columns_one_and_two_and_leaves_three_empty() {
    let geometry = home_grid_geometry(1_500.0);
    let first = home_grid_cell_rect(0.0, 0.0, geometry, 0, 188.0);
    let second = home_grid_cell_rect(0.0, 0.0, geometry, 1, 188.0);
    let empty_third = home_grid_cell_rect(0.0, 0.0, geometry, 2, 188.0);
    assert_eq!(first.left(), 0.0);
    assert_eq!(second.left(), first.right() + geometry.gap);
    assert_eq!(empty_third.left(), second.right() + geometry.gap);
    assert!(empty_third.right() <= geometry.content_width + 0.01);
}

#[test]
fn medium_and_narrow_cells_are_exact_peers_without_shrink_wrapping() {
    for width in [1_000.0, 650.0] {
        let geometry = home_grid_geometry(width);
        let cells = (0..geometry.columns)
            .map(|column| home_grid_cell_rect(12.0, 30.0, geometry, column, 188.0))
            .collect::<Vec<_>>();
        assert_eq!(cells.len(), geometry.columns);
        for cell in &cells {
            assert_eq!(cell.width(), geometry.column_width);
        }
        if geometry.columns > 1 {
            assert_eq!(cells[0].right() + geometry.gap, cells[1].left());
        }
    }
}

#[test]
fn first_run_hero_uses_onboarding_language_without_inventing_counts() {
    let view = build_home_view(&fresh_install_inputs());
    let (output, _) = render(&view, 1100.0);
    assert!(rendered_text_contains(&output, "Add your game library"));
    assert!(rendered_text_contains(&output, "Add game folder"));
    assert!(!rendered_text_contains(&output, "0 games"));
}

// --- Disappeared-config warning behaviour ---

#[test]
fn config_disappeared_after_being_confirmed_shows_the_warning_banner_not_the_welcome_one() {
    let mut inputs = fresh_install_inputs();
    inputs.first_run = false; // previously confirmed, now gone
    let view = build_home_view(&inputs);
    assert_eq!(view.banner, HomeBanner::ConfigDisappeared);
    let (output, _) = render(&view, 1100.0);
    assert!(rendered_text_contains(
        &output,
        "EmuWiz settings could not be found"
    ));
    assert!(!rendered_text_contains(&output, "Doctor"));
    assert!(!rendered_text_contains(&output, "Welcome to EmuWiz"));
    // The banner's own direct next action: a beginner told their settings
    // could not be found must not be left to independently rediscover
    // Problems & Repair.
    assert!(rendered_text_contains(&output, "Check the problem"));
}

#[test]
fn only_the_config_disappeared_banner_shows_its_check_the_problem_action() {
    // Fresh install: expected/cheerful, and has nothing to "check" yet -
    // the action button must not appear there.
    let fresh_view = build_home_view(&fresh_install_inputs());
    assert_eq!(fresh_view.banner, HomeBanner::FreshInstall);
    let (fresh_output, fresh_clicked) = render(&fresh_view, 1100.0);
    assert!(!rendered_text_contains(&fresh_output, "Check the problem"));
    assert_eq!(
        fresh_clicked, None,
        "an unclicked render must report no action"
    );

    // Normal, fully configured Home: no banner at all, so definitely no
    // erroneous or duplicate action button.
    let checks = [passing_check("config file")];
    let established_view = build_home_view(&established_inputs(&checks));
    assert_eq!(established_view.banner, HomeBanner::None);
    let (established_output, established_clicked) = render(&established_view, 1100.0);
    assert!(!rendered_text_contains(
        &established_output,
        "Check the problem"
    ));
    assert_eq!(established_clicked, None);

    // Exactly one "Check the problem" affordance renders for the disappeared
    // state - not a duplicate alongside some other label for the same
    // action.
    let mut inputs = fresh_install_inputs();
    inputs.first_run = false;
    let disappeared_view = build_home_view(&inputs);
    let (disappeared_output, _) = render(&disappeared_view, 1100.0);
    let occurrences = disappeared_output
        .shapes
        .iter()
        .filter(|clipped| {
            fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
                match shape {
                    egui::Shape::Text(text_shape) => text_shape.galley.text() == needle,
                    egui::Shape::Vec(nested) => nested.iter().any(|s| shape_contains(s, needle)),
                    _ => false,
                }
            }
            shape_contains(&clipped.shape, "Check the problem")
        })
        .count();
    assert_eq!(
        occurrences, 1,
        "exactly one \"Check the problem\" label must render, found {occurrences}"
    );
}

#[test]
fn clicking_the_config_disappeared_action_emits_check_problems() {
    let mut inputs = fresh_install_inputs();
    inputs.first_run = false;
    let view = build_home_view(&inputs);
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 2000.0));

    let first = ctx.run(
        egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_home_page(ui, &view);
            });
        },
    );
    let button_pos = exact_text_center(&first, "Check the problem")
        .expect("the config-disappeared recovery action should render");

    let mut clicked = None;
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(screen),
            events: click_at(button_pos),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                clicked = show_home_page(ui, &view);
            });
        },
    );

    assert_eq!(clicked, Some(HomeCard::CheckProblems));
}

// --- Purity: identical inputs produce byte-identical (structurally equal) views ---

#[test]
fn the_same_inputs_always_produce_the_same_view() {
    let checks = [passing_check("config file")];
    let inputs = established_inputs(&checks);
    let first = build_home_view(&inputs);
    let second = build_home_view(&inputs);
    assert_eq!(first, second);
}

// --- Every card navigates to the correct destination ---

#[test]
fn every_primary_card_action_reports_its_own_card() {
    let checks = [passing_check("config file")];
    let view = build_home_view(&established_inputs(&checks));
    let expected = [
        (HomeCard::BrowseGames, "Open Library"),
        (HomeCard::CanonicalOrganisation, "Open Organise"),
        (HomeCard::CheckSetup, "Open Emulator Setup"),
        (HomeCard::CheatsAndMods, "Open Cheats & Mods"),
        (HomeCard::DatSources, "Verify games"),
        (HomeCard::DuplicateReview, "Open duplicate finder"),
        (HomeCard::ConvertDiscs, "Open disc conversion"),
        (HomeCard::Settings, "Open Settings"),
        (HomeCard::BuildLibrary, "Open Sources"),
        (HomeCard::QuickRename, "Choose a library"),
        (HomeCard::RomM, "Open RomM"),
    ];
    assert_eq!(view.cards.len(), expected.len());
    for (card, (expected_card, expected_label)) in view.cards.iter().zip(expected.iter()) {
        assert_eq!(card.card, *expected_card);
        assert_eq!(card.action_label, *expected_label);
    }
}

#[test]
fn cheat_sources_secondary_link_is_present_and_distinct_from_the_primary_action() {
    let checks = [passing_check("config file")];
    let view = build_home_view(&established_inputs(&checks));
    let cheats = view
        .cards
        .iter()
        .find(|c| c.card == HomeCard::CheatsAndMods)
        .unwrap();
    assert_eq!(
        cheats.secondary,
        Some((HomeCard::CheatSources, "Manage Cheat Sources"))
    );
}

// --- Unavailable tasks use honest wording ---

#[test]
fn a_romm_error_state_is_reported_as_unavailable_never_as_ready_or_not_configured() {
    let checks = [passing_check("config file")];
    let mut inputs = established_inputs(&checks);
    inputs.romm_state_label = Some(RommReadinessLabel::Unavailable("Error"));
    let view = build_home_view(&inputs);
    let romm = view
        .cards
        .iter()
        .find(|c| c.card == HomeCard::RomM)
        .unwrap();
    assert!(matches!(
        romm.readiness,
        Some(CardReadiness::Unavailable(_))
    ));
    assert_ne!(romm.readiness.as_ref().unwrap().label(), "Ready");
}

#[test]
fn setup_checks_with_errors_are_reported_as_unavailable() {
    let checks = [error_check("mount root"), warning_check("ratarmount")];
    let view = build_home_view(&established_inputs(&checks));
    let setup = view
        .cards
        .iter()
        .find(|c| c.card == HomeCard::CheckSetup)
        .unwrap();
    assert!(matches!(
        setup.readiness,
        Some(CardReadiness::Unavailable(_))
    ));
    assert_eq!(
        setup.readiness.as_ref().unwrap().label(),
        "1 check needs attention"
    );
}

#[test]
fn lazily_loaded_pages_not_yet_visited_omit_their_status_badge() {
    let view = build_home_view(&fresh_install_inputs());
    for card_kind in [
        HomeCard::CheatsAndMods,
        HomeCard::DatSources,
        HomeCard::RomM,
    ] {
        let card = view.cards.iter().find(|c| c.card == card_kind).unwrap();
        assert_eq!(card.readiness, None);
    }
}

// --- No filesystem writes or network requests merely from rendering ---
// `build_home_view` and `show_home_page` take no I/O-capable handle (no
// `Path`, no client, no thread spawn) - the type signatures themselves are
// the guarantee. This test only pins the observable behaviour: rendering
// twice from identical inputs never changes the view, i.e. rendering has no
// side effect that could feed back into a later `build_home_view` call.
#[test]
fn rendering_does_not_mutate_anything_observable_by_a_later_build() {
    let checks = [passing_check("config file")];
    let inputs = established_inputs(&checks);
    let view = build_home_view(&inputs);
    let _ = render(&view, 1100.0);
    let view_after = build_home_view(&inputs);
    assert_eq!(view, view_after);
}

// --- Minimum-size headless render ---

#[test]
fn renders_without_panicking_at_a_narrow_compact_width() {
    let checks = [passing_check("config file")];
    let view = build_home_view(&established_inputs(&checks));
    let (output, _) = render(&view, 700.0);
    assert!(rendered_text_contains(&output, "Home"));
    assert!(rendered_text_contains(&output, "Build my library"));
}

// --- Keyboard focus order ---

#[test]
fn tabbing_through_the_page_visits_cards_top_to_bottom() {
    let checks = [passing_check("config file")];
    let view = build_home_view(&established_inputs(&checks));
    let ctx = egui::Context::default();
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 3000.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_home_page(ui, &view);
            });
        },
    );
    let mut last_top: Option<f32> = None;
    for _ in 0..view.cards.len() {
        let _output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1100.0, 3000.0),
                )),
                events: vec![egui::Event::Key {
                    key: egui::Key::Tab,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = show_home_page(ui, &view);
                });
            },
        );
        if let Some(id) = ctx.memory(|memory| memory.focused())
            && let Some(response) = ctx.read_response(id)
        {
            let top = response.rect.top();
            if let Some(previous) = last_top {
                assert!(
                    top >= previous,
                    "focus moved upward ({top} came after {previous}); tab order is not top-to-bottom"
                );
            }
            last_top = Some(top);
        }
    }
    assert!(last_top.is_some(), "Tab never focused anything on Home");
}

#[test]
fn rename_and_organise_are_discoverable_from_home() {
    let checks = [passing_check("config file")];
    let view = build_home_view(&established_inputs(&checks));
    let organise = view
        .cards
        .iter()
        .find(|c| c.card == HomeCard::CanonicalOrganisation)
        .unwrap();
    // "Organise by platform" is the primary action; Quick Rename is the
    // simple route and the full evidence-backed workflow remains available.
    assert_eq!(organise.action_label, "Open Organise");
    assert_eq!(
        organise.secondary,
        Some((HomeCard::QuickRename, "Quick Rename"))
    );
    assert_eq!(
        view.cards
            .iter()
            .filter(|c| c.title == "Identify & Rename")
            .count(),
        0,
        "the duplicate Identify & Rename Home card must be absent"
    );
}

#[test]
fn every_home_card_shows_its_icon_alongside_its_title() {
    let checks = [passing_check("config file")];
    let view = build_home_view(&established_inputs(&checks));
    for card in &view.cards {
        assert!(!card.icon.is_empty(), "{} must carry an icon", card.title);
    }
    // The icons are drawn next to the text labels - the labels never become
    // icon-only.
    let (output, _) = render(&view, 1200.0);
    for expected in [
        crate::ui::icons::ORGANISE,
        crate::ui::icons::CLEAN_UP,
        crate::ui::icons::GAMES,
        crate::ui::icons::VERIFY,
        crate::ui::icons::CHECK,
        crate::ui::icons::SETTINGS,
        crate::ui::icons::CHEATS,
        "Organise",
        "My Games",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "expected {expected:?} to be drawn"
        );
    }
}

// --- Beta 1 visual language ---

#[test]
fn primary_home_cards_carry_their_visual_identity() {
    let checks = [passing_check("config file")];
    let view = build_home_view(&established_inputs(&checks));
    let find = |card: HomeCard| view.cards.iter().find(|c| c.card == card).unwrap();
    let my_games = find(HomeCard::BrowseGames);
    assert_eq!(my_games.icon, crate::ui::icons::GAMES);
    assert_eq!(my_games.title, "My Games");
    let organise = find(HomeCard::CanonicalOrganisation);
    assert_eq!(organise.icon, crate::ui::icons::ORGANISE);
    assert_eq!(organise.title, "Organise");
    let check = find(HomeCard::CheckSetup);
    assert_eq!(check.icon, crate::ui::icons::CHECK);
    assert_eq!(check.title, "Set up emulators");
    let cheats = find(HomeCard::CheatsAndMods);
    assert_eq!(cheats.icon, crate::ui::icons::CHEATS);
    let verify = find(HomeCard::DatSources);
    assert_eq!(verify.icon, crate::ui::icons::VERIFY);
    assert_eq!(verify.title, "Verify your games");
    assert_eq!(verify.action_label, "Verify games");
    let settings = find(HomeCard::Settings);
    assert_eq!(settings.icon, crate::ui::icons::SETTINGS);
    assert_eq!(settings.title, "Settings");
}

#[test]
fn all_destinations_remain_and_three_are_primary() {
    let checks = [passing_check("config file")];
    let view = build_home_view(&established_inputs(&checks));
    let mut all: Vec<HomeCard> = view.cards.iter().map(|c| c.card).collect();
    all.sort_by_key(|c| format!("{c:?}"));
    let mut expected: Vec<HomeCard> = vec![
        HomeCard::BuildLibrary,
        HomeCard::BrowseGames,
        HomeCard::ConvertDiscs,
        HomeCard::CheatsAndMods,
        HomeCard::CanonicalOrganisation,
        HomeCard::QuickRename,
        HomeCard::DatSources,
        HomeCard::RomM,
        HomeCard::CheckSetup,
        HomeCard::DuplicateReview,
        HomeCard::Settings,
    ];
    expected.sort_by_key(|c| format!("{c:?}"));
    assert_eq!(all, expected, "all major workflows remain represented");
    assert_eq!(
        view.cards
            .iter()
            .filter(|c| c.tier == HomeCardTier::Primary)
            .count(),
        3
    );
    assert!(
        view.cards
            .iter()
            .filter(|card| card.tier == HomeCardTier::Primary)
            .all(|card| card.accent.is_some())
    );
    assert!(
        view.cards
            .iter()
            .filter(|card| card.tier == HomeCardTier::Secondary)
            .all(|card| card.accent.is_none())
    );
}

#[test]
fn icons_never_replace_text_labels() {
    let checks = [passing_check("config file")];
    let view = build_home_view(&established_inputs(&checks));
    for card in &view.cards {
        assert!(!card.icon.is_empty());
        assert!(
            !card.title.is_empty(),
            "an icon-only card would break this rule"
        );
        assert!(!card.explanation.is_empty());
    }
}

#[test]
fn home_and_page_headers_share_the_same_concept_icons() {
    // The page headers reference the very same constants the Home cards use,
    // so the visual identity cannot drift between Home and a page.
    assert_eq!(crate::ui::icons::GAMES, "■");
    assert_eq!(crate::ui::icons::ORGANISE, "▪");
    assert_eq!(crate::ui::icons::CHECK, "○");
    assert_eq!(crate::ui::icons::CHEATS, "★");
    assert_eq!(crate::ui::icons::VERIFY, "⊞");
    assert_eq!(crate::ui::icons::SETTINGS, "⚙");
    assert_eq!(crate::ui::icons::ARTWORK, "■");
}

#[test]
fn the_primary_home_cards_render_at_compact_width() {
    let checks = [passing_check("config file")];
    let view = build_home_view(&established_inputs(&checks));
    let (output, _) = render(&view, 700.0);
    for expected in [
        crate::ui::icons::GAMES,
        crate::ui::icons::ORGANISE,
        crate::ui::icons::CHECK,
        crate::ui::icons::CHEATS,
        crate::ui::icons::VERIFY,
        crate::ui::icons::SETTINGS,
        "My Games",
        "Organise",
        "Set up emulators",
        "Cheats & Mods",
        "Verify your games",
        "Settings",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "expected {expected:?} to render at compact width"
        );
    }
}

#[test]
fn dat_terminology_is_retained_but_collapsed_by_default() {
    let checks = [passing_check("config file")];
    let view = build_home_view(&established_inputs(&checks));
    let (output, _) = render(&view, 1200.0);
    assert!(rendered_text_contains(&output, "Verify your games"));
    assert!(rendered_text_contains(&output, "Technical details"));
    assert!(!rendered_text_contains(
        &output,
        "commonly called DAT files"
    ));
    assert!(!rendered_text_contains(
        &output,
        "Check your games with DATs"
    ));
}

// --- "Set up emulators" badge tracks the Doctor scan it opens ------------
//
// The card's action opens Problems & Repair -> Diagnostics, which renders
// `ArchiveFsApp::doctor_scan`. `build_home_view` is fed a
// `SetupCheckSummary` derived from that same state (see
// `crate::setup_check_summary`), so the badge and the page can never
// disagree, and "All checks passed" can only appear after a clean completed
// run that actually performed a check.

fn check_setup_readiness(summary: SetupCheckSummary) -> CardReadiness {
    let mut inputs = established_inputs(&[passing_check("config file")]);
    inputs.setup_check = summary;
    build_home_view(&inputs)
        .cards
        .into_iter()
        .find(|c| c.card == HomeCard::CheckSetup)
        .and_then(|c| c.readiness)
        .expect("the Set up emulators card always carries a readiness")
}

#[test]
fn set_up_emulators_shows_not_checked_yet_before_the_first_doctor_run() {
    let readiness = check_setup_readiness(SetupCheckSummary::NeverRun);
    assert!(matches!(readiness, CardReadiness::Unknown(_)));
    assert_eq!(readiness.label(), "Not checked yet");
    assert_ne!(readiness.label(), "All checks passed");
}

#[test]
fn set_up_emulators_is_pending_while_a_doctor_run_is_in_flight() {
    let readiness = check_setup_readiness(SetupCheckSummary::Running);
    assert!(matches!(readiness, CardReadiness::Unknown(_)));
    assert_ne!(readiness.label(), "All checks passed");
}

#[test]
fn set_up_emulators_shows_passed_only_after_a_clean_completed_doctor_run() {
    let readiness = check_setup_readiness(SetupCheckSummary::Healthy);
    assert!(matches!(readiness, CardReadiness::Ready(_)));
    assert_eq!(readiness.label(), "All checks passed");
}

#[test]
fn set_up_emulators_never_shows_passed_when_doctor_found_warnings() {
    let readiness = check_setup_readiness(SetupCheckSummary::Warnings(2));
    assert!(matches!(readiness, CardReadiness::Unavailable(_)));
    assert_eq!(readiness.label(), "2 warnings");
    assert_ne!(readiness.label(), "All checks passed");
}

#[test]
fn set_up_emulators_never_shows_passed_when_doctor_found_blocking_problems() {
    let readiness = check_setup_readiness(SetupCheckSummary::NeedsAttention(1));
    assert!(matches!(readiness, CardReadiness::Unavailable(_)));
    assert_eq!(readiness.label(), "1 check needs attention");
    assert_ne!(readiness.label(), "All checks passed");
}

#[test]
fn set_up_emulators_never_shows_passed_when_no_check_could_run() {
    let readiness = check_setup_readiness(SetupCheckSummary::NoChecksRun);
    assert!(matches!(readiness, CardReadiness::Unknown(_)));
    assert_ne!(readiness.label(), "All checks passed");
}

#[test]
fn connect_romm_card_readiness_comes_from_the_romm_provider_state() {
    // Blocker 1: the card that now routes to the RomM provider workflow must
    // keep taking its badge from the RomM provider subsystem.
    let mut inputs = established_inputs(&[passing_check("config file")]);
    inputs.romm_state_label = Some(RommReadinessLabel::Unavailable("Offline"));
    let romm = build_home_view(&inputs)
        .cards
        .into_iter()
        .find(|c| c.card == HomeCard::RomM)
        .unwrap();
    assert_eq!(romm.title, "Connect RomM");
    assert_eq!(romm.action_label, "Open RomM");
    assert!(matches!(
        romm.readiness,
        Some(CardReadiness::Unavailable(_))
    ));

    inputs.romm_state_label = Some(RommReadinessLabel::Ready("Connected"));
    let romm = build_home_view(&inputs)
        .cards
        .into_iter()
        .find(|c| c.card == HomeCard::RomM)
        .unwrap();
    assert!(matches!(romm.readiness, Some(CardReadiness::Ready(_))));
}
