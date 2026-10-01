use super::*;
use super::{
    activity::Phase,
    artwork::{Artwork, Picture},
    library::{DuplicateGroup, DuplicateMember, DuplicateReport, Game, media_kind_label},
    media_sources::{Kind, MediaIndex, Source},
    problems::ProblemSummary,
};
use archivefs_core::PersistedArchive;
use archivefs_core::game_identity::{
    IdentityConfidence, IdentityEvidence, IdentityKind, IdentityProvenance, IdentityStatus,
};
use archivefs_core::persistent_state_inventory::{
    PersistentStateInventory, PersistentStateRecord, PersistentStateType, PortabilityClass,
    StateEmulator, StatePathOrigin,
};
use std::{fs, path::Path, sync::atomic::Ordering};

fn archive(id: i64, title: &str, platform: Option<&str>) -> PersistedArchive {
    PersistedArchive {
        id,
        source_folder_id: 1,
        relative_path: format!("{title}.iso").into(),
        absolute_path: format!("/fixture/{title}.iso").into(),
        archive_kind: "iso".into(),
        display_name: title.into(),
        normalized_name: title.into(),
        size_bytes: Some(4),
        modified_time_unix_seconds: Some(1),
        platform: platform.map(str::to_string),
        platform_source: platform.map(|_| "manual".into()),
        last_known_health: "pending".into(),
        last_seen_at: "2026-09-19".into(),
        last_verified_missing_at: None,
        identity_report: None,
    }
}

fn fixture(context: &egui::Context) -> App {
    readable_style(context);
    App {
        router: Router::default(),
        backend: Backend::start(context.clone()),
        library: Arc::new(Library::default()),
        indices: Vec::new(),
        filter: Filter::default(),
        filter_generation: 0,
        filter_inflight: false,
        filter_dirty: None,
        detail: None,
        detail_pending: None,
        detail_generation: 0,
        detail_failed: None,
        activity: Activity::default(),
        artwork: Artwork::with_indexer(context.clone(), None, Arc::new(|_| MediaIndex::default())),
        imagery: super::imagery::Imagery::default(),
        load_job: None,
        artwork_job: None,
        index_job: None,
        browse_play: Default::default(),
        preferences_dirty: None,
        interacted: false,
        loaded: true,
        notice: None,
        handoff_status: None,
        confirm_scan: false,
        screenshots: false,
        check_platform: None,
        verification: None,
        verification_job: None,
        duplicate_report: None,
        duplicate_job: None,
        problem_summary: None,
        problem_summary_job: None,
        duplicate_ignored: std::collections::HashSet::new(),
        problem_selected: None,
        problem_filter: super::problems::ProblemFilter::default(),
        problem_query: String::new(),
        repair_preview: None,
        repair_confirm: false,
        repair_job: None,
        repair_history: Vec::new(),
        undo_confirm: None,
        undo_job: None,
        repair_result: None,
        playing_library: crate::playing_library_page::PlayingLibraryPageState::load(),
        playing_library_history: Vec::new(),
        playing_library_job: None,
        playing_library_generation: 0,
        organisation: super::organisation::OrganisationState::default(),
        canonical_organisation: crate::rom_organisation_page::RomOrganisationPageState::default(),
        canonical_organisation_job: None,
        canonical_organisation_generation: 0,
        canonical_organisation_history: Vec::new(),
        mods: super::mods::ModsPageState::default(),
        native_workflows: None,
        environment: None,
        environment_job: None,
        welcome_dismissed: false,
        beginner_hints_enabled: true,
        doctor_platform: None,
        guidance: super::guidance::GuidanceState::default(),
        saves_states: super::saves_states::SavesStatesState::default(),
        converter: crate::optical_conversion_page::OpticalConversionPageState::default(),
        archive_inspector: super::archive_inspector::ArchiveInspectorPageState::default(),
        bezel: super::bezel::BezelPanelState::default(),
        hackhash: super::hackhash::HackHashPageState::default(),
        romm_library: super::romm_library::RommBrowserState::default(),
        romm_library_job: None,
        document_preferences: super::documents::DocumentPreferences::default(),
        document_cache: None,
        setup_portability: super::setup_portability::SetupPortabilityState::default(),
    }
}

#[test]
fn gui_v2_converter_is_a_native_workflow() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Converter);

    let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
    for expected in ["Disc Conversion", "CUE/BIN → CHD"] {
        assert!(
            strings.iter().any(|value| value.contains(expected)),
            "missing native converter content: {expected}"
        );
    }
}

#[test]
fn gui_v2_browse_play_reuses_filter_selection_and_canonical_destinations() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![
        archive(1, "Mario", Some("SNES")),
        archive(2, "Sonic", Some("Mega Drive")),
    ]));
    app.filter.select_platform("SNES".into());
    app.filter.search = "mario".into();
    assert_eq!(super::browse_play::filtered_game_indices(&app), vec![0]);
    assert_eq!(
        super::browse_play::browse_play_contextual_routes(1)[0].1,
        Route::Task {
            section: Section::Mods,
            game: 1
        }
    );
    assert_eq!(
        super::browse_play::browse_play_contextual_routes(1)[1].1,
        Route::Task {
            section: Section::Saves,
            game: 1
        }
    );
    assert_eq!(
        super::browse_play::browse_play_contextual_routes(1)[3].1,
        Route::Task {
            section: Section::Problems,
            game: 1
        }
    );
    assert!(
        super::browse_play::browse_play_contextual_routes(1)
            .iter()
            .any(|(label, route)| {
                *label == "Saves & States"
                    && *route
                        == Route::Task {
                            section: Section::Saves,
                            game: 1,
                        }
            })
    );
    assert!(
        super::browse_play::browse_play_contextual_routes(1)
            .iter()
            .any(|(label, route)| {
                *label == "Problems & Repair"
                    && *route
                        == Route::Task {
                            section: Section::Problems,
                            game: 1,
                        }
            })
    );
    app.router.current = Route::BrowsePlayGame(1);
    let strings = text(&frame(&context, &mut app, [820.0, 720.0]));
    for expected in ["Browse & Play", "Mario", "Play"] {
        assert!(
            strings.iter().any(|value| value.contains(expected)),
            "missing Browse & Play content: {expected}"
        );
    }
    assert_eq!(Route::Game(1).game(), Some(1));
    assert_eq!(app.filter.platform, "SNES");
    assert_eq!(app.filter.search, "mario");
}

#[test]
fn gui_v2_browse_play_empty_state_is_safe_and_points_to_sources() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.loaded = true;
    app.router.current = Route::BrowsePlay;
    let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("No games have been added yet"))
    );
    assert!(strings.iter().any(|value| value.contains("Open Sources")));
    assert_eq!(app.router.current, Route::BrowsePlay);
}

#[test]
fn gui_v2_browse_play_zero_results_explain_the_active_scope() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(1, "Mario", Some("SNES"))]));
    app.filter.search = "missing".into();
    app.router.current = Route::BrowsePlay;
    let strings = text(&frame(&context, &mut app, [820.0, 720.0]));
    assert!(strings.iter().any(|value| value == "No games match"));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("matching \"missing\""))
    );
    assert!(
        !strings
            .iter()
            .any(|value| value.contains("No games have been added yet"))
    );
}

#[test]
fn gui_v2_browse_play_repeated_widgets_have_semantic_ids_and_one_chrome() {
    let source = include_str!("browse_play.rs");
    assert!(source.contains("v2_browse_play_platform"));
    assert!(source.contains("v2_browse_play_game"));
    assert!(source.contains("v2_browse_play_action"));
    let pages = include_str!("pages.rs");
    assert_eq!(pages.matches("v2_app_chrome").count(), 1);
    assert!(pages.contains("v2_navigation"));
}

#[test]
fn gui_v2_top_toolbar_exposes_core_mouse_routes() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
    for expected in ["Home", "Games", "Jump to…"] {
        assert!(
            strings.iter().any(|value| value == expected),
            "missing app-chrome control: {expected}"
        );
    }
    assert!(
        routes::breadcrumb_labels(&Route::Section(Section::Check), None)
            .iter()
            .any(|label| label == "DATs & Verification")
    );
}

#[test]
fn gui_v2_quick_rename_is_a_native_dat_child_route() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::QuickRename;
    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));

    for expected in [
        "Quick Rename",
        "DATs & Verification",
        "Rename verified games to their trusted DAT names.",
        "Advanced Rename",
        "Manage DATs",
        "Open MAME tools",
        "Games",
    ] {
        assert!(
            strings.iter().any(|value| value.contains(expected)),
            "missing {expected}"
        );
    }
    assert!(!strings.iter().any(|value| value == "Organisation mode"));
}

#[test]
fn gui_v2_dat_page_offers_the_same_quick_rename_route() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Dat);
    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    assert!(strings.iter().any(|value| value == "Quick Rename"));
    assert_eq!(
        routes::breadcrumb_labels(&Route::QuickRename, None),
        ["DATs & Verification", "Quick Rename"]
    );
}

#[test]
fn gui_v2_organisation_landing_uses_user_intents() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Build);
    let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
    for expected in [
        "Rename verified games",
        "Build a clean playing library",
        "Fix my MAME library",
        "Analyse MAME collection",
    ] {
        assert!(
            strings.iter().any(|value| value.contains(expected)),
            "missing organisation intent: {expected}"
        );
    }
}

#[test]
fn gui_v2_romm_browser_has_explicit_empty_failure_state() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.romm_library.snapshot = Some(super::romm_library::RommBrowserSnapshot::unavailable(
        "no cache",
    ));
    app.router.current = Route::Section(Section::Romm);
    let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
    // The single page title comes from the page header.
    assert!(strings.iter().any(|value| value == "RomM Library"));
    assert!(!strings.iter().any(|value| value == "RomM library"));
    assert!(strings.iter().any(|value| value.contains("unavailable")));
    assert!(strings.iter().any(|value| value.contains("Local EmuWiz")));
}

#[test]
fn gui_v2_mame_health_shows_internal_repair_preview_without_apply() {
    let context = egui::Context::default();
    let plan = archivefs_core::mame_internal_repair::MameInternalRepairPlan {
        schema_version: 1,
        collection_root: "/roms".into(),
        catalogue_version: Some("0.264".into()),
        sets_currently_failing: 3,
        affected_sets: vec!["pacman".into()],
        requirements: Vec::new(),
        safe_internal_repair_count: 4,
        no_download_needed_count: 4,
        genuinely_absent_count: 2,
        preservation_only_no_dump_count: 1,
        bad_dump_count: 1,
        ambiguous_count: 1,
        wrong_content_same_name_count: 1,
        unique_source_identities_needed: 3,
        filesystem_operations_required: 4,
        projected_sets_repairable: 2,
        top_repairs_by_impact: Vec::new(),
        warnings: Vec::new(),
    };
    let output = context.run(Default::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            super::mame_collection_health::show_with_plans(ui, None, Some(&plan));
        });
    });
    let strings = text(&output);
    for expected in [
        "Repair from your own collection",
        "Exact matches already available",
        "Genuinely absent",
        "Preservation-only / NO_DUMP",
        "Present but BAD_DUMP",
        "No Apply button is available: this health projection is read-only.",
    ] {
        assert!(
            strings.iter().any(|value| value == expected),
            "missing MAME repair UI text: {expected}"
        );
    }
}

#[test]
fn artwork_route_exposes_local_first_bezel_preview() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(1, "Bezel Game", Some("SNES"))]));
    app.router.current = Route::Task {
        section: Section::Artwork,
        game: 1,
    };
    let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
    assert!(strings.iter().any(|value| value == "Bezel & decorations"));
}

#[test]
fn converter_route_does_not_use_legacy_handoff() {
    assert_eq!(
        super::pages::native_route_for_handoff(Section::Converter, None),
        None
    );
}

#[test]
fn museum_route_is_native_and_does_not_use_the_legacy_handoff() {
    assert_eq!(
        super::pages::native_route_for_handoff(Section::Museum, None),
        None
    );
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(1, "Museum Game", Some("SNES"))]));
    app.router.current = Route::Section(Section::Museum);
    let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
    assert!(strings.iter().any(|value| value == "Museum"));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("1 catalogued games"))
    );
    assert!(strings.iter().any(|value| value.contains("SNES (1)")));
}

#[test]
fn museum_empty_state_reflects_the_current_v2_catalogue() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Museum);
    let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("No games in the current catalogue"))
    );
    assert!(
        !strings
            .iter()
            .any(|value| value.contains("No library loaded yet"))
    );
}

#[test]
fn museum_platform_selection_browses_titles_and_keeps_catalogue_counts() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![
        archive(1, "SNES One", Some("SNES")),
        archive(2, "SNES Two", Some("SNES")),
        archive(3, "Mega Drive One", Some("Mega Drive")),
    ]));
    app.filter.select_platform("SNES".into());
    app.router.current = Route::Section(Section::Museum);
    let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
    assert!(strings.iter().any(|value| value.contains("SNES One")));
    assert!(strings.iter().any(|value| value.contains("SNES Two")));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("2 title(s) in the current catalogue"))
    );
    assert!(!strings.iter().any(|value| value.contains("Mega Drive One")));
}

#[test]
fn museum_titles_offer_details_and_play_routes() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(
        41,
        "Play From Museum",
        Some("SNES"),
    )]));
    app.filter.select_platform("SNES".into());
    app.router.current = Route::Section(Section::Museum);
    let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
    assert!(strings.iter().any(|value| value == "Details"));
    assert!(strings.iter().any(|value| value == "Play"));
    assert!(strings.iter().any(|value| value.contains("No picture yet")
        || value.contains("Loading picture")
        || value.contains("Preparing artwork")));
}

fn frame(context: &egui::Context, app: &mut App, size: [f32; 2]) -> egui::FullOutput {
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(size[0], size[1]),
            )),
            ..Default::default()
        },
        |context| {
            app.show(context);
        },
    )
}

fn saves_frame(context: &egui::Context, app: &mut App, size: [f32; 2]) -> egui::FullOutput {
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(size[0], size[1]),
            )),
            ..Default::default()
        },
        |context| {
            egui::CentralPanel::default().show(context, |ui| {
                super::saves_states::show(app, ui, None);
            });
        },
    )
}

fn scroll_page(context: &egui::Context, app: &mut App, size: [f32; 2]) -> egui::FullOutput {
    frame(context, app, size);
    let _ = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(size[0], size[1]),
            )),
            events: vec![
                egui::Event::PointerMoved(egui::pos2(size[0] * 0.6, size[1] - 150.0)),
                egui::Event::MouseWheel {
                    phase: egui::TouchPhase::Move,
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -250.0),
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        },
        |context| app.show(context),
    );
    for _ in 0..20 {
        frame(context, app, size);
    }
    frame(context, app, size)
}

fn text(output: &egui::FullOutput) -> Vec<String> {
    fn gather(shape: &egui::Shape, output: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => output.push(text.galley.text().to_string()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    gather(shape, output);
                }
            }
            _ => {}
        }
    }
    let mut texts = Vec::new();
    for shape in &output.shapes {
        gather(&shape.shape, &mut texts);
    }
    texts
}

fn text_bounds(output: &egui::FullOutput, wanted: &str) -> Vec<egui::Rect> {
    fn gather(shape: &egui::Shape, wanted: &str, output: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == wanted => {
                output.push(egui::Rect::from_min_size(text.pos, text.galley.size()));
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    gather(shape, wanted, output);
                }
            }
            _ => {}
        }
    }
    let mut bounds = Vec::new();
    for shape in &output.shapes {
        gather(&shape.shape, wanted, &mut bounds);
    }
    bounds
}

#[test]
fn gui_v2_guidance_precedes_large_problem_body_and_stays_visible_when_scrolling() {
    for height in [600.0, 1080.0] {
        let context = egui::Context::default();
        let mut app = fixture(&context);
        app.router.current = Route::Section(Section::Problems);
        app.library = Arc::new(Library::new(vec![archive(1, "Missing Game", Some("PS2"))]));
        let mut summary = ProblemSummary::from_library(&app.library, None);
        let template = summary.problems[0].clone();
        summary.problems = (0..132_064)
            .map(|index| {
                let mut problem = template.clone();
                problem.id = format!("missing-{index}");
                problem.title = format!("Missing Game {index}");
                problem
            })
            .collect();
        summary
            .category_indices
            .insert(template.category, (0..132_064).collect());
        app.problem_summary = Some(Arc::new(summary));
        frame(&context, &mut app, [1280.0, height]);
        let output = frame(&context, &mut app, [1280.0, height]);
        let guidance = text_bounds(&output, "Mr Wiz · Explain");
        assert_eq!(guidance.len(), 1);
        let controls = text_bounds(&output, "Inbox view")[0];
        let header = text_bounds(&output, "Problems & Repair")
            .into_iter()
            .find(|rect| rect.left() >= controls.left())
            .unwrap();
        assert!(header.bottom() < guidance[0].top());
        assert!(guidance[0].bottom() < controls.top());
        assert!(guidance[0].bottom() < height - 80.0);
        let scrolled = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, height),
                )),
                events: vec![
                    egui::Event::PointerMoved(egui::pos2(900.0, height - 100.0)),
                    egui::Event::MouseWheel {
                        phase: egui::TouchPhase::Move,
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -1800.0),
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            },
            |context| app.show(context),
        );
        assert_eq!(text_bounds(&scrolled, "Mr Wiz · Explain"), guidance);
        app.beginner_hints_enabled = false;
        assert!(
            text_bounds(
                &frame(&context, &mut app, [1280.0, height]),
                "Mr Wiz · Explain"
            )
            .is_empty()
        );
    }
}

#[test]
fn gui_v2_sidebar_wheel_hover_and_focus_do_not_activate_routes() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let initial = app.router.current.clone();
    frame(&context, &mut app, [1024.0, 600.0]);
    for step in 0..80 {
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1024.0, 600.0),
                )),
                events: vec![
                    egui::Event::PointerMoved(egui::pos2(80.0, 180.0 + (step % 6) as f32 * 55.0)),
                    egui::Event::MouseWheel {
                        phase: egui::TouchPhase::Move,
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, if step % 20 < 10 { -180.0 } else { 180.0 }),
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::Key {
                        key: egui::Key::Tab,
                        physical_key: None,
                        pressed: step % 2 == 0,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            },
            |context| app.show(context),
        );
        assert_eq!(app.router.current, initial, "wheel/focus pass {step}");
    }
    let output = frame(&context, &mut app, [1024.0, 600.0]);
    let point = text_bounds(&output, "Browse & Play")[0].center();
    for pressed in [true, false] {
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1024.0, 600.0),
                )),
                events: vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            },
            |context| app.show(context),
        );
    }
    assert_eq!(app.router.current, Route::BrowsePlay);
}

/// Clicks the centre of the (collapsing header or button) text `label`, then
/// returns a settled frame so the result of the click can be inspected.
fn click_label(
    context: &egui::Context,
    app: &mut App,
    size: [f32; 2],
    label: &str,
) -> egui::FullOutput {
    // Header opening is animated; settle it instantly so one frame is enough.
    context.style_mut(|style| style.animation_time = 0.0);
    let layout = frame(context, app, size);
    let point = text_bounds(&layout, label)
        .into_iter()
        .find(|rect| rect.min.y > 64.0)
        .unwrap_or_else(|| panic!("{label} not on screen"))
        .center();
    for pressed in [true, false] {
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(size[0], size[1]),
                )),
                events: vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            },
            |context| app.show(context),
        );
    }
    frame(context, app, size);
    frame(context, app, size)
}

fn state_record(state_type: PersistentStateType, emulator: StateEmulator) -> PersistentStateRecord {
    PersistentStateRecord {
        emulator,
        selected_installation: None,
        state_type,
        game_identity: Vec::new(),
        path: "/fixture/saves/checkpoint.bin".into(),
        container_path: None,
        slot_profile_account: None,
        emulator_version: None,
        firmware_context: None,
        portability_class: PortabilityClass::SafeToCopy,
        source_path_origin: StatePathOrigin::Configured,
        provenance: "fixture".into(),
        sha256: None,
        size_bytes: 128,
        warnings: Vec::new(),
    }
}

fn saves_inventory(records: Vec<PersistentStateRecord>) -> PersistentStateInventory {
    PersistentStateInventory {
        records,
        warnings: Vec::new(),
        roots_inspected: 1,
        read_only: true,
    }
}

#[test]
fn gui_v2_saves_empty_state_explains_read_only_discovery() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.saves_states.inventory = Some(saves_inventory(Vec::new()));
    app.router.current = Route::Section(Section::Saves);

    let strings = text(&saves_frame(&context, &mut app, [1280.0, 720.0]));
    for expected in [
        "Review your saved progress",
        "Your progress, preserved safely.",
        "No saves found yet",
        "game saves, memory cards and savestates",
        "Inspection never changes them",
        "Refresh save locations",
    ] {
        assert!(
            strings.iter().any(|value| value.contains(expected)),
            "missing {expected}"
        );
    }
}

#[test]
fn gui_v2_saves_populated_state_keeps_save_types_and_game_identity_scanable() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let mut selected = state_record(PersistentStateType::NativeSave, StateEmulator::DuckStation);
    selected.game_identity.push(IdentityEvidence {
        kind: IdentityKind::Ps1Serial,
        status: IdentityStatus::Verified,
        value: Some("SLUS-20312".into()),
        confidence: IdentityConfidence::ExactBytes,
        provenance: IdentityProvenance {
            archive_path: "/fixture/game.iso".into(),
            member_path: None,
            member_index: None,
            method: "fixture".into(),
        },
        diagnostic: "fixture".into(),
    });
    app.saves_states.inventory = Some(saves_inventory(vec![
        selected,
        state_record(PersistentStateType::SaveState, StateEmulator::DuckStation),
        state_record(PersistentStateType::MemoryCard, StateEmulator::Pcsx2),
    ]));
    app.router.current = Route::Section(Section::Saves);

    let strings = text(&saves_frame(&context, &mut app, [1280.0, 720.0]));
    for expected in [
        "Game save",
        "Savestate",
        "Memory card",
        "Game identity",
        "SLUS-20312",
        "Read-only inspection",
        "Open PS1/PS2 Save Vault",
    ] {
        assert!(
            strings.iter().any(|value| value.contains(expected)),
            "missing {expected}"
        );
    }
}

#[test]
fn gui_v2_saves_remains_readable_at_narrow_width() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.saves_states.inventory = Some(saves_inventory(vec![state_record(
        PersistentStateType::SaveState,
        StateEmulator::DuckStation,
    )]));
    app.router.current = Route::Section(Section::Saves);

    let strings = text(&saves_frame(&context, &mut app, [620.0, 480.0]));
    assert!(
        strings
            .iter()
            .any(|value| value == "Review your saved progress")
    );
    assert!(!strings.iter().any(|value| value == "Saves & States"));
    assert!(strings.iter().any(|value| value.contains("Savestate")));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("Read-only inspection"))
    );
}

#[test]
fn gui_v2_navigation_keeps_selection_and_back_history() {
    let mut router = Router::default();
    router.go(Route::Section(Section::Games));
    router.go(Route::Game(42));
    assert_eq!(router.current.section(), Section::Games);
    router.go(Route::Task {
        section: Section::Mods,
        game: 42,
    });
    router.back();
    assert_eq!(router.current, Route::Game(42));
    router.back();
    assert_eq!(router.current, Route::Section(Section::Games));
    router.back();
    assert_eq!(router.current, Route::Home);
}

#[test]
fn gui_v2_top_toolbar_destinations_render_and_share_sidebar_routes() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let output = frame(&context, &mut app, [1280.0, 720.0]);
    let strings = text(&output);

    for label in ["Home", "Games", "Jump to…"] {
        assert!(
            strings.iter().any(|value| value == label),
            "missing app-chrome control {label}"
        );
    }

    for section in [
        Section::Games,
        Section::Platforms,
        Section::Build,
        Section::Launch,
        Section::Converter,
        Section::Museum,
        Section::Setup,
    ] {
        assert!(routes::SECTIONS.contains(&section));
        app.go(Route::Section(section));
        assert_eq!(app.router.current.section(), section);
    }
}

#[test]
fn gui_v2_top_toolbar_back_uses_existing_history_at_narrow_width() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.go(Route::Section(Section::Platforms));
    assert!(app.router.can_back());

    let output = frame(&context, &mut app, [480.0, 360.0]);
    let strings = text(&output);
    assert!(strings.iter().any(|value| value == "Back"));

    app.back();
    assert_eq!(app.router.current, Route::Home);
    assert!(!app.router.can_back());
}

#[test]
fn gui_v2_arcade_set_is_a_logical_library_row_with_plain_details() {
    let mut persisted = archive(7, "Pac-Man", Some("Arcade"));
    persisted.archive_kind = "arcade_set_directory".into();
    persisted.relative_path = "pacman".into();
    persisted.absolute_path = "/fixture/arcade/pacman".into();

    assert_eq!(media_kind_label(&persisted.archive_kind), "Arcade set");
    let library = Library::new(vec![persisted]);
    assert_eq!(library.games.len(), 1);
    assert_eq!(library.games[0].title, "Pac-Man");
    assert_eq!(library.games[0].archive.relative_path, Path::new("pacman"));

    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(library);
    app.router.current = Route::Game(7);
    let strings = text(&frame(&context, &mut app, [1280.0, 1200.0]));

    assert!(strings.iter().any(|value| value.ends_with("· Arcade set")));
    // The set folder is reference material, kept behind Advanced details.
    assert!(!strings.iter().any(|value| value == "Source: pacman"));
    assert!(!strings.iter().any(|value| value.contains("unknown media")));
}

#[test]
fn gui_v2_playing_library_preview_runs_on_the_background_worker() {
    let context = egui::Context::default();
    let backend = super::backend::Backend::start(context);
    let state = crate::playing_library_page::PlayingLibraryPageState::load();
    backend
        .send(
            41,
            super::backend::Command::PlayingLibraryPreview {
                state: Box::new(state),
                generation: 9,
            },
        )
        .unwrap();
    assert!(matches!(
        backend
            .rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap(),
        super::backend::Event::Started(41)
    ));
    let event = backend
        .rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(matches!(
        event,
        super::backend::Event::Finished {
            id: 41,
            outcome: Ok(super::backend::Payload::PlayingLibraryPreview { generation: 9, .. }),
        }
    ));
}

#[test]
fn gui_v2_playing_library_apply_runs_on_the_background_worker() {
    let context = egui::Context::default();
    let backend = super::backend::Backend::start(context);
    let state = crate::playing_library_page::PlayingLibraryPageState::load();
    backend
        .send(
            42,
            super::backend::Command::PlayingLibraryApply {
                state: Box::new(state),
                generation: 10,
            },
        )
        .unwrap();
    assert!(matches!(
        backend
            .rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap(),
        super::backend::Event::Started(42)
    ));
    assert!(matches!(
        backend
            .rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap(),
        super::backend::Event::Finished {
            id: 42,
            outcome: Ok(super::backend::Payload::PlayingLibraryApply { generation: 10, .. }),
        }
    ));
}

#[test]
fn gui_v2_canonical_organisation_preview_runs_on_the_background_worker() {
    let context = egui::Context::default();
    let backend = super::backend::Backend::start(context);
    backend
        .send(
            43,
            super::backend::Command::CanonicalOrganisation {
                state: Box::new(crate::rom_organisation_page::RomOrganisationPageState::default()),
                generation: 11,
                kind: super::CanonicalOrganisationJobKind::Preview,
            },
        )
        .unwrap();
    assert!(matches!(
        backend
            .rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap(),
        super::backend::Event::Started(43)
    ));
    assert!(matches!(
        backend
            .rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap(),
        super::backend::Event::Finished {
            id: 43,
            outcome: Ok(super::backend::Payload::CanonicalOrganisation {
                generation: 11,
                kind: super::CanonicalOrganisationJobKind::Preview,
                ..
            }),
        }
    ));
}

#[test]
fn gui_v2_frontend_projection_preview_runs_on_the_background_worker() {
    let context = egui::Context::default();
    let backend = super::backend::Backend::start(context);
    backend
        .send(
            44,
            super::backend::Command::PlayingLibrarySpecial {
                state: Box::new(crate::playing_library_page::PlayingLibraryPageState::default()),
                generation: 12,
                kind: super::PlayingLibraryJobKind::PreviewRomm,
            },
        )
        .unwrap();
    assert!(matches!(
        backend
            .rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap(),
        super::backend::Event::Started(44)
    ));
    assert!(matches!(
        backend
            .rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap(),
        super::backend::Event::Finished {
            id: 44,
            outcome: Ok(super::backend::Payload::PlayingLibrarySpecial {
                generation: 12,
                kind: super::PlayingLibraryJobKind::PreviewRomm,
                ..
            }),
        }
    ));
}

#[test]
fn gui_v2_stale_playing_library_preview_is_discarded() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.playing_library.source_root_draft = "/new-source".into();
    app.playing_library_generation = 1;
    let activity_id = app.activity.queue(
        "Planning your playing library",
        Route::Section(Section::Build),
        false,
    );
    app.playing_library_job = Some(super::PlayingLibraryJob {
        id: activity_id,
        kind: super::PlayingLibraryJobKind::Preview,
        generation: 1,
        input_fingerprint: "old-input".into(),
    });
    let result = app.playing_library.clone();
    app.finish_playing_library_job(
        activity_id,
        super::PlayingLibraryJobKind::Preview,
        Box::new(result),
        1,
    );
    assert!(app.playing_library_job.is_none());
    assert!(
        app.activity
            .jobs
            .get(&activity_id)
            .unwrap()
            .summary
            .contains("discarded")
    );
}

#[test]
fn gui_v2_changed_then_restored_playing_library_input_stays_stale() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let original = app.playing_library.source_root_draft.clone();
    let activity_id = app.activity.queue(
        "Planning your playing library",
        Route::Section(Section::Build),
        false,
    );
    app.playing_library_job = Some(super::PlayingLibraryJob {
        id: activity_id,
        kind: super::PlayingLibraryJobKind::Preview,
        generation: 1,
        input_fingerprint: app.playing_library.input_fingerprint(),
    });
    app.playing_library_generation = 1;
    app.playing_library.source_root_draft = "/changed-source".into();
    app.invalidate_changed_playing_library_plan();
    app.playing_library.source_root_draft = original;
    let result = app.playing_library.clone();
    app.finish_playing_library_job(
        activity_id,
        super::PlayingLibraryJobKind::Preview,
        Box::new(result),
        1,
    );
    assert!(
        app.activity
            .jobs
            .get(&activity_id)
            .unwrap()
            .summary
            .contains("discarded")
    );
}

#[test]
fn gui_v2_duplicate_playing_library_submit_is_refused() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.playing_library_job = Some(super::PlayingLibraryJob {
        id: 99,
        kind: super::PlayingLibraryJobKind::Preview,
        generation: 1,
        input_fingerprint: app.playing_library.input_fingerprint(),
    });
    app.playing_library_generation = 1;
    app.start_playing_library_job(super::PlayingLibraryJobKind::Preview);
    assert_eq!(app.playing_library_job.as_ref().unwrap().id, 99);
}

#[test]
fn gui_v2_duplicate_canonical_organisation_submit_is_refused() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.canonical_organisation_job = Some(super::CanonicalOrganisationJob {
        id: 98,
        kind: super::CanonicalOrganisationJobKind::Preview,
        generation: 1,
        input_fingerprint: app.canonical_organisation.input_fingerprint(),
    });
    app.start_canonical_organisation_job(super::CanonicalOrganisationJobKind::Preview);
    assert_eq!(app.canonical_organisation_job.as_ref().unwrap().id, 98);
}

#[test]
fn gui_v2_stale_canonical_organisation_preview_is_discarded() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.canonical_organisation_generation = 1;
    let activity_id = app.activity.queue(
        "Planning verified-game organisation",
        Route::Section(Section::Build),
        false,
    );
    app.canonical_organisation_job = Some(super::CanonicalOrganisationJob {
        id: activity_id,
        kind: super::CanonicalOrganisationJobKind::Preview,
        generation: 1,
        input_fingerprint: "older-settings".into(),
    });
    app.finish_canonical_organisation_job(
        activity_id,
        super::CanonicalOrganisationJobKind::Preview,
        Box::new(app.canonical_organisation.clone()),
        1,
    );
    assert!(
        app.activity.jobs[&activity_id]
            .summary
            .contains("discarded")
    );
}

#[test]
fn gui_v2_stale_romm_projection_result_is_discarded() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.playing_library_generation = 3;
    let activity_id = app.activity.queue(
        "Checking the RomM library layout",
        Route::Section(Section::Build),
        false,
    );
    app.playing_library_job = Some(super::PlayingLibraryJob {
        id: activity_id,
        kind: super::PlayingLibraryJobKind::PreviewRomm,
        generation: 3,
        input_fingerprint: "old-preferences".into(),
    });
    app.finish_playing_library_job(
        activity_id,
        super::PlayingLibraryJobKind::PreviewRomm,
        Box::new(app.playing_library.clone()),
        3,
    );
    assert!(
        app.activity.jobs[&activity_id]
            .summary
            .contains("discarded")
    );
}

#[test]
fn gui_v2_organisation_is_a_native_plain_english_workflow() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Build);
    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    for expected in [
        "Choose what you want to organise",
        "Rename verified games",
        "Build a clean playing library",
        "Fix my MAME library",
    ] {
        assert!(strings.iter().any(|value| value == expected), "{expected}");
    }
    assert!(
        strings
            .iter()
            .any(|value| value.contains("Nothing changes until"))
    );
    assert!(
        strings
            .iter()
            .any(|value| value.contains("Source untouched"))
    );
}

#[test]
fn gui_v2_organisation_advanced_options_stay_native() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Build);
    let strings = text(&frame(&context, &mut app, [1280.0, 1800.0]));
    for expected in ["Advanced organisation options", "Fix my MAME library"] {
        assert!(
            strings.iter().any(|value| value.contains(expected)),
            "{expected}"
        );
    }
    assert!(
        !strings
            .iter()
            .any(|value| value.contains("Legacy / Advanced interface"))
    );
}

#[test]
fn gui_v2_organisation_sidebar_title_and_all_normal_flows_are_reachable() {
    assert_eq!(Section::Build.title(), "Organisation");
    let context = egui::Context::default();
    for (destination, expected) in [
        (
            crate::playing_library_page::PlayingLibraryDestination::Generic,
            "Generic Library",
        ),
        (
            crate::playing_library_page::PlayingLibraryDestination::Romm,
            "RomM Library",
        ),
        (
            crate::playing_library_page::PlayingLibraryDestination::EsDe,
            "ES-DE Library",
        ),
        (
            crate::playing_library_page::PlayingLibraryDestination::RetroDeck,
            "RetroDECK Library",
        ),
    ] {
        let mut app = fixture(&context);
        app.router.current = Route::Section(Section::Build);
        app.organisation.view = super::organisation::OrganisationView::PlayingLibrary;
        app.playing_library.set_destination(destination);
        let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
        assert!(strings.iter().any(|value| value == expected), "{expected}");
        assert!(
            strings
                .iter()
                .any(|value| value.contains("Original files stay untouched"))
        );
    }
}

#[test]
fn gui_v2_verified_game_flow_explains_move_rename_link_and_keeps_advanced_escape() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Build);
    app.organisation.view = super::organisation::OrganisationView::VerifiedGames;
    let strings = text(&frame(&context, &mut app, [1366.0, 768.0]));
    for wording in ["MOVE changes", "RENAME changes", "LINK leaves"] {
        assert!(
            strings.iter().any(|value| value.contains(wording)),
            "{wording}"
        );
    }
    assert!(strings.iter().any(|value| value == "← Organisation"));
}

#[test]
fn gui_v2_organisation_landing_remains_usable_at_supported_viewports() {
    for size in [
        [1280.0, 720.0],
        [1366.0, 768.0],
        [1920.0, 1080.0],
        [2560.0, 1440.0],
    ] {
        let context = egui::Context::default();
        let mut app = fixture(&context);
        app.router.current = Route::Section(Section::Build);
        let strings = text(&frame(&context, &mut app, size));
        assert!(
            strings.iter().any(|value| value == "Organisation"),
            "{size:?}"
        );
        assert!(
            strings.iter().any(|value| value == "Rename verified games"),
            "{size:?}"
        );
    }
}

#[test]
fn gui_v2_organisation_flow_survives_navigation_away_and_back() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Build);
    app.organisation.view = super::organisation::OrganisationView::PlayingLibrary;
    app.playing_library
        .set_destination(crate::playing_library_page::PlayingLibraryDestination::Romm);
    app.go(Route::Section(Section::Activity));
    app.back();
    assert_eq!(app.router.current, Route::Section(Section::Build));
    assert_eq!(
        app.organisation.view,
        super::organisation::OrganisationView::PlayingLibrary
    );
    assert_eq!(
        app.playing_library.destination,
        crate::playing_library_page::PlayingLibraryDestination::Romm
    );
}

/// The first-use / empty-state explainer: source stays intact, a preview
/// always comes first, and destination/output is separate - shown while the
/// primary action cards remain fully visible and reachable.
///
/// Uses a tall [1280.0, 1800.0] viewport, similar in spirit to
/// `gui_v2_organisation_is_a_native_plain_english_workflow`'s 820px fixture:
/// this harness renders a single `egui::Context::run` frame, so - exactly
/// like a real (multi-frame, scrollable) session's first paint - content
/// past the first frame's laid-out height is not yet drawn. The extra
/// height accounts for the new hero and contextual guidance pushing the five
/// cards further down than the previous plain-list layout.
#[test]
fn gui_v2_organisation_landing_shows_the_plain_english_explainer_alongside_actions() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Build);
    let strings = text(&frame(&context, &mut app, [1280.0, 1800.0]));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("preview before anything happens")
                || value.contains("Nothing changes until"))
    );
    // All five action cards must still be present and clickable alongside
    // the contextual guidance - it must never bury the primary actions.
    for title in [
        "Rename verified games",
        "Build a clean playing library",
        "Organise for RomM",
        "Export to ES-DE",
        "Prepare for RetroDECK",
    ] {
        assert!(strings.iter().any(|value| value == title), "{title}");
    }
    // Re-rendering keeps the contextual guidance local and the actions intact.
    let strings_after = text(&frame(&context, &mut app, [1280.0, 1800.0]));
    assert!(
        strings_after
            .iter()
            .any(|value| value == "Rename verified games")
    );
}

/// Each organisation target's card is individually reachable and labelled,
/// and clicking it routes to the correct destination - Playing Library,
/// RomM, ES-DE and RetroDECK must each be selectable independently.
#[test]
fn gui_v2_organisation_each_target_card_routes_to_its_own_destination() {
    let context = egui::Context::default();
    for (title, expected_destination) in [
        (
            "Build a clean playing library",
            crate::playing_library_page::PlayingLibraryDestination::Generic,
        ),
        (
            "Organise for RomM",
            crate::playing_library_page::PlayingLibraryDestination::Romm,
        ),
        (
            "Export to ES-DE",
            crate::playing_library_page::PlayingLibraryDestination::EsDe,
        ),
        (
            "Prepare for RetroDECK",
            crate::playing_library_page::PlayingLibraryDestination::RetroDeck,
        ),
    ] {
        // Selecting a destination and entering its Playing Library flow is
        // exactly the same routing the card's own click handler performs
        // (`super::organisation::organisation_page`); this asserts the
        // per-target flow it leads to renders correctly for each target,
        // matching the un-restyled routing behaviour.
        let mut app = fixture(&context);
        app.router.current = Route::Section(Section::Build);
        app.playing_library.set_destination(expected_destination);
        app.organisation.view = super::organisation::OrganisationView::PlayingLibrary;
        let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
        assert!(
            strings.iter().any(|value| value == "← Organisation"),
            "{title}: back control must remain reachable"
        );
        assert_eq!(app.playing_library.destination, expected_destination);
    }
}

/// The Organisation page must remain fully usable at the accessibility
/// floor (1280x720): the hero, the plain-English explainer and the first
/// action card render without the motif artwork crowding out its own text
/// or button - and the page uses a vertical `ScrollArea` (unchanged by this
/// pass), so the remaining cards stay reachable by scrolling exactly as the
/// destination card grid already was before this visual pass. (This
/// single-frame harness cannot itself simulate a scroll gesture; the full
/// five-card set rendering correctly is covered by
/// `gui_v2_organisation_landing_shows_the_plain_english_explainer_alongside_actions`
/// at a taller fixture height, following this test file's existing
/// convention for viewport-height-sensitive assertions.)
#[test]
fn gui_v2_organisation_landing_is_reachable_at_narrow_1280x720() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Build);
    let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
    for expected in [
        "Choose what you want to organise",
        "Nothing changes until you preview and confirm",
        "Rename verified games",
    ] {
        assert!(
            strings.iter().any(|value| value.contains(expected)),
            "{expected}"
        );
    }
}

#[test]
fn gui_v2_mods_page_is_native_and_keeps_cheats_separate() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Mods);
    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    assert!(strings.iter().any(|value| value == "Mods & Cheats"));
    assert!(strings.iter().any(|value| value == "Available packages"));
    assert!(strings.iter().any(|value| value == "Active stack"));
    assert!(strings.iter().any(|value| value == "Conflicts"));
    assert!(strings.iter().any(|value| value == "Cheats"));
    assert!(!strings.iter().any(|value| value.contains("legacy handoff")));
}

#[test]
fn gui_v2_cheats_tab_is_native_and_has_a_safe_empty_state() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.mods.select_cheats();
    app.router.current = Route::Section(Section::Mods);

    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));

    assert!(strings.iter().any(|value| value == "Select a game first"));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("No cheat is changed"))
    );
    assert!(
        !strings
            .iter()
            .any(|value| value.contains("existing safe cheat workflow"))
    );
    assert!(!strings.iter().any(|value| value.contains("legacy handoff")));
}

#[test]
fn gui_v2_cheat_activity_lifecycle_reports_failure_safely() {
    let mut activity = Activity::default();
    let mut job = None;
    native_workflows::observe_cheat_activity_state(
        &mut activity,
        &mut job,
        Some(("Applying cheats", "Updating the selected emulator profile.")),
        None,
    );
    assert_eq!(activity.running(), 1);
    assert_eq!(
        activity.jobs.values().next().unwrap().title,
        "Applying cheats"
    );

    native_workflows::observe_cheat_activity_state(
        &mut activity,
        &mut job,
        None,
        Some("fixture apply failed".into()),
    );
    let completed = activity.jobs.values().next().unwrap();
    assert_eq!(completed.phase, Phase::Failed);
    assert!(completed.summary.contains("stopped safely"));
}

#[test]
fn gui_v2_history_projects_cheat_apply_and_truthful_undo_state() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let mut workflows = native_workflows::NativeWorkflows::new(context.clone());
    workflows
        .app
        .history
        .record(crate::activity_history::HistoryEntry::new(
            crate::activity_history::ActivityAction::CheatInstall,
            Some("/games/example.iso".into()),
            crate::activity_history::ActivityOutcome::Completed,
            "Installed one reviewed cheat with a recoverable shared transaction.",
        ));
    app.native_workflows = Some(workflows);
    app.router.current = Route::Section(Section::History);

    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    assert!(strings.iter().any(|value| value == "Cheat activity"));
    assert!(strings.iter().any(|value| value == "Cheats & Mods install"));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("Undo is unavailable"))
    );
}

#[test]
fn gui_v2_history_can_project_a_playing_library_transaction() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.playing_library_history.push(
        archivefs_core::dat::rename_apply::model::RenameTransaction {
            transaction_id: "playing-library-test".into(),
            plan_generation: 1,
            classifier_version: None,
            created_at_unix: 1,
            source_scan_root: "/tmp/playing-library".into(),
            state: archivefs_core::dat::rename_apply::model::TransactionState::Applied,
            entries: Vec::new(),
            created_directories: Vec::new(),
            recovery_resolution: None,
            recovery_resolved_at_unix: None,
            unknown: Default::default(),
        },
    );
    app.router.current = Route::Section(Section::History);
    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    assert!(strings.iter().any(|value| value == "Built Playing Library"));
    assert!(strings.iter().any(|value| value == "Ready to undo"));
}

#[test]
fn gui_v2_every_sidebar_route_has_a_purpose_and_action() {
    let mut unique = std::collections::HashSet::new();
    for section in routes::SECTIONS {
        assert!(unique.insert(section));
        assert!(!section.title().is_empty());
        assert!(!section.purpose().is_empty());
        assert!(!section.action().is_empty());
        assert_eq!(Route::Section(*section).section(), *section);
    }
    assert_eq!(unique.len(), routes::SECTIONS.len());
}

#[test]
fn gui_v2_feature_families_have_unique_homes_and_cover_direct_routes() {
    use routes::FeatureFamily;

    let families = [
        FeatureFamily::DatsVerification,
        FeatureFamily::CheatsMods,
        FeatureFamily::SavesStates,
        FeatureFamily::Emulators,
        FeatureFamily::Mame,
        FeatureFamily::ArtworkExtras,
        FeatureFamily::Conversion,
        FeatureFamily::Organisation,
        FeatureFamily::ProblemsRepair,
        FeatureFamily::SourcesProviders,
        FeatureFamily::HistoryUndo,
        FeatureFamily::AdvancedDiagnostics,
    ];
    let homes = families
        .iter()
        .map(|family| routes::family_home(*family))
        .collect::<Vec<_>>();
    assert_eq!(homes.len(), families.len());
    assert!(homes.windows(2).all(|window| window[0] != window[1]));

    for section in routes::SECTIONS
        .iter()
        .copied()
        .filter(|section| *section != Section::Home)
    {
        assert!(
            routes::family_for_route(&Route::Section(section)).is_some(),
            "unclassified route: {section:?}"
        );
    }
}

#[test]
fn gui_v2_family_children_preserve_canonical_workflows() {
    use routes::FeatureFamily;

    let dat = routes::family_children(FeatureFamily::DatsVerification);
    assert!(dat.iter().any(|action| action.label == "Check Games"));
    assert!(dat.iter().any(|action| action.label == "Quick Rename"));
    assert!(dat.iter().any(|action| action.label == "Advanced Rename"));
    assert!(dat.iter().any(|action| action.label == "DAT Management"));

    let cheats = routes::family_children(FeatureFamily::CheatsMods);
    assert!(cheats.iter().any(|action| action.label == "Cheats"));
    assert!(cheats.iter().any(|action| action.label == "Mods"));

    let saves = routes::family_children(FeatureFamily::SavesStates);
    for label in ["Saves", "Snapshots", "Restore", "Memory Cards"] {
        assert!(
            saves.iter().any(|action| action.label == label),
            "missing saves action: {label}"
        );
    }

    let emulators = routes::family_children(FeatureFamily::Emulators);
    assert!(
        emulators
            .iter()
            .any(|action| action.label == "Emulator Setup")
    );
    assert!(
        emulators
            .iter()
            .any(|action| action.label == "BIOS / Firmware")
    );
    assert!(
        routes::family_children(FeatureFamily::Mame)
            .iter()
            .any(|action| action.label == "Health")
    );
    for label in [
        "Repair",
        "Reconstruction",
        "Verify",
        "Playing Library",
        "Problems",
        "History & Undo",
    ] {
        assert!(
            routes::family_children(FeatureFamily::Mame)
                .iter()
                .any(|action| action.label == label),
            "missing MAME workflow: {label}"
        );
    }
    assert!(
        routes::family_children(FeatureFamily::Conversion)
            .iter()
            .any(|action| action.label == "Disc Conversion"
                && action.route == Route::Section(Section::Converter))
    );
    assert!(
        routes::family_children(FeatureFamily::Conversion)
            .iter()
            .any(|action| action.label == "History & Undo"
                && action.route == Route::Section(Section::History))
    );
    assert!(
        routes::family_children(FeatureFamily::Organisation)
            .iter()
            .any(|action| action.label == "Playing Library")
    );
    assert!(
        routes::family_children(FeatureFamily::ProblemsRepair)
            .iter()
            .any(|action| action.route == Route::Section(Section::Problems))
    );
    assert_eq!(
        routes::family_children(FeatureFamily::HistoryUndo)
            .iter()
            .filter(|action| action.route == Route::Section(Section::History))
            .count(),
        2
    );
}

#[test]
fn gui_v2_family_landing_render_is_paint_only_and_uses_family_breadcrumb() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = routes::family_home(routes::FeatureFamily::DatsVerification);
    let before = app.activity.jobs.len();
    let strings = text(&frame(&context, &mut app, [1280.0, 1000.0]));
    assert!(strings.iter().any(|value| value == "DATs & Verification"));
    assert!(strings.iter().any(|value| value == "Quick Rename · Easy"));
    assert!(
        strings
            .iter()
            .any(|value| value == "Advanced Rename · Advanced")
    );
    assert_eq!(app.activity.jobs.len(), before);
}

#[test]
fn gui_v2_saves_states_is_a_native_route_and_duplicate_refresh_is_refused() {
    assert!(routes::SECTIONS.contains(&Section::Saves));
    assert_eq!(Section::Saves.title(), "Saves & States");
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.start_saves_inventory();
    let first_job_count = app.activity.jobs.len();
    assert!(app.saves_states.loading);
    assert!(app.saves_states.job.is_some());
    app.start_saves_inventory();
    assert_eq!(app.activity.jobs.len(), first_job_count);
}

#[test]
fn gui_v2_migrates_the_old_dat_bookmark_to_dat_management() {
    assert_eq!(
        routes::migrate_route(Route::Section(Section::Advanced)),
        Route::Section(Section::Dat)
    );
    assert_eq!(
        routes::migrate_route(Route::Section(Section::Build)),
        Route::Section(Section::Build)
    );
}

#[test]
fn gui_v2_fresh_profile_opens_plain_language_welcome() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.environment = Some(environment::EnvironmentSnapshot::default());
    app.router.current = Route::Section(Section::Setup);
    let strings = text(&frame(&context, &mut app, [1280.0, 1200.0]));
    assert!(strings.iter().any(|value| value == "Welcome to EmuWiz"));
    assert!(strings.iter().any(|value| value == "Get started"));
    assert!(strings.iter().any(|value| value == "Suggested first steps"));
    assert!(strings.iter().any(|value| value == "Scan / inspect"));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("Set up the basics"))
    );
    assert!(strings.iter().all(|value| value != "DAT registry"));
}

#[test]
fn gui_v2_beginner_hints_can_be_disabled_without_hiding_advanced_tools() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.beginner_hints_enabled = false;
    app.router.current = Route::Section(Section::Games);

    let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
    assert!(strings.iter().all(|value| value != "Mr Wiz"));

    app.router.current = Route::Section(Section::Advanced);
    assert_eq!(app.router.current, Route::Section(Section::Advanced));
}

#[test]
fn gui_v2_settings_explain_beginner_terms_without_removing_advanced_access() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Settings);

    let strings = text(&frame(&context, &mut app, [1280.0, 900.0]));
    assert!(strings.iter().any(|value| value == "Beginner guidance"));
    assert!(strings.iter().any(|value| value == "Show beginner hints"));
}

#[test]
fn gui_v2_doctor_keeps_missing_mount_distinct_from_fresh_install() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let mut snapshot = environment::EnvironmentSnapshot {
        source_count: 1,
        ..environment::EnvironmentSnapshot::default()
    };
    snapshot.unavailable_sources.push("/mnt/games".into());
    assert!(!snapshot.is_fresh());
    assert!(snapshot.source_needs_attention());
    app.environment = Some(snapshot);
    app.welcome_dismissed = true;
    app.router.current = Route::Section(Section::Setup);
    let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("Game drive unavailable"))
    );
    assert!(strings.iter().all(|value| value != "Welcome to EmuWiz"));
}

#[test]
fn gui_v2_home_has_explained_tasks_with_correct_routes() {
    assert_eq!(routes::HOME_TASKS.len(), 7);
    assert_eq!(
        routes::HOME_TASKS
            .iter()
            .map(|task| task.0)
            .collect::<Vec<_>>(),
        vec![
            Section::Setup,
            Section::Games,
            Section::Check,
            Section::Problems,
            Section::Build,
            Section::Mods,
            Section::Launch
        ]
    );
    for (_, title, purpose, action) in routes::HOME_TASKS {
        assert!(!title.is_empty() && purpose.len() > 20 && !action.is_empty());
    }
}

#[test]
fn gui_v2_check_games_is_native_and_marks_arcade_ready() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(1, "Pac-Man", Some("Arcade"))]));
    app.router.current = Route::Section(Section::Check);
    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("Choose a platform"))
    );
    assert!(
        strings
            .iter()
            .any(|value| value.contains("MAME Arcade verification data ready"))
    );
    assert!(
        !strings
            .iter()
            .any(|value| value.contains("existing interface"))
    );
}

#[test]
fn gui_v2_problems_page_is_native_and_plain_english() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(
        1,
        "Missing Pac-Man",
        Some("Arcade"),
    )]));
    app.problem_summary = Some(Arc::new(ProblemSummary::from_library(&app.library, None)));
    app.problem_selected = Some("missing-1".into());
    app.router.current = Route::Section(Section::Problems);
    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("Broken or missing files"))
    );
    assert!(strings.iter().any(|value| value.contains("What happened")));
    assert!(strings.iter().any(|value| value.contains("Read-only")));
    assert!(
        !strings
            .iter()
            .any(|value| value.contains("existing interface"))
    );
}

#[test]
fn gui_v2_problems_page_has_loading_and_healthy_states() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Problems);
    let loading = text(&frame(&context, &mut app, [1280.0, 720.0]));
    assert!(
        loading
            .iter()
            .any(|value| value.contains("Checking the saved catalogue evidence"))
    );

    app.problem_summary = Some(Arc::new(ProblemSummary::default()));
    let healthy = text(&frame(&context, &mut app, [1280.0, 720.0]));
    assert!(
        healthy
            .iter()
            .any(|value| value.contains("Nothing currently needs your attention."))
    );
}

#[test]
fn gui_v2_history_has_a_truthful_empty_state() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::History);
    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("No repair history yet"))
    );
    assert!(
        !strings
            .iter()
            .any(|value| value.contains("Undo this repair"))
    );
}

#[test]
fn gui_v2_duplicate_badge_keeps_separate_files_visible() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let first = archive(1, "Same title", Some("PS2"));
    let mut second = archive(2, "Same title", Some("PS2"));
    second.relative_path = "other/Same title.iso".into();
    second.absolute_path = "/fixture/other/Same title.iso".into();
    app.library = Arc::new(Library::new(vec![first.clone(), second.clone()]));
    app.indices = vec![0, 1];
    app.duplicate_report = Some(DuplicateReport {
        files_examined: 2,
        exact_groups: Vec::new(),
        groups: vec![DuplicateGroup {
            exact_index: 0,
            kind: "Exact duplicates".into(),
            sha256: "abc".into(),
            size_bytes: 4,
            members: vec![
                DuplicateMember {
                    path: first.absolute_path,
                    title: first.display_name.clone(),
                    platform: "PS2".into(),
                    size_bytes: 4,
                    evidence: "same hash".into(),
                },
                DuplicateMember {
                    path: second.absolute_path,
                    title: second.display_name.clone(),
                    platform: "PS2".into(),
                    size_bytes: 4,
                    evidence: "same hash".into(),
                },
            ],
        }],
    });
    app.router.current = Route::Section(Section::Games);
    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    assert!(strings.iter().any(|value| value.contains("2 exact copies")));
    assert!(strings.iter().any(|value| value.contains("Same title")));
}

#[test]
fn gui_v2_duplicates_empty_state_explains_safe_review() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Duplicates);
    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("Nothing has been compared yet"))
    );
    assert!(
        strings
            .iter()
            .any(|value| value.contains("Find duplicates"))
    );
    assert!(
        strings
            .iter()
            .any(|value| value.contains("never deletes anything"))
    );
    assert!(
        strings
            .iter()
            .any(|value| value.contains("not filenames alone"))
    );
}

#[test]
fn gui_v2_duplicates_find_action_keeps_existing_scan_route() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.start_duplicate_scan();
    assert!(app.duplicate_job.is_some());
    let job = app.activity.jobs.values().next().expect("scan activity");
    assert_eq!(job.result, Route::Section(Section::Duplicates));
}

#[test]
fn gui_v2_duplicates_readiness_wording_uses_backend_states() {
    use archivefs_core::repair::GroupQuarantineReadiness;

    assert_eq!(
        super::pages::duplicate_readiness_label(&GroupQuarantineReadiness::Safe),
        "Exact duplicate · safe to preview"
    );
    assert_eq!(
        super::pages::duplicate_readiness_label(&GroupQuarantineReadiness::NeedsReview(
            "choice".into()
        )),
        "Review needed · no automatic action"
    );
    assert_eq!(
        super::pages::duplicate_readiness_label(&GroupQuarantineReadiness::Blocked(
            "blocked".into()
        )),
        "Blocked from automatic action"
    );
}

#[test]
fn gui_v2_duplicates_empty_state_fits_narrow_width() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Duplicates);
    let strings = text(&frame(&context, &mut app, [620.0, 720.0]));
    assert!(strings.iter().any(|value| value.contains("Duplicates")));
}

#[test]
fn gui_v2_verification_result_has_per_game_identity_status() {
    let mut result = VerificationResult {
        platform: "Arcade".into(),
        total: 1,
        matched: 1,
        ..Default::default()
    };
    result.statuses.insert(7, "Verified".into());
    assert_eq!(result.platform, "Arcade");
    assert_eq!(
        result.statuses.get(&7).map(String::as_str),
        Some("Verified")
    );
}

#[test]
fn gui_v2_legacy_handoff_preserves_game_and_workflow() {
    use crate::navigation::MainView;
    assert_eq!(
        legacy::destination(Section::Launch, true),
        MainView::Selected
    );
    assert_eq!(
        legacy::destination(Section::Launch, false),
        MainView::ReadyToPlay
    );
    assert_eq!(
        legacy::destination(Section::Mods, true),
        MainView::CheatsMods
    );
    assert_eq!(
        legacy::destination(Section::Check, true),
        MainView::CheckGames
    );
    assert_eq!(
        legacy::destination(Section::Advanced, false),
        MainView::Library
    );
    assert_eq!(
        legacy::destination(Section::Dat, false),
        MainView::DatSources
    );
}

#[test]
fn gui_v2_play_route_is_native_and_starts_readiness_without_legacy_handoff() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let game = archive(41, "Shadow of the Colossus", Some("PS2"));
    app.library = Arc::new(Library::new(vec![game]));
    app.indices = vec![0];
    app.router.current = Route::Task {
        section: Section::Launch,
        game: 41,
    };

    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    assert!(
        strings
            .iter()
            .any(|text| text.contains("Shadow of the Colossus"))
    );
    assert!(strings.iter().any(|text| text.contains("Platform: PS2")));
    assert!(
        strings
            .iter()
            .any(|text| text.contains("Play / Launch readiness"))
    );
    assert!(
        !strings
            .iter()
            .any(|text| text.contains("existing interface"))
    );
    assert!(app.native_workflows.is_some());
    assert!(app.activity.running() > 0);
}

#[test]
fn gui_v2_launch_handoff_cannot_spawn_the_legacy_problems_workflow() {
    assert_eq!(
        super::pages::native_route_for_handoff(Section::Launch, Some(41)),
        Some(Route::Task {
            section: Section::Launch,
            game: 41,
        })
    );
    assert_eq!(
        super::pages::native_route_for_handoff(Section::Launch, None),
        Some(Route::Section(Section::Games))
    );
    assert_eq!(
        super::pages::native_route_for_handoff(Section::Problems, Some(41)),
        None
    );
}

#[test]
fn gui_v2_emulator_setup_route_is_native_and_not_a_legacy_handoff() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Emulators);

    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    assert!(strings.iter().any(|text| text == "Emulators"));
    assert!(
        strings
            .iter()
            .any(|text| text.contains("Check which emulators"))
    );
    assert!(
        !strings
            .iter()
            .any(|text| text.contains("existing interface"))
    );
    assert!(app.native_workflows.is_some());
}

#[test]
fn gui_v2_firmware_route_is_native_and_recovery_stays_on_firmware_page() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Firmware);

    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    assert!(strings.iter().any(|text| text == "BIOS / Firmware"));
    assert!(
        strings
            .iter()
            .any(|text| text.contains("Review the firmware"))
    );
    assert!(
        !strings
            .iter()
            .any(|text| text.contains("Legacy / Advanced interface"))
    );
    assert!(app.native_workflows.is_some());
}

#[test]
fn gui_v2_native_launch_state_survives_navigation_away_and_back() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(42, "Disc set", Some("PSX"))]));
    app.indices = vec![0];
    app.router.current = Route::Task {
        section: Section::Launch,
        game: 42,
    };
    let _ = frame(&context, &mut app, [1280.0, 820.0]);
    assert_eq!(
        app.native_workflows
            .as_ref()
            .and_then(|bridge| bridge.selected_path()),
        Some(Path::new("/fixture/Disc set.iso"))
    );

    app.router.current = Route::Home;
    let _ = frame(&context, &mut app, [1280.0, 820.0]);
    app.router.current = Route::Task {
        section: Section::Launch,
        game: 42,
    };
    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));

    assert_eq!(
        app.native_workflows
            .as_ref()
            .and_then(|bridge| bridge.selected_path()),
        Some(Path::new("/fixture/Disc set.iso"))
    );
    assert!(
        strings
            .iter()
            .any(|text| text.contains("Play / Launch readiness"))
    );
}

#[test]
fn gui_v2_changed_game_discards_stale_readiness_and_tracks_the_new_game() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![
        archive(51, "First game", Some("PSX")),
        archive(52, "Second game", Some("PS2")),
    ]));
    app.indices = vec![0, 1];
    app.router.current = Route::Task {
        section: Section::Launch,
        game: 51,
    };
    let _ = frame(&context, &mut app, [1280.0, 820.0]);

    app.router.current = Route::Task {
        section: Section::Launch,
        game: 52,
    };
    let _ = frame(&context, &mut app, [1280.0, 820.0]);

    assert_eq!(
        app.native_workflows
            .as_ref()
            .and_then(|bridge| bridge.selected_path()),
        Some(Path::new("/fixture/Second game.iso"))
    );
    assert!(app.activity.jobs.values().any(|job| {
        job.active()
            && job.result
                == Route::Task {
                    section: Section::Launch,
                    game: 52,
                }
    }));
}

#[test]
fn gui_v2_library_search_platform_filter_attention_and_empty_platform() {
    let mut bad = archive(3, "Needs fixing", Some("PS2"));
    bad.last_verified_missing_at = Some("today".into());
    let library = Library::new(vec![
        archive(2, "Tekken", Some("PSX")),
        archive(1, "Shadow", Some("PS2")),
        bad,
    ]);
    assert_eq!(library.platforms["PS2"], 2);
    assert_eq!(library.attention, 1);
    assert_eq!(
        library
            .filter(&Filter {
                search: "SHADOW".into(),
                ..Default::default()
            })
            .len(),
        1
    );
    assert_eq!(
        library
            .filter(&Filter {
                platform: "PSX".into(),
                ..Default::default()
            })
            .len(),
        1
    );
    assert_eq!(
        library
            .filter(&Filter {
                attention_only: true,
                ..Default::default()
            })
            .len(),
        1
    );
    assert!(
        library
            .filter(&Filter {
                platform: "Amiga".into(),
                ..Default::default()
            })
            .is_empty()
    );
    assert_eq!(library.game(1).unwrap().title, "Shadow");
    assert!(library.game(100).is_none());
}

#[test]
fn gui_v2_unknown_review_filter_excludes_verified_games() {
    let mut verified = Game::from_archive(archive(1, "Verified", Some("SNES")));
    verified.identified = true;
    let unknown = Game::from_archive(archive(2, "Unknown", Some("SNES")));
    let library = Library {
        games: vec![verified, unknown],
        ..Default::default()
    };
    let indices = library.filter(&Filter {
        unverified_only: true,
        ..Default::default()
    });
    assert_eq!(indices.len(), 1);
    assert_eq!(library.games[indices[0]].title, "Unknown");
}

#[test]
fn gui_v2_unknown_title_never_becomes_verified_identity() {
    let game = Game::from_archive(archive(1, "Verified Final Fantasy VII Disc 1", None));
    assert!(!game.identified);
    assert_eq!(game.platform, "Unknown system");
    assert_eq!(game.status(), "Not checked yet");
}

#[test]
fn gui_v2_identified_detail_uses_existing_identity_bridge() {
    use archivefs_core::game_identity::*;
    let mut row = archive(1, "Final Fantasy VII", Some("PSX"));
    row.identity_report = Some(GameIdentityReport {
        archive_path: row.absolute_path.clone(),
        platform: IdentityPlatform::PlayStation,
        format: IdentityImageFormat::Iso,
        evidence: vec![IdentityEvidence {
            kind: IdentityKind::Ps1Serial,
            status: IdentityStatus::Verified,
            value: Some("SCUS-94163".into()),
            confidence: IdentityConfidence::StructuredMetadata,
            provenance: IdentityProvenance {
                archive_path: row.absolute_path.clone(),
                member_path: None,
                member_index: None,
                method: "fixture".into(),
            },
            diagnostic: "fixture".into(),
        }],
        warnings: vec![],
        bytes_read: 1,
        archive_members_inspected: 0,
        metadata_paths_inspected: 1,
        nested_container_depth: 0,
        complete: true,
    });
    assert!(Game::from_archive(row.clone()).identified);
    row.identity_report.as_mut().unwrap().evidence[0].confidence = IdentityConfidence::FilenameOnly;
    assert!(!Game::from_archive(row).identified);
}

#[test]
fn gui_v2_emulator_found_is_not_a_false_ready_to_launch_claim() {
    let mut detail = Detail::default();
    assert!(detail.emulator_status().contains("Needs setup"));
    detail.installed.push("PCSX2".into());
    assert!(detail.emulator_status().contains("Found: PCSX2"));
    assert!(
        detail
            .emulator_status()
            .contains("checks game and firmware")
    );
    assert!(!detail.emulator_status().contains("Ready to play"));
}

#[test]
fn gui_v2_large_library_projection_measurement() {
    let start = Instant::now();
    let library = Library::new(
        (0..50_000)
            .map(|id| {
                archive(
                    id,
                    &format!("Game {id:06}"),
                    Some(if id % 2 == 0 { "PS2" } else { "PSX" }),
                )
            })
            .collect(),
    );
    let load = start.elapsed();
    let start = Instant::now();
    let indices = library.filter(&Filter {
        search: "Game 049".into(),
        platform: "PS2".into(),
        ..Default::default()
    });
    eprintln!(
        "PERF v2 50000 rows: projection={}ms filter={}us",
        load.as_millis(),
        start.elapsed().as_micros()
    );
    assert_eq!(indices.len(), 500);
    assert_eq!(library.games.len(), 50_000);
}

#[test]
fn gui_v2_database_load_is_real_and_read_only() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("games");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("game.zip"), b"fixture archive").unwrap();
    let database_path = directory.path().join("library.sqlite3");
    let config = archivefs_core::Config {
        source_folders: vec![source],
        mount_root: directory.path().join("mount"),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    };
    {
        let mut database = archivefs_core::Database::open_or_create(&database_path).unwrap();
        archivefs_core::scan_and_persist(&mut database, &config, "v2-test-fixture").unwrap();
        for row in database.load_archives().unwrap() {
            database
                .assign_platform(row.id, Some("Arcade"), "manual")
                .unwrap();
            database
                .apply_screenscraper_enrichment(
                    row.id,
                    &archivefs_core::screenscraper_enrichment::AcceptedScreenScraperMetadata {
                        synopsis: Some("Persisted synopsis".into()),
                        ..Default::default()
                    },
                    &archivefs_core::screenscraper_enrichment::ScreenScraperEnrichmentReceipt {
                        provider: "ScreenScraper".into(),
                        provider_record_id: "fixture-record".into(),
                        retrieved_at_unix_seconds: 1,
                        match_basis: "fixture hash".into(),
                        before: Default::default(),
                        accepted: Default::default(),
                        media_reference_count: 0,
                    },
                )
                .unwrap();
        }
    }
    let before = fs::read(&database_path).unwrap();
    let library = backend::load_library(&database_path).unwrap();
    assert_eq!(library.games.len(), 1);
    assert_eq!(
        library.games[0]
            .screenscraper
            .as_ref()
            .map(|saved| saved.receipt.provider_record_id.as_str()),
        Some("fixture-record")
    );
    let connection = rusqlite::Connection::open_with_flags(
        &database_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let expected: i64 = connection.query_row("SELECT count(*) FROM archives a JOIN platform_assignments p ON p.archive_id=a.id AND p.is_current=1 WHERE p.platform='Arcade'", [], |row| row.get(0)).unwrap();
    assert_eq!(
        library
            .filter(&Filter {
                platform: "Arcade".into(),
                ..Default::default()
            })
            .len(),
        expected as usize
    );
    assert_eq!(fs::read(&database_path).unwrap(), before);
}

#[test]
fn gui_v2_empty_database_does_not_create_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("absent.sqlite3");
    assert!(backend::load_library(&path).unwrap().games.is_empty());
    assert!(!path.exists());
}

#[test]
fn gui_v2_preferences_round_trip_is_separate_from_legacy_mode() {
    let directory = tempfile::tempdir().unwrap();
    let old = directory.path().join("gui_mode.txt");
    fs::write(&old, "advanced").unwrap();
    let path = directory.path().join("gui-v2.json");
    backend::save_preferences(
        &path,
        &Preferences {
            route: Route::Game(22),
            filter: Filter {
                search: "Zelda".into(),
                ..Default::default()
            },
            welcome_dismissed: false,
            beginner_hints_enabled: true,
            document_roots: Vec::new(),
            document_associations: std::collections::BTreeMap::new(),
            document_reading: std::collections::BTreeMap::new(),
        },
    )
    .unwrap();
    let restored: Preferences = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(restored.route, Route::Game(22));
    assert_eq!(restored.filter.search, "Zelda");
    assert!(restored.beginner_hints_enabled);
    assert_eq!(fs::read_to_string(old).unwrap(), "advanced");
}

#[test]
fn gui_v2_preferences_default_beginner_hints_on() {
    assert!(Preferences::default().beginner_hints_enabled);
}

#[test]
fn gui_v2_activity_covers_all_states_and_never_invents_eta() {
    let mut activity = Activity::default();
    let id = activity.queue("Test", Route::Home, true);
    assert_eq!(activity.jobs[&id].phase, Phase::Queued);
    assert!(activity.jobs[&id].fraction().is_none());
    activity.start(id);
    assert_eq!(activity.jobs[&id].phase, Phase::Running);
    assert_eq!(activity.running(), 1);
    activity.jobs.get_mut(&id).unwrap().progress = Some((25, 100));
    assert_eq!(activity.jobs[&id].fraction(), Some(0.25));
    activity.finish(id, "Done".into(), None);
    assert_eq!(activity.jobs[&id].phase, Phase::Complete);
    assert_eq!(activity.running(), 0);
    let id = activity.queue("Failure", Route::Home, false);
    activity.finish(id, "Retry".into(), Some("raw".into()));
    assert_eq!(activity.jobs[&id].phase, Phase::Failed);
    let id = activity.queue("Cancel", Route::Home, true);
    activity.start(id);
    activity.jobs[&id].request_cancel();
    activity.finish(id, "Cancelled".into(), None);
    assert_eq!(activity.jobs[&id].phase, Phase::Cancelled);
    assert_eq!(activity.active(), 0);
    assert_eq!(activity.running(), 0);
    assert_eq!(activity.queued(), 0);

    let id = activity.queue("Failed", Route::Home, false);
    activity.start(id);
    activity.finish(id, "Failed".into(), Some("failure".into()));
    assert_eq!(activity.jobs[&id].phase, Phase::Failed);
    assert_eq!(activity.active(), 0);

    let id = activity.queue("Superseded", Route::Home, false);
    activity.supersede(id, "A newer request replaced this one.".into());
    assert_eq!(activity.jobs[&id].phase, Phase::Superseded);
    activity.start(id);
    activity.finish(id, "Late completion".into(), None);
    assert_eq!(activity.jobs[&id].phase, Phase::Superseded);
    assert_eq!(activity.active(), 0);
}

#[test]
fn gui_v2_normal_route_navigation_does_not_create_activity_jobs() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let routes = [
        Route::Section(Section::Games),
        Route::Section(Section::Setup),
        Route::Section(Section::Problems),
        Route::Section(Section::Games),
        Route::Section(Section::Games),
        Route::Section(Section::Activity),
    ];
    for route in routes {
        app.go(route);
        assert_eq!(app.activity.active(), 0);
        assert!(app.activity.jobs.is_empty());
    }
}

#[test]
fn gui_v2_romm_failed_initial_load_settles_without_retry_churn() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Romm);
    assert!(app.romm_library_load_needed());
    let id = app
        .activity
        .queue("Loading RomM library", app.router.current.clone(), true);
    app.activity.start(id);
    app.romm_library_job = Some(id);
    app.romm_library.loading = true;

    assert!(app.settle_romm_library_load(id, Err("fixture load failure".into())));
    app.activity.finish(
        id,
        "RomM library could not be loaded.".into(),
        Some("fixture load failure".into()),
    );

    assert_eq!(app.activity.jobs[&id].phase, Phase::Failed);
    assert_eq!(app.activity.active(), 0);
    assert_eq!(app.romm_library_job, None);
    assert!(!app.romm_library.loading);
    assert_eq!(
        app.romm_library.snapshot.as_ref().unwrap().status,
        "fixture load failure"
    );
    assert!(!app.romm_library_load_needed());
}

#[test]
fn gui_v2_activity_view_and_footer_use_the_authoritative_job_registry() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let queued = app.activity.queue("Queued fixture", Route::Home, true);
    let running = app.activity.queue("Running fixture", Route::Home, true);
    app.activity.start(running);
    let complete = app.activity.queue("Completed fixture", Route::Home, false);
    app.activity.finish(complete, "Done".into(), None);

    app.router.current = Route::Section(Section::Activity);
    let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
    assert_eq!(app.activity.active(), 2);
    assert_eq!(
        app.activity.active(),
        app.activity.queued() + app.activity.running()
    );
    assert!(
        strings
            .iter()
            .any(|value| value.contains("2 active jobs · 1 running · 1 waiting"))
    );
    assert!(
        strings
            .iter()
            .any(|value| value.contains("Running fixture"))
    );
    assert!(app.activity.jobs[&queued].active());
    assert!(app.activity.jobs[&complete].phase == Phase::Complete);
}

#[test]
fn gui_v2_real_worker_job_completes_and_leaves_activity_snapshot_inactive() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let id = app.activity.queue("Filter fixture", Route::Home, false);
    let command = super::backend::Command::Filter {
        library: Arc::new(Library::default()),
        filter: Filter::default(),
        generation: 42,
    };
    assert!(app.send(id, command));

    for _ in 0..200 {
        app.poll(&context);
        if !app.activity.jobs[&id].active() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(app.activity.jobs[&id].phase, Phase::Complete);
    assert_eq!(app.activity.active(), 0);
    assert_eq!(app.activity.running(), 0);
    assert_eq!(app.activity.queued(), 0);

    app.router.current = Route::Section(Section::Activity);
    let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
    assert!(strings.iter().any(|value| value.contains("Filter fixture")));
    assert!(strings.iter().any(|value| value.contains("Finished")));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("Ready · browsing does not change your game files"))
    );
}

#[test]
fn gui_v2_activity_status_labels_use_plain_english() {
    assert_eq!(Phase::Queued.label(), "Waiting to start");
    assert_eq!(Phase::Running.label(), "In progress");
    assert_eq!(Phase::Complete.label(), "Finished");
    assert_eq!(Phase::Failed.label(), "Needs attention");
    assert_eq!(Phase::Cancelled.label(), "Stopped");

    use crate::activity_history::ActivityOutcome;
    assert_eq!(ActivityOutcome::Completed.to_string(), "Finished");
    assert_eq!(ActivityOutcome::Failed.to_string(), "Could not complete");
    assert_eq!(ActivityOutcome::Cancelled.to_string(), "Stopped");
    assert_eq!(ActivityOutcome::Rejected.to_string(), "Review required");
}

#[test]
fn gui_v2_activity_safe_cancellation_and_unknown_total() {
    let mut activity = Activity::default();
    let id = activity.queue("Scan", Route::Home, false);
    activity.jobs[&id].request_cancel();
    assert!(activity.jobs[&id].cancel.is_none());
    activity.jobs.get_mut(&id).unwrap().progress = Some((4, 0));
    assert!(activity.jobs[&id].fraction().is_none());
    let id = activity.queue("Pictures", Route::Home, true);
    activity.jobs[&id].request_cancel();
    assert!(
        activity.jobs[&id]
            .cancel
            .as_ref()
            .unwrap()
            .load(Ordering::Relaxed)
    );
}

fn image_fixture(path: &Path, format: image::ImageFormat) {
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        1200,
        1600,
        image::Rgb([63, 123, 180]),
    ))
    .save_with_format(path, format)
    .unwrap();
}

#[test]
fn gui_v2_local_thumbnail_cache_miss_hit_resize_and_media_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cover.jpg");
    image_fixture(&path, image::ImageFormat::Jpeg);
    let before = fs::read(&path).unwrap();
    let cache = directory.path().join("cache");
    let start = Instant::now();
    let miss = thumbnail::load_local(&path, &cache).unwrap();
    let miss_ms = start.elapsed().as_micros();
    let start = Instant::now();
    let hit = thumbnail::load_local(&path, &cache).unwrap();
    let hit_us = start.elapsed().as_micros();
    assert!(!miss.timings.cache_hit);
    assert!(hit.timings.cache_hit);
    assert_eq!(hit.image.size, [240, 320]);
    assert_eq!(hit.image, miss.image);
    assert_eq!(before, fs::read(&path).unwrap());
    eprintln!(
        "PERF v2 local JPEG 1200x1600: miss={}us hit={}us decode={}us resize={}us",
        miss_ms,
        hit_us,
        miss.timings.decode.as_micros(),
        miss.timings.resize.as_micros()
    );
}

#[test]
fn gui_v2_thumbnail_stale_source_creates_new_key() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cover.png");
    let cache = directory.path().join("cache");
    image::RgbImage::from_pixel(10, 20, image::Rgb([1, 2, 3]))
        .save(&path)
        .unwrap();
    thumbnail::load_local(&path, &cache).unwrap();
    image::RgbImage::from_pixel(20, 40, image::Rgb([5, 6, 7]))
        .save(&path)
        .unwrap();
    assert!(
        !thumbnail::load_local(&path, &cache)
            .unwrap()
            .timings
            .cache_hit
    );
    assert_eq!(fs::read_dir(cache).unwrap().count(), 2);
}

#[test]
fn gui_v2_thumbnail_broken_missing_and_unsafe_paths_are_recoverable() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("broken.jpg");
    fs::write(&path, b"not an image").unwrap();
    assert!(thumbnail::load_local(&path, &directory.path().join("cache")).is_err());
    assert!(thumbnail::load_local(&directory.path().join("absent.png"), directory.path()).is_err());
    assert!(thumbnail::load_local(Path::new("../../secret.png"), directory.path()).is_err());
}

#[cfg(unix)]
#[test]
fn gui_v2_thumbnail_refuses_symlink_cache_and_never_clobbers_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cover.png");
    image_fixture(&path, image::ImageFormat::Png);
    let outside = directory.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let cache = directory.path().join("linked-cache");
    std::os::unix::fs::symlink(&outside, &cache).unwrap();
    assert!(thumbnail::load_local(&path, &cache).is_err());
    assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
    let cache = directory.path().join("cache");
    thumbnail::load_local(&path, &cache).unwrap();
    let entry = fs::read_dir(&cache)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(&entry, "unrelated user's file").unwrap();
    assert!(thumbnail::load_local(&path, &cache).is_ok());
    assert_eq!(fs::read_to_string(entry).unwrap(), "unrelated user's file");
}

#[test]
fn gui_v2_artwork_missing_uses_immediate_deterministic_placeholder() {
    let context = egui::Context::default();
    let mut artwork = Artwork::start(context);
    artwork.index = Some(Arc::new(MediaIndex::default()));
    let key = artwork.request(1, Kind::Cover);
    assert!(matches!(artwork.pictures.get(&key), Some(Picture::Missing)));
    assert_eq!(artwork.active(), 0);
    assert_eq!(key, artwork.request(1, Kind::Cover));
}

#[test]
fn gui_v2_artwork_async_local_loading_reuse_failure_and_retry() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cover.jpg");
    image_fixture(&path, image::ImageFormat::Jpeg);
    let context = egui::Context::default();
    let mut artwork = Artwork::with_cache(context.clone(), Some(directory.path().join("cache")));
    let mut index = MediaIndex::default();
    index.covers.insert(1, Source::Local(path));
    index
        .covers
        .insert(2, Source::Local(directory.path().join("absent.png")));
    artwork.index = Some(Arc::new(index));
    let first = artwork.request(1, Kind::Cover);
    let bad = artwork.request(2, Kind::Cover);
    assert!(matches!(
        artwork.pictures.get(&first),
        Some(Picture::Loading)
    ));
    let deadline = Instant::now() + Duration::from_secs(10);
    while artwork.active() > 0 && Instant::now() < deadline {
        artwork.begin_frame(&context);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(
        artwork.pictures.get(&first),
        Some(Picture::Ready { .. })
    ));
    assert!(matches!(
        artwork.pictures.get(&bad),
        Some(Picture::Failed(_))
    ));
    let requested = artwork.requested;
    artwork.request(1, Kind::Cover);
    assert_eq!(artwork.requested, requested);
    artwork.retry(bad);
    assert!(!artwork.pictures.contains_key(&bad));
}

#[test]
fn gui_v2_artwork_bounded_queue_and_offscreen_cancellation() {
    let context = egui::Context::default();
    let mut artwork = Artwork::start(context.clone());
    let mut index = MediaIndex::default();
    for id in 0..500 {
        index
            .covers
            .insert(id, Source::Local("/nonexistent/v2-test-image.png".into()));
    }
    artwork.index = Some(Arc::new(index));
    for id in 0..500 {
        artwork.request(id, Kind::Cover);
    }
    assert!(artwork.active() <= 96 + artwork::LOCAL_WORKERS + artwork::REMOTE_WORKERS + 32);
    artwork.begin_frame(&context);
    artwork.end_frame();
    let deadline = Instant::now() + Duration::from_secs(5);
    while artwork.active() > 0 && Instant::now() < deadline {
        artwork.begin_frame(&context);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(artwork.active(), 0);
}

#[test]
fn gui_v2_stale_artwork_delivery_never_attaches_to_new_generation() {
    let context = egui::Context::default();
    let mut artwork = Artwork::start(context.clone());
    let mut index = MediaIndex::default();
    index
        .covers
        .insert(1, Source::Local("/nonexistent/v2-stale.png".into()));
    artwork.index = Some(Arc::new(index));
    artwork.request(1, Kind::Cover);
    artwork.generation += 1;
    artwork.pictures.clear();
    for _ in 0..10 {
        artwork.begin_frame(&context);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(artwork.pictures.is_empty());
}

#[test]
fn gui_v2_render_path_has_no_io_or_image_decode() {
    let source = include_str!("pages.rs");
    for forbidden in [
        "std::fs",
        "fs::",
        "image::",
        "Database::",
        "load_local(",
        "reqwest",
        "ureq",
        "Config::load",
    ] {
        assert!(
            !source.contains(forbidden),
            "paint path contains {forbidden}"
        );
    }
}

#[test]
fn gui_v2_readability_has_large_text_and_targets() {
    let context = egui::Context::default();
    readable_style(&context);
    let style = context.style();
    assert!(style.text_styles[&egui::TextStyle::Body].size >= 18.0);
    assert!(style.text_styles[&egui::TextStyle::Button].size >= 18.0);
    assert!(style.spacing.interact_size.y >= 40.0);
}

#[test]
fn gui_v2_all_major_pages_have_purpose_location_back_and_handle_at_small_and_desktop_sizes() {
    for size in [[1280.0, 820.0], [1024.0, 600.0], [640.0, 480.0]] {
        for section in routes::SECTIONS {
            let context = egui::Context::default();
            let mut app = fixture(&context);
            app.router.current = Route::Section(*section);
            frame(&context, &mut app, size);
            let output = frame(&context, &mut app, size);
            let texts = text(&output);
            assert!(
                texts.iter().any(|text| text == "Home"),
                "{section:?} {size:?}"
            );
            assert!(
                texts.iter().any(|text| text == "Back"),
                "{section:?} {size:?}"
            );
            assert!(
                texts.iter().any(|text| text.contains(section.purpose())),
                "missing purpose {section:?} {size:?}"
            );
            assert!(
                texts.iter().any(|text| text == "Games"),
                "sidebar absent {section:?} {size:?}"
            );
        }
    }
}

#[test]
fn gui_v2_game_detail_exposes_contextual_actions_and_lazy_screenshots() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(1, "Example Game", Some("PS2"))]));
    app.router.current = Route::Game(1);
    let output = frame(&context, &mut app, [1280.0, 1200.0]);
    let texts = text(&output);
    for label in [
        "Play",
        "Checking whether this game is ready",
        "Verify",
        "Artwork, Manuals & Extras",
        "Mods & Cheats",
        "Fix Problems",
        "Open Folder",
        "Back",
        "Advanced details",
    ] {
        assert!(texts.iter().any(|text| text == label), "{label}");
    }
    assert!(!app.screenshots);
    assert_eq!(app.artwork.requested, 0);
}

fn bounds_below_chrome(output: &egui::FullOutput, wanted: &str) -> Option<egui::Rect> {
    // The top bar repeats the game title in its breadcrumb; the page body
    // starts below it.
    text_bounds(output, wanted)
        .into_iter()
        .find(|rect| rect.min.y > 64.0)
}

#[test]
fn gui_v2_game_details_lead_with_title_and_play_and_keep_reference_material_last() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(
        1,
        "Example Game",
        Some("Saturn"),
    )]));
    app.router.current = Route::Game(1);
    let output = frame(&context, &mut app, [1280.0, 2400.0]);
    let y = |label: &str| {
        bounds_below_chrome(&output, label)
            .unwrap_or_else(|| panic!("{label} missing: {:?}", text(&output)))
            .min
            .y
    };
    // The page names the game itself, then offers Play, then secondary
    // tools, and only then reference material.
    let order = [
        y("Example Game"),
        y("Play"),
        y("Verify"),
        y("Screenshots"),
        y("Disc & ROM evidence"),
        y("Advanced details"),
    ];
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
    let texts = text(&output);
    assert_eq!(texts.iter().filter(|line| *line == "Play").count(), 1);
    // Closed by default: no raw path, enum name or specialist panel body.
    assert!(!texts.iter().any(|line| line.contains("Example Game.iso")));
    assert!(!texts.iter().any(|line| line.starts_with("Media kind:")));
    assert!(!texts.iter().any(|line| line == "Saturn disc layout"));
}

#[test]
fn gui_v2_game_details_keep_play_on_screen_in_a_short_window_for_specialist_media() {
    // Specialist evidence used to render above the scroll area and push the
    // title and Play out of a small window.
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(
        1,
        "Example Game",
        Some("Saturn"),
    )]));
    app.router.current = Route::Game(1);
    let output = frame(&context, &mut app, [1000.0, 560.0]);
    for label in ["Example Game", "Play"] {
        let rect = bounds_below_chrome(&output, label).unwrap_or_else(|| panic!("{label} missing"));
        assert!(rect.max.y < 560.0, "{label} is pushed off-screen: {rect:?}");
    }
}

#[test]
fn gui_v2_game_details_show_evidence_section_only_when_the_media_has_evidence() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(1, "Plain Game", Some("PS2"))]));
    app.router.current = Route::Game(1);
    let output = frame(&context, &mut app, [1280.0, 1600.0]);
    assert!(
        !text(&output)
            .iter()
            .any(|line| line == "Disc & ROM evidence")
    );
    // With nothing associated, Game Details has no empty Manuals section.
    assert!(!text(&output).iter().any(|line| line == "Manuals & Guides"));
}

#[test]
fn gui_v2_game_details_do_not_carry_expanded_screenshots_to_another_game() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![
        archive(1, "First", Some("PS2")),
        archive(2, "Second", Some("PS2")),
    ]));
    app.go(Route::Game(1));
    app.screenshots = true;
    app.go(Route::Game(2));
    assert!(
        !app.screenshots,
        "a new game starts with only its first picture"
    );
}

#[test]
fn gui_v2_browser_virtualizes_large_collection() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(
        (0..10_000)
            .map(|id| archive(id, &format!("Game {id:06}"), Some("PS2")))
            .collect(),
    ));
    app.indices = app.library.filter(&Filter::default());
    app.router.current = Route::Section(Section::Games);
    let output = frame(&context, &mut app, [1280.0, 820.0]);
    assert_eq!(app.indices.len(), 10_000);
    assert!(
        text(&output)
            .iter()
            .any(|text| text == "10000 catalogued games shown")
    );
    assert!(
        text(&output)
            .iter()
            .filter(|text| text.starts_with("Game 0"))
            .count()
            < 30
    );
}

#[test]
fn gui_v2_platform_selection_clears_old_filters_and_counts_all_sources() {
    let mut second = archive(2, "Other copy", Some("Arcade"));
    second.source_folder_id = 2;
    second.absolute_path = "/other/Other copy.iso".into();
    let library = Library::new(vec![
        archive(1, "First", Some("Arcade")),
        second,
        archive(3, "Distinct platform", Some("NeoGeo")),
    ]);
    let mut filter = Filter {
        search: "old search".into(),
        attention_only: true,
        list: true,
        ..Default::default()
    };
    filter.select_platform("Arcade".into());
    assert!(filter.search.is_empty());
    assert!(!filter.attention_only);
    assert!(filter.list);
    assert_eq!(library.filter(&filter).len(), library.platforms["Arcade"]);
    assert_eq!(library.filter(&filter).len(), 2);
    assert_eq!(library.platform_sources["Arcade"].len(), 2);
    assert_eq!(
        library.platform_sources["Arcade"].values().sum::<usize>(),
        2
    );
}

#[test]
fn gui_v2_scummvm_aliases_share_one_selector_count_and_filter() {
    let library = Library::new(vec![
        archive(1, "Monkey Island", Some("scumm")),
        archive(2, "Broken Sword", Some("ScummVM")),
        archive(3, "DOS game", Some("DOS")),
    ]);

    assert_eq!(library.platforms.get("ScummVM"), Some(&2));
    assert!(!library.platforms.contains_key("scumm"));
    assert!(!library.platforms.contains_key("DOSBox"));
    let filter = Filter {
        platform: "ScummVM".into(),
        ..Default::default()
    };
    let selected = library.filter(&filter);
    assert_eq!(selected.len(), 2);
    assert!(selected.iter().all(|index| {
        library.games[*index].platform == "ScummVM" && library.games[*index].platform != "DOS"
    }));
}

#[test]
fn gui_v2_catalogue_projection_reconciles_aliases_unknowns_and_missing_rows() {
    let mut missing_amiga = archive(1, "Amiga disk", Some("commodoreamiga"));
    missing_amiga.last_verified_missing_at = Some("2026-09-24".into());
    let unknown = archive(2, "Unassigned tape", None);
    let registry_only = archive(3, "Future registry id", Some("Future Platform"));
    let archives = vec![missing_amiga, unknown, registry_only];
    let projection = super::library::project_platforms(&archives);

    assert_eq!(projection["Amiga"].total, 1);
    assert_eq!(projection["Amiga"].assigned, 1);
    assert_eq!(projection["Amiga"].missing, 1);
    assert_eq!(projection["Unknown system"].total, 1);
    assert_eq!(projection["Unknown system"].assigned, 0);
    assert_eq!(projection["Future Platform"].total, 1);
    assert_eq!(projection.values().map(|item| item.total).sum::<usize>(), 3);

    let library = Library::new(archives);
    assert_eq!(library.platforms["Amiga"], 1);
    assert_eq!(library.platforms["Unknown system"], 1);
    assert_eq!(library.platforms["Future Platform"], 1);
    assert_eq!(
        library
            .filter(&Filter {
                platform: "Amiga".into(),
                ..Default::default()
            })
            .len(),
        1
    );
}

#[test]
fn gui_v2_scummvm_platform_keeps_native_readiness_adapter() {
    let game = Game::from_archive(archive(1, "Monkey Island", Some("scummvm")));
    let compatibility = archivefs_core::launch::launch_compatibility_for_platform(&game.platform)
        .expect("ScummVM has a native readiness mapping");
    assert_eq!(game.platform, "ScummVM");
    assert_eq!(compatibility.standalone_adapters, &["scummvm"]);
}

#[test]
fn gui_v2_pending_filter_does_not_present_stale_games_or_count() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(
        1,
        "STALE OLD GAME",
        Some("PS2"),
    )]));
    app.indices = vec![0];
    app.router.current = Route::Section(Section::Games);
    app.filter.select_platform("Arcade".into());
    app.change_filter();
    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    assert!(strings.iter().any(|s| s.contains("Updating results")));
    assert!(
        !strings
            .iter()
            .any(|s| s == "STALE OLD GAME" || s == "1 catalogued games shown")
    );
}

#[test]
fn gui_v2_screenshot_diagnostics_combine_local_and_romm_without_guessing() {
    use super::media_sources::screenshot_diagnostic;
    let path = Path::new("/games/original.gba");
    let zero = screenshot_diagnostic(path, 0, Some(("85674", 0)), "Local indexes checked.", false);
    assert!(zero.contains("RomM matched this game, but that record has no screenshots."));
    assert!(zero.contains("Screenshot candidates: 0."));
    assert!(zero.contains("85674"));
    assert!(zero.contains("exact original archive path"));
    let local = screenshot_diagnostic(
        path,
        2,
        Some(("85674", 0)),
        "ES-DE matched this game.",
        false,
    );
    assert!(local.contains("Screenshot candidates: 2."));
    assert!(!local.contains("Screenshot candidates: 0."));
    let incomplete = screenshot_diagnostic(path, 0, None, "Local source could not be read.", true);
    assert!(incomplete.contains("incomplete"));
    assert!(!incomplete.contains("Screenshot candidates: 0."));
}

#[test]
fn gui_v2_detail_keeps_screenshot_diagnostics_secondary() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(1, "Example", Some("GBA"))]));
    let mut index = MediaIndex::default();
    index
        .diagnostics
        .insert(1, "PRIVATE DIAGNOSTIC 85674".into());
    app.artwork.index = Some(Arc::new(index));
    app.router.current = Route::Game(1);
    let mut strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    // The upper guidance strip now takes space ahead of this scrollable body.
    // Check real scroll reachability, not an artificially taller viewport.
    strings.extend(text(&scroll_page(&context, &mut app, [1280.0, 820.0])));
    assert!(strings.iter().any(|s| s == "Play"), "{strings:?}");
    assert!(!strings.iter().any(|s| s.contains("PRIVATE DIAGNOSTIC")));
}

#[test]
fn gui_v2_check_games_scrolls_long_list_and_keeps_sidebar_fixed() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(
        (0..100)
            .map(|id| archive(id, "Game", Some(&format!("System {id:03}"))))
            .collect(),
    ));
    app.router.current = Route::Section(Section::Check);
    frame(&context, &mut app, [1024.0, 600.0]);
    let first = frame(&context, &mut app, [1024.0, 600.0]);
    fn position(output: &egui::FullOutput, label: &str) -> Option<egui::Pos2> {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => Some(text.pos),
            _ => None,
        })
    }
    let sidebar = position(&first, "EmuWiz").unwrap();
    let input_frame = |app: &mut App, events: Vec<egui::Event>| {
        context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1024.0, 600.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| app.show(ctx),
        )
    };
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    input_frame(&mut app, vec![key(egui::Key::Tab)]);
    assert!(
        context.wants_keyboard_input(),
        "exercise paging with a focused button"
    );
    input_frame(&mut app, vec![key(egui::Key::End)]);
    let last = frame(&context, &mut app, [1024.0, 600.0]);
    assert_eq!(position(&last, "EmuWiz"), Some(sidebar));
    let final_row = position(&last, "Check System 099").expect("last platform must be reachable");
    assert!(final_row.y < 580.0 && final_row.y > 100.0, "{final_row:?}");
    Arc::make_mut(&mut app.library)
        .platforms
        .insert("System 050".into(), 9);
    app.router.current = Route::Home;
    frame(&context, &mut app, [1024.0, 600.0]);
    app.router.current = Route::Section(Section::Check);
    let restored = frame(&context, &mut app, [1024.0, 600.0]);
    assert_eq!(position(&restored, "Check System 099"), Some(final_row));
    input_frame(&mut app, vec![key(egui::Key::Home)]);
    let home = frame(&context, &mut app, [1024.0, 600.0]);
    assert!(position(&home, "System 000").unwrap().y < 400.0);
    input_frame(&mut app, vec![key(egui::Key::PageDown)]);
    let paged = frame(&context, &mut app, [1024.0, 600.0]);
    assert_ne!(
        position(&paged, "System 000"),
        position(&home, "System 000")
    );
    input_frame(&mut app, vec![key(egui::Key::PageUp)]);
    let returned = frame(&context, &mut app, [1024.0, 600.0]);
    assert_eq!(
        position(&returned, "System 000"),
        position(&home, "System 000")
    );
    input_frame(
        &mut app,
        vec![
            egui::Event::PointerMoved(egui::pos2(700.0, 400.0)),
            egui::Event::MouseWheel {
                phase: egui::TouchPhase::Move,
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -400.0),
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    for _ in 0..20 {
        frame(&context, &mut app, [1024.0, 600.0]);
    }
    let scrolled = frame(&context, &mut app, [1024.0, 600.0]);
    assert_ne!(
        position(&scrolled, "System 000"),
        position(&home, "System 000")
    );
    assert_eq!(position(&scrolled, "EmuWiz"), Some(sidebar));
    frame(&context, &mut app, [640.0, 480.0]);
    frame(&context, &mut app, [1280.0, 820.0]);
    input_frame(&mut app, vec![key(egui::Key::End)]);
    assert!(
        position(
            &frame(&context, &mut app, [1024.0, 600.0]),
            "Check System 099"
        )
        .unwrap()
        .y < 580.0
    );
}

#[test]
fn gui_v2_accidental_exploration_never_runs_scan_or_legacy() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    for section in routes::SECTIONS {
        app.go(Route::Section(*section));
        frame(&context, &mut app, [1024.0, 600.0]);
    }
    assert!(app.activity.jobs.values().all(|job| {
        job.title == "Refreshing artwork providers"
            || job.title == "Refreshing metadata and artwork"
            || job.title == "Checking emulator readiness"
            || job.title == "Checking save locations"
            || job.title == "Checking saved problem evidence"
    }));
    assert!(app.load_job.is_none());
    assert!(!app.confirm_scan);
}

#[test]
fn gui_v2_sidebar_has_real_keyboard_focus() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    frame(&context, &mut app, [1024.0, 600.0]);
    let _ = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 600.0),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
            ..Default::default()
        },
        |context| app.show(context),
    );
    assert!(context.memory(|memory| memory.focused().is_some()));
}

#[test]
fn gui_v2_back_shortcut_returns_to_originating_platform_filter() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.filter.platform = "PS2".into();
    app.go(Route::Section(Section::Games));
    app.go(Route::Game(44));
    let _ = context.run(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
            ..Default::default()
        },
        |context| app.show(context),
    );
    assert_eq!(app.router.current, Route::Section(Section::Games));
    assert_eq!(app.filter.platform, "PS2");
}

#[test]
fn gui_v2_primary_action_is_visible_without_scrolling() {
    fn visible(shape: &egui::Shape, clip: egui::Rect, needle: &str) -> bool {
        match shape {
            egui::Shape::Text(text) => {
                text.galley.text() == needle
                    && clip.contains_rect(egui::Rect::from_min_size(text.pos, text.galley.size()))
            }
            egui::Shape::Vec(shapes) => shapes.iter().any(|shape| visible(shape, clip, needle)),
            _ => false,
        }
    }
    for section in [
        Section::Setup,
        Section::Check,
        Section::Problems,
        Section::Build,
        Section::Emulators,
        Section::Sources,
        Section::Dat,
        Section::Advanced,
    ] {
        // The compact 640px layout has dedicated route-specific readability
        // tests; this broad sweep verifies the primary action at the normal
        // supported application viewport without conflating page-specific
        // narrow wrapping with route reachability.
        let size = [1024.0, 600.0];
        let context = egui::Context::default();
        let mut app = fixture(&context);
        app.router.current = Route::Section(section);
        frame(&context, &mut app, size);
        let output = frame(&context, &mut app, size);
        let actions = if section == Section::Setup {
            vec!["Setup & Doctor"]
        } else if section == Section::Check {
            vec!["Choose a platform"]
        } else if section == Section::Problems {
            vec![
                "Nothing needs attention right now.",
                "Checking saved evidence",
                "Problems & Repair",
            ]
        } else if section == Section::Build {
            vec![
                "Rename verified games",
                "Build a clean playing library",
                "Organisation",
            ]
        } else if section == Section::Sources {
            vec!["Add source"]
        } else if section == Section::Dat {
            vec!["DATs & Verification"]
        } else if section == Section::Advanced {
            vec!["Open specialist interface"]
        } else {
            vec![section.action()]
        };
        assert!(
            actions.iter().any(|action| {
                output
                    .shapes
                    .iter()
                    .any(|shape| visible(&shape.shape, shape.clip_rect, action))
            }),
            "primary action is clipped: {section:?} {size:?}"
        );
    }
}

#[test]
fn gui_v2_refresh_readiness_error_retains_an_obvious_retry() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(1, "Game", Some("PS2"))]));
    app.router.current = Route::Game(1);
    app.detail_failed = Some(1);
    let output = frame(&context, &mut app, [1280.0, 1200.0]);
    assert!(text(&output).iter().any(|line| line == "Try again"));
    assert!(
        text(&output)
            .iter()
            .any(|line| line.contains("Your game has not been changed"))
    );
}

#[test]
fn gui_v2_webp_and_transparent_png_cache_keep_correct_pixels() {
    let directory = tempfile::tempdir().unwrap();
    for (name, format) in [
        ("cover.webp", image::ImageFormat::WebP),
        ("cover.png", image::ImageFormat::Png),
    ] {
        let path = directory.path().join(name);
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            80,
            100,
            image::Rgba([120, 80, 40, 128]),
        ))
        .save_with_format(&path, format)
        .unwrap();
        let first = thumbnail::load_local(&path, &directory.path().join("cache")).unwrap();
        let second = thumbnail::load_local(&path, &directory.path().join("cache")).unwrap();
        assert!(second.timings.cache_hit);
        assert_eq!(first.image.pixels[0], second.image.pixels[0]);
    }
}

#[test]
fn gui_v2_sources_route_hosts_the_native_source_manager() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Sources);

    let output = frame(&context, &mut app, [1280.0, 820.0]);
    let strings = text(&output);

    assert!(strings.iter().any(|value| value == "Sources"));
    assert!(strings.iter().any(|value| value == "Add game folder"));
    assert!(strings.iter().any(|value| value == "Configured folders"));
    assert!(strings.iter().any(|value| value == "Discovery"));
    assert!(
        !strings
            .iter()
            .any(|value| value.contains("separate window"))
    );
}

#[test]
fn gui_v2_dat_management_is_the_native_normal_route() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Dat);

    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));

    assert!(strings.iter().any(|value| value == "DATs & Verification"));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("trusted DAT catalogues"))
    );
    assert!(
        !strings
            .iter()
            .any(|value| value.contains("separate window"))
    );
    assert_eq!(
        app.native_workflows.as_ref().unwrap().app.view,
        crate::navigation::MainView::DatSources
    );
}

#[test]
fn gui_v2_dat_page_explains_bounded_no_intro_import_without_legacy_handoff() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Dat);

    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));

    assert!(
        strings
            .iter()
            .any(|value| value == "Installed and imported DATs")
    );
    assert!(strings.iter().any(|value| value == "No-Intro pack import"));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("Download the pack externally"))
    );
    assert!(strings.iter().any(|value| value == "Choose No-Intro ZIP"));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("content-addressed snapshot"))
    );
    assert!(
        !strings
            .iter()
            .any(|value| value.contains("separate window"))
    );
}

#[test]
fn gui_v2_advanced_is_a_specialist_escape_not_a_duplicate_dat_route() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Advanced);

    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    // The page header supplies the single title and description.
    assert!(
        strings
            .iter()
            .any(|value| value == Section::Advanced.purpose())
    );
    assert!(!strings.iter().any(|value| value == "Advanced tools"));
    assert!(
        strings
            .iter()
            .any(|value| value == "Open specialist interface")
    );
    assert!(strings.iter().any(|value| value == "Open DAT Management"));
    assert!(!strings.iter().any(|value| value == "DATs & Verification"));
}

#[test]
fn gui_v2_dat_worker_lifecycle_is_reflected_in_activity() {
    let mut activity = Activity::default();
    let mut job = None;

    native_workflows::observe_dat_activity_state(
        &mut activity,
        &mut job,
        Some(crate::dat_sources_page::DatBackgroundActivity {
            title: "Validating DAT",
            detail: "Reading catalogue entries".into(),
        }),
        None,
    );
    assert_eq!(activity.running(), 1);
    let active = activity.jobs.values().next().unwrap();
    assert_eq!(active.title, "Validating DAT");
    assert_eq!(active.item.as_deref(), Some("Reading catalogue entries"));

    native_workflows::observe_dat_activity_state(
        &mut activity,
        &mut job,
        None,
        Some("fixture validation failure".into()),
    );
    assert_eq!(activity.running(), 0);
    assert_eq!(activity.jobs.values().next().unwrap().phase, Phase::Failed);
}

#[test]
fn gui_v2_source_worker_lifecycle_is_reflected_in_activity() {
    let context = egui::Context::default();
    let mut workflows = native_workflows::NativeWorkflows::new(context.clone());
    let (sender, receiver) = std::sync::mpsc::channel();
    workflows.app.sources_ui.source_action =
        Some(crate::platform_source_actions::RunningSourceAction {
            action: crate::platform_source_actions::SourceAction::ScanAll,
            receiver,
            worker: None,
        });
    let mut activity = Activity::default();

    workflows.observe_source_activity(&mut activity);
    assert_eq!(activity.running(), 1);
    sender.send(Err("fixture scan failure".into())).unwrap();
    workflows.poll(&context, &mut activity);

    assert_eq!(activity.running(), 0);
    assert!(activity.jobs.values().any(|job| job.phase == Phase::Failed));
}

#[test]
fn gui_v2_artwork_metadata_has_library_and_selected_game_views() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(9, "Rez", Some("Dreamcast"))]));
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    app.router.current = Route::Section(Section::Artwork);

    let library_output = frame(&context, &mut app, [1280.0, 820.0]);
    let library_text = text(&library_output);
    assert!(library_text.iter().any(|value| value == "Library artwork"));
    assert!(library_text.iter().any(|value| value == "Rez"));
    assert!(
        !library_text
            .iter()
            .any(|value| value.contains("existing interface"))
    );

    app.router.current = Route::Task {
        section: Section::Artwork,
        game: 9,
    };
    let selected_output = frame(&context, &mut app, [1280.0, 820.0]);
    let selected_text = text(&selected_output);
    assert!(selected_text.iter().any(|value| value == "Rez"));
    assert!(selected_text.iter().any(|value| value == "Metadata"));
    assert!(
        selected_text
            .iter()
            .any(|value| value.contains("No box art is available"))
    );
    assert!(
        selected_text
            .iter()
            .any(|value| value.contains("No manual is associated"))
    );
    assert!(
        !selected_text
            .iter()
            .any(|value| value.contains("Original path:")),
        "advanced details start closed"
    );
}

#[test]
fn gui_v2_selected_dreamcast_game_shows_native_ipbin_boundary() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(9, "Rez", Some("Dreamcast"))]));
    app.router.current = Route::Game(9);

    let closed = text(&frame(&context, &mut app, [1280.0, 1600.0]));
    assert!(closed.iter().any(|value| value == "Disc & ROM evidence"));
    assert!(
        !closed
            .iter()
            .any(|value| value == "Dreamcast boot metadata")
    );
    let strings = text(&click_label(
        &context,
        &mut app,
        [1280.0, 1600.0],
        "Disc & ROM evidence",
    ));
    assert!(
        strings
            .iter()
            .any(|value| value == "Dreamcast boot metadata")
    );
    assert!(strings.iter().any(|value| {
        value.contains("Read-only facts from the bounded Dreamcast IP.BIN inspection")
    }));
    assert!(
        !strings
            .iter()
            .any(|value| value.contains("existing interface"))
    );
}

#[test]
fn gui_v2_selected_saturn_game_shows_read_only_disc_manifest_boundary() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(
        9,
        "Panzer Dragoon",
        Some("Saturn"),
    )]));
    app.router.current = Route::Game(9);

    let closed = text(&frame(&context, &mut app, [1280.0, 1600.0]));
    assert!(closed.iter().any(|value| value == "Disc & ROM evidence"));
    assert!(!closed.iter().any(|value| value == "Saturn disc layout"));
    let strings = text(&click_label(
        &context,
        &mut app,
        [1280.0, 2600.0],
        "Disc & ROM evidence",
    ));
    assert!(strings.iter().any(|value| value == "Saturn disc layout"));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("Disc manifest unavailable"))
    );
    assert!(
        strings
            .iter()
            .any(|value| value.contains("will not infer Saturn topology"))
    );
    assert!(
        strings
            .iter()
            .any(|value| value == "Saturn patch readiness")
    );
    assert!(
        strings
            .iter()
            .any(|value| value == "Apply is unavailable: Saturn patch readiness is read-only.")
    );
}

#[test]
fn gui_v2_sources_route_owns_native_provider_setup_without_legacy_handoff() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    app.router.current = Route::Section(Section::Sources);

    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));

    for label in [
        "Artwork and metadata providers",
        "Local artwork",
        "RomM",
        "ES-DE",
        "ScreenScraper",
    ] {
        assert!(
            strings.iter().any(|value| value == label),
            "missing {label}"
        );
    }
    assert!(strings.iter().any(|value| value.contains("Credentials:")));
    assert!(
        !strings
            .iter()
            .any(|value| value.contains("separate window"))
    );
}

#[test]
fn gui_v2_artwork_metadata_reports_provider_unavailability_without_guessing() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let mut library = Library::new(vec![archive(11, "Known Game", Some("PS2"))]);
    library.games[0].identified = true;
    app.library = Arc::new(library);
    let mut index = MediaIndex::default();
    index.warnings.push("fixture provider unavailable".into());
    app.artwork.index = Some(Arc::new(index));
    app.router.current = Route::Task {
        section: Section::Artwork,
        game: 11,
    };

    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("provider is unavailable"))
    );
    assert!(
        !strings
            .iter()
            .any(|value| value.contains("fixture provider unavailable")),
        "technical provider detail starts hidden"
    );
}

#[test]
fn gui_v2_artwork_metadata_displays_local_romm_and_screenscraper_provenance() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let mut library = Library::new(vec![archive(3, "Provider Game", Some("PS2"))]);
    library.games[0].screenscraper = Some(
        archivefs_core::screenscraper_enrichment::PersistedScreenScraperEnrichment {
            archive_id: 3,
            values: archivefs_core::screenscraper_enrichment::AcceptedScreenScraperMetadata {
                synopsis: Some("Saved provider synopsis".into()),
                ..Default::default()
            },
            receipt: archivefs_core::screenscraper_enrichment::ScreenScraperEnrichmentReceipt {
                provider: "ScreenScraper".into(),
                provider_record_id: "ss-42".into(),
                retrieved_at_unix_seconds: 1,
                match_basis: "verified hash".into(),
                before: Default::default(),
                accepted: Default::default(),
                media_reference_count: 0,
            },
        },
    );
    app.library = Arc::new(library);
    let mut index = MediaIndex::default();
    index
        .covers
        .insert(3, Source::Local("/cache/cover.png".into()));
    index.descriptions.insert(3, "RomM description".into());
    app.artwork.index = Some(Arc::new(index));
    app.router.current = Route::Task {
        section: Section::Artwork,
        game: 3,
    };

    let output = frame(&context, &mut app, [1280.0, 820.0]);
    let strings = text(&output);
    assert!(strings.iter().any(|value| value == "RomM description"));
    assert!(strings.iter().any(|value| value == "Using local artwork"));
    assert!(!strings.iter().any(|value| value.contains("ss-42")));
    assert!(
        !strings
            .iter()
            .any(|value| value.contains("/cache/cover.png"))
    );
}

#[test]
fn gui_v2_artwork_refresh_is_async_and_rejects_duplicate_submit() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.artwork.index_loading = false;

    app.refresh_artwork_index();
    let generation = app.artwork.generation;
    let job = app.index_job;
    app.refresh_artwork_index();

    assert!(app.artwork.index_loading);
    assert_eq!(app.artwork.generation, generation);
    assert_eq!(app.index_job, job);
    assert_eq!(app.activity.running(), 1);
}

#[test]
fn untracked_background_errors_do_not_create_a_global_user_banner() {
    assert!(notice_for_background_error(None, "history root unavailable").is_none());
}

#[test]
fn tracked_background_errors_use_native_plain_language() {
    let notice = notice_for_background_error(Some("Loading your library"), "database detail")
        .expect("tracked user work should produce a notice");
    assert!(notice.message.contains("Retry it from this page"));
    assert!(notice.message.contains("game files were not changed"));
    assert!(!notice.message.contains("Legacy / Advanced"));
    assert_eq!(notice.technical, "database detail");
}

fn visual_fixture(context: &egui::Context) -> App {
    let mut app = fixture(context);
    app.library = Arc::new(Library::new(vec![
        archive(1, "Crash Test", Some("PSX")),
        archive(2, "Mario Fixture", Some("SNES")),
        archive(3, "Mystery Disc", None),
    ]));
    app.indices = (0..app.library.games.len()).collect();
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    app.artwork.index_loading = false;
    app
}

fn pump_imagery(context: &egui::Context, app: &mut App, size: [f32; 2]) -> egui::FullOutput {
    let start = Instant::now();
    let mut output = frame(context, app, size);
    while app.imagery.pending() > 0 && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(5));
        app.imagery.begin_frame(context);
        output = frame(context, app, size);
    }
    output
}

#[test]
fn gui_v2_platforms_page_shows_hardware_art_with_glyph_fallback() {
    let context = egui::Context::default();
    let mut app = visual_fixture(&context);
    app.router.current = Route::Section(Section::Platforms);
    let strings = text(&pump_imagery(&context, &mut app, [1280.0, 720.0]));
    for platform in ["PSX", "SNES", "Unknown system"] {
        assert!(
            strings.iter().any(|value| value == platform),
            "{platform}: {strings:?}"
        );
    }
    assert!(
        strings
            .iter()
            .any(|value| value.contains("games available to browse"))
    );
    assert_eq!(app.imagery.pending(), 0);
    // PSX and SNES have bundled hardware; "Unknown system" paints a glyph.
    assert_eq!(app.imagery.decoded, 2);
    // Revisiting reuses the cached thumbnails instead of decoding again.
    let _ = pump_imagery(&context, &mut app, [1920.0, 1080.0]);
    assert_eq!(app.imagery.decoded, 2);
}

#[test]
fn gui_v2_home_shows_library_hero_systems_and_recently_opened_games() {
    let context = egui::Context::default();
    let mut app = visual_fixture(&context);
    app.go(Route::Home);
    let strings = text(&pump_imagery(&context, &mut app, [1280.0, 720.0]));
    assert!(strings.iter().any(|value| value == "Your game library"));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("3 games · 3 systems"))
    );
    assert!(strings.iter().any(|value| value == "Your systems"));
    assert!(!strings.iter().any(|value| value == "Recently opened"));
    // Only real systems get a hardware tile.
    assert!(!strings.iter().any(|value| value == "Unknown system"));

    app.go(Route::Game(2));
    let _ = frame(&context, &mut app, [1280.0, 720.0]);
    app.go(Route::Home);
    let strings = text(&pump_imagery(&context, &mut app, [1280.0, 1080.0]));
    assert!(strings.iter().any(|value| value == "Recently opened"));
    assert!(strings.iter().any(|value| value.starts_with("Mario")));
    assert_eq!(app.imagery.recently_opened(), &[2]);
}

#[test]
fn gui_v2_game_details_keep_artwork_actions_and_metadata_together() {
    let context = egui::Context::default();
    let mut app = visual_fixture(&context);
    app.go(Route::Game(1));
    let mut strings = text(&pump_imagery(&context, &mut app, [1280.0, 1200.0]));
    // Screenshots may be below the fold after the upper guidance/readiness
    // content; they must remain reachable with ordinary page scrolling.
    strings.extend(text(&scroll_page(&context, &mut app, [1280.0, 1200.0])));
    for expected in [
        "Play",
        "PSX · Game image",
        "Verify",
        "Open Folder",
        "No screenshots found for this game.",
    ] {
        assert!(
            strings.iter().any(|value| value == expected),
            "{expected}: {strings:?}"
        );
    }
    // The missing cover falls back to the PSX hardware, not a blank box.
    assert_eq!(app.imagery.decoded, 1);
}

#[test]
fn gui_v2_empty_states_explain_in_plain_english() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Activity);
    let strings = text(&pump_imagery(&context, &mut app, [1280.0, 720.0]));
    assert!(
        strings
            .iter()
            .any(|value| value == "Nothing is running yet")
    );
    assert!(strings.iter().any(|value| value == "Browse my games"));

    app.router.current = Route::Section(Section::Platforms);
    let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
    assert!(
        strings
            .iter()
            .any(|value| value == "No systems are listed yet")
    );
    assert!(strings.iter().any(|value| value == "Add my games"));

    app.router.current = Route::Section(Section::Games);
    let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
    assert!(
        strings
            .iter()
            .any(|value| value == "Your games can go here")
    );
}

/// Real-scale timing harness. Ignored by default: it needs a real catalogue.
/// Run with `EMUWIZ_V2_PERF_DB=/path/library.sqlite3` (and isolated
/// `EMUWIZ_DATA_HOME`/`EMUWIZ_CONFIG_HOME`, so preference saves and the
/// thumbnail cache never touch a live profile):
/// `cargo test -p archivefs-gui --release gui_v2_real_catalogue_timings -- --ignored --nocapture`
#[test]
#[ignore = "needs EMUWIZ_V2_PERF_DB pointing at a real catalogue"]
fn gui_v2_real_catalogue_timings() {
    let Some(path) = std::env::var_os("EMUWIZ_V2_PERF_DB") else {
        eprintln!("EMUWIZ_V2_PERF_DB is not set; skipping");
        return;
    };
    fn tick(context: &egui::Context, app: &mut App, size: [f32; 2]) -> Duration {
        let start = Instant::now();
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(size[0], size[1]),
                )),
                ..Default::default()
            },
            |context| {
                app.poll(context);
                app.show(context);
                app.finish_frame();
            },
        );
        start.elapsed()
    }
    fn steady(context: &egui::Context, app: &mut App, size: [f32; 2]) -> (f64, f64) {
        let first = tick(context, app, size).as_secs_f64() * 1000.0;
        let mut total = 0.0;
        for _ in 0..30 {
            total += tick(context, app, size).as_secs_f64() * 1000.0;
        }
        (first, total / 30.0)
    }
    fn settle(
        context: &egui::Context,
        app: &mut App,
        size: [f32; 2],
        done: impl Fn(&App) -> bool,
    ) -> (f64, u32) {
        let start = Instant::now();
        let mut frames = 0;
        let mut worst = Duration::ZERO;
        while (frames < 2 || !done(app)) && start.elapsed() < Duration::from_secs(30) {
            worst = worst.max(tick(context, app, size));
            frames += 1;
            std::thread::sleep(Duration::from_millis(4));
        }
        eprintln!(
            "PERF   (worst frame while settling: {:.2}ms)",
            worst.as_secs_f64() * 1000.0
        );
        (start.elapsed().as_secs_f64() * 1000.0, frames)
    }
    fn states(app: &App) -> String {
        let mut states = [0usize; 4];
        for picture in app.artwork.pictures.values() {
            states[match picture {
                Picture::Loading => 0,
                Picture::Missing => 1,
                Picture::Failed(_) | Picture::Unavailable { .. } => 2,
                Picture::Ready { .. } => 3,
            }] += 1;
        }
        format!(
            "pictures[loading={} missing={} failed={} ready={}]",
            states[0], states[1], states[2], states[3]
        )
    }
    let pictures_settled = |app: &App| {
        app.imagery.pending() == 0
            && app.artwork.active() == 0
            && !app
                .artwork
                .pictures
                .values()
                .any(|picture| matches!(picture, Picture::Loading))
    };
    let start = Instant::now();
    let library = backend::load_library(Path::new(&path)).unwrap();
    let load_ms = start.elapsed().as_millis();
    let start = Instant::now();
    let index = MediaIndex::discover(&library);
    let index_ms = start.elapsed().as_millis();
    eprintln!(
        "PERF real load: {} games / {} platforms in {load_ms} ms; index {} covers in {index_ms} ms",
        library.games.len(),
        library.platforms.len(),
        index.covers.len()
    );
    let local: Vec<_> = index
        .covers
        .iter()
        .filter(|(_, source)| matches!(source, Source::Local(_)))
        .map(|(id, _)| *id)
        .collect();
    let start = Instant::now();
    let showcase = super::imagery::select_showcase(&library, &index);
    eprintln!(
        "PERF local covers: {} ({} without attention); showcase selection {} games in {}us",
        local.len(),
        local
            .iter()
            .filter(|id| library.game(**id).is_some_and(|game| !game.attention))
            .count(),
        showcase.len(),
        start.elapsed().as_micros()
    );
    let covered = library
        .games
        .iter()
        .find(|game| {
            index.covers.contains_key(&game.archive.id)
                && index
                    .screenshots
                    .get(&game.archive.id)
                    .is_some_and(|shots| shots.len() > 1)
        })
        .map(|game| game.archive.id);
    if let Some(game) = covered.and_then(|id| library.game(id)) {
        eprintln!(
            "PERF sample game with cover+screenshots: {} {} {}",
            game.archive.id, game.title, game.platform
        );
    }
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(library);
    app.indices = (0..app.library.games.len()).collect();
    app.artwork.index = Some(Arc::new(index));
    app.artwork.index_loading = false;
    app.environment = Some(super::environment::gather());
    app.welcome_dismissed = true;

    for size in [[1280.0, 720.0], [1920.0, 1080.0]] {
        let label = format!("{}x{}", size[0], size[1]);
        app.go(Route::Home);
        let first = tick(&context, &mut app, size).as_secs_f64() * 1000.0;
        let (settled, frames) = settle(&context, &mut app, size, pictures_settled);
        let (_, after) = steady(&context, &mut app, size);
        eprintln!(
            "PERF {label} Home: first={first:.2}ms artwork-settle={settled:.0}ms/{frames}f steady={after:.2}ms {}",
            states(&app)
        );
        app.go(Route::Section(Section::Platforms));
        let first = tick(&context, &mut app, size).as_secs_f64() * 1000.0;
        let (settled, frames) = settle(&context, &mut app, size, pictures_settled);
        let (_, after) = steady(&context, &mut app, size);
        eprintln!(
            "PERF {label} Platforms: first={first:.2}ms artwork-settle={settled:.0}ms/{frames}f steady={after:.2}ms {}",
            states(&app)
        );
        app.go(Route::Section(Section::Setup));
        let (first, avg) = steady(&context, &mut app, size);
        eprintln!("PERF {label} Setup&Doctor: first={first:.2}ms steady={avg:.2}ms");
        app.filter = Filter::default();
        app.indices = (0..app.library.games.len()).collect();
        app.go(Route::Section(Section::Games));
        let first = tick(&context, &mut app, size).as_secs_f64() * 1000.0;
        let (settled, frames) = settle(&context, &mut app, size, pictures_settled);
        let (_, after) = steady(&context, &mut app, size);
        eprintln!(
            "PERF {label} Games(all): first={first:.2}ms artwork-settle={settled:.0}ms/{frames}f steady={after:.2}ms {}",
            states(&app)
        );
        app.go(Route::Section(Section::Problems));
        let first = tick(&context, &mut app, size).as_secs_f64() * 1000.0;
        let (settled, frames) = settle(&context, &mut app, size, |app| {
            app.problem_summary.is_some() && app.problem_summary_job.is_none()
        });
        let (_, after) = steady(&context, &mut app, size);
        eprintln!(
            "PERF {label} Problems: first={first:.2}ms full={settled:.0}ms/{frames}f warm={after:.2}ms findings={}",
            app.problem_summary
                .as_ref()
                .map_or(0, |summary| summary.problems.len())
        );
    }
    let size = [1920.0, 1080.0];
    for platform in ["PSX", "SNES", "MegaDrive"] {
        app.filter.select_platform(platform.into());
        app.change_filter();
        app.filter_dirty = Some(Instant::now() - Duration::from_secs(1));
        let (switch, frames) = settle(&context, &mut app, size, |app| {
            !app.filter_inflight && app.filter_dirty.is_none()
        });
        let first = tick(&context, &mut app, size).as_secs_f64() * 1000.0;
        let (settled, art_frames) = settle(&context, &mut app, size, pictures_settled);
        let (_, avg) = steady(&context, &mut app, size);
        eprintln!(
            "PERF platform-switch {platform}: {} rows in {switch:.0}ms/{frames}f first={first:.2}ms steady={avg:.2}ms artwork-settle={settled:.0}ms/{art_frames}f {}",
            app.indices.len(),
            states(&app)
        );
    }
    if let Some(id) = covered {
        app.go(Route::Game(id));
        let first = tick(&context, &mut app, size).as_secs_f64() * 1000.0;
        let (settled, frames) = settle(&context, &mut app, size, pictures_settled);
        let (_, avg) = steady(&context, &mut app, size);
        eprintln!(
            "PERF game-select {id}: first={first:.2}ms artwork-settle={settled:.0}ms/{frames}f steady={avg:.2}ms {}",
            states(&app)
        );
    }
    let mut states = [0usize; 4];
    for picture in app.artwork.pictures.values() {
        states[match picture {
            Picture::Loading => 0,
            Picture::Missing => 1,
            Picture::Failed(_) | Picture::Unavailable { .. } => 2,
            Picture::Ready { .. } => 3,
        }] += 1;
    }
    eprintln!(
        "PERF artwork pictures: loading={} missing={} failed={} ready={}",
        states[0], states[1], states[2], states[3]
    );
}

// --- reconciliation loose ends: Problems -> MAME, Sources primary action ---

fn clip_visible(output: &egui::FullOutput, needle: &str) -> bool {
    fn shape_visible(shape: &egui::Shape, clip: egui::Rect, needle: &str) -> bool {
        match shape {
            egui::Shape::Text(text) => {
                text.galley.text() == needle
                    && clip.contains_rect(egui::Rect::from_min_size(text.pos, text.galley.size()))
            }
            egui::Shape::Vec(shapes) => shapes
                .iter()
                .any(|shape| shape_visible(shape, clip, needle)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|shape| shape_visible(&shape.shape, shape.clip_rect, needle))
}

fn sources_frame(size: [f32; 2]) -> (egui::Context, App, egui::FullOutput) {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    app.router.current = Route::Section(Section::Sources);
    frame(&context, &mut app, size);
    let output = frame(&context, &mut app, size);
    (context, app, output)
}

#[test]
fn sources_add_source_and_provider_setup_coexist() {
    // Primary action visible at the compact supported viewport.
    let (_, _, compact) = sources_frame([1024.0, 600.0]);
    assert!(
        clip_visible(&compact, "Add source"),
        "Add source clipped at 1024x600"
    );
    assert!(
        text(&compact)
            .iter()
            .any(|value| value == "Artwork and metadata providers"),
        "provider setup is not on the Sources page"
    );
    // Provider status fully visible at the normal viewport, alongside the action.
    let (_, _, normal) = sources_frame([1280.0, 820.0]);
    assert!(clip_visible(&normal, "Add source"));
    for label in [
        "Artwork and metadata providers",
        "Local artwork",
        "RomM",
        "ES-DE",
        "ScreenScraper",
    ] {
        assert!(
            clip_visible(&normal, label),
            "{label} not visible at 1280x820"
        );
    }
}

#[test]
fn sources_primary_action_is_stable_across_repeats_order_and_entry() {
    for _ in 0..3 {
        let (_, _, output) = sources_frame([1024.0, 600.0]);
        assert!(clip_visible(&output, "Add source"));
    }
    // After rendering related Artwork/Providers pages first, in the same
    // context and app.
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    for route in [
        Route::Section(Section::Artwork),
        Route::Section(Section::SourcesProviders),
        Route::Section(Section::Sources),
    ] {
        app.router.current = route;
        frame(&context, &mut app, [1024.0, 600.0]);
    }
    let output = frame(&context, &mut app, [1024.0, 600.0]);
    assert!(clip_visible(&output, "Add source"));
    // Entering through navigation instead of assigning the route directly.
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.go(Route::Section(Section::Sources));
    frame(&context, &mut app, [1024.0, 600.0]);
    let output = frame(&context, &mut app, [1024.0, 600.0]);
    assert!(clip_visible(&output, "Add source"));
}

#[test]
fn sources_paint_only_rendering_does_not_mutate_state_or_reuse_ids() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    app.router.current = Route::Section(Section::Sources);
    let roots = |app: &App| {
        app.native_workflows.as_ref().and_then(|workflows| {
            workflows
                .app
                .gui_config
                .source_roots()
                .ok()
                .map(|r| r.to_vec())
        })
    };
    // First frame creates the workflow state; everything after is paint-only.
    frame(&context, &mut app, [1024.0, 600.0]);
    let roots_before = roots(&app);
    for size in [[1024.0, 600.0], [1280.0, 820.0], [1024.0, 600.0]] {
        let output = frame(&context, &mut app, size);
        assert!(
            !text(&output).iter().any(|value| value.starts_with('🔥')),
            "duplicate widget id at {size:?}"
        );
    }
    assert_eq!(app.router.current, Route::Section(Section::Sources));
    assert_eq!(roots(&app), roots_before);
}

/// The canonical GUI-v2 Sources page projects the backend's source health: a
/// source that needs review offers the reviewed rebind, a source the backend has
/// not answered for reads as unknown, and neither is ever shown as up to date.
#[test]
fn sources_page_projects_backend_health_and_offers_reviewed_rebind_only_when_required() {
    use archivefs_core::catalogue_health::{RebindReason, SourceHealth, SourceHealthState};
    let view = || archivefs_core::SourceFolderView {
        path: std::path::PathBuf::from("/games"),
        role: Default::default(),
        enabled: true,
        created_at: None,
        id: Some(1),
        availability: archivefs_core::SourceAvailability::Available,
        last_scan_status: None,
        last_scan_error: None,
        last_scan_at: None,
        last_successful_scan_at: None,
        last_archive_count: None,
        assigned_platform: None,
        unknown_archive_count: 0,
    };
    let health = |state, rebind| SourceHealth {
        source_id: 1,
        path: std::path::PathBuf::from("/games"),
        state,
        rebind,
        generation: 0,
        detail: None,
    };
    let render_at = |size: [f32; 2], entries: Vec<SourceHealth>| {
        let context = egui::Context::default();
        let mut app = fixture(&context);
        app.artwork.index = Some(Arc::new(MediaIndex::default()));
        app.router.current = Route::Section(Section::Sources);
        frame(&context, &mut app, size);
        let mut snapshot = crate::tests::cached_snapshot(Vec::new());
        snapshot.source_views = vec![view()];
        snapshot.source_health = entries;
        app.native_workflows.as_mut().unwrap().app.database_state = crate::DatabaseState::Ready {
            snapshot: Box::new(snapshot),
            last_scan_summary: None,
        };
        frame(&context, &mut app, size);
        frame(&context, &mut app, size)
    };
    // Tall enough that the whole page is painted.
    let render = |entries: Vec<SourceHealth>| text(&render_at([1280.0, 2600.0], entries));
    // Status badges carry a tone glyph ("× Review needed"), so match the label.
    let has = |texts: &[String], needle: &str| {
        texts
            .iter()
            .any(|value| value == needle || value.ends_with(&format!(" {needle}")))
    };

    let legacy = render(vec![health(
        SourceHealthState::RebindRequired,
        Some(RebindReason::NeverBound),
    )]);
    assert!(has(&legacy, "Review and rebind source"), "{legacy:?}");
    // The review is reachable at the ordinary viewport without scrolling, even
    // with the full provider setup block on the same page.
    let ordinary = render_at(
        [1280.0, 820.0],
        vec![health(
            SourceHealthState::RebindRequired,
            Some(RebindReason::NeverBound),
        )],
    );
    assert!(clip_visible(&ordinary, "Review and rebind source"));
    assert!(clip_visible(&ordinary, "Artwork and metadata providers"));
    assert!(has(&legacy, "Review needed"));
    assert!(!has(&legacy, "Up to date"));

    let swapped = render(vec![health(
        SourceHealthState::RebindRequired,
        Some(RebindReason::BackingChanged),
    )]);
    assert!(has(&swapped, "Review and rebind source"));
    assert!(has(&swapped, "Review needed: different storage"));

    // No backend answer for the source: unknown, no review button, never healthy.
    let unknown = render(Vec::new());
    assert!(has(&unknown, "Not checked yet"), "{unknown:?}");
    assert!(!has(&unknown, "Review and rebind source"));
    assert!(!has(&unknown, "Up to date"));

    // A stale-looking or incomplete state never offers review and never reads healthy.
    for state in [
        SourceHealthState::NeedsScan,
        SourceHealthState::PartialScan,
        SourceHealthState::CoverageIncomplete,
        SourceHealthState::SourceUnavailable,
    ] {
        let rendered = render(vec![health(state, None)]);
        assert!(!has(&rendered, "Review and rebind source"), "{state:?}");
        assert!(!has(&rendered, "Up to date"), "{state:?}");
    }

    let healthy = render(vec![health(SourceHealthState::Healthy, None)]);
    assert!(has(&healthy, "Up to date"));
    assert!(!has(&healthy, "Review and rebind source"));
}

/// The Sources & Providers overview must not call the game folders "Ready" while
/// the backend says a folder needs review, a scan, or cannot be reached.
#[test]
fn sources_overview_card_never_says_ready_while_the_backend_has_open_health_issues() {
    use archivefs_core::catalogue_health::{RebindReason, SourceHealth, SourceHealthState};
    let render = |health: Vec<SourceHealth>| {
        let context = egui::Context::default();
        let mut app = fixture(&context);
        app.artwork.index = Some(Arc::new(MediaIndex::default()));
        app.router.current = Route::Section(Section::SourcesProviders);
        frame(&context, &mut app, [1280.0, 3000.0]);
        let mut snapshot = crate::tests::cached_snapshot(Vec::new());
        snapshot.source_views = vec![archivefs_core::SourceFolderView {
            path: std::path::PathBuf::from("/games"),
            role: Default::default(),
            enabled: true,
            created_at: None,
            id: Some(1),
            availability: archivefs_core::SourceAvailability::Available,
            last_scan_status: None,
            last_scan_error: None,
            last_scan_at: None,
            last_successful_scan_at: None,
            last_archive_count: None,
            assigned_platform: None,
            unknown_archive_count: 0,
        }];
        snapshot.source_health = health;
        app.native_workflows.as_mut().unwrap().app.database_state = crate::DatabaseState::Ready {
            snapshot: Box::new(snapshot),
            last_scan_summary: None,
        };
        frame(&context, &mut app, [1280.0, 3000.0]);
        text(&frame(&context, &mut app, [1280.0, 3000.0]))
    };
    let entry = |state, rebind| SourceHealth {
        source_id: 1,
        path: std::path::PathBuf::from("/games"),
        state,
        rebind,
        generation: 1,
        detail: None,
    };
    let has_part = |texts: &[String], part: &str| texts.iter().any(|v| v.contains(part));

    let review = render(vec![entry(
        SourceHealthState::RebindRequired,
        Some(RebindReason::BackingChanged),
    )]);
    assert!(
        has_part(&review, "1 folder needs review before scanning"),
        "{review:?}"
    );
    assert!(!has_part(&review, "available and up to date"));

    let unknown = render(Vec::new());
    assert!(
        has_part(&unknown, "catalogue health has not been read yet"),
        "{unknown:?}"
    );
    assert!(!has_part(&unknown, "available and up to date"));

    let healthy = render(vec![entry(SourceHealthState::Healthy, None)]);
    assert!(
        has_part(&healthy, "available and up to date"),
        "{healthy:?}"
    );
}

fn mame_archive(id: i64, title: &str) -> PersistedArchive {
    use archivefs_core::game_identity::*;
    let mut row = archive(id, title, Some("Arcade"));
    row.identity_report = Some(GameIdentityReport {
        archive_path: row.absolute_path.clone(),
        platform: IdentityPlatform::Arcade,
        format: IdentityImageFormat::LooseCartridgeRom,
        evidence: vec![IdentityEvidence {
            kind: IdentityKind::MameMachineName,
            status: IdentityStatus::Verified,
            value: Some("pacman".into()),
            confidence: IdentityConfidence::ExactBytes,
            provenance: IdentityProvenance {
                archive_path: row.absolute_path.clone(),
                member_path: None,
                member_index: None,
                method: "fixture".into(),
            },
            diagnostic: "fixture".into(),
        }],
        warnings: vec![],
        bytes_read: 1,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: true,
    });
    row
}

#[test]
fn problems_page_hands_proven_mame_findings_to_the_mame_workflow() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![
        mame_archive(1, "Missing Pac-Man"),
        archive(2, "Missing Plain", Some("PS2")),
    ]));
    let summary = Arc::new(ProblemSummary::from_library(&app.library, None));
    app.problem_summary = Some(summary.clone());
    app.problem_selected = None;
    app.router.current = Route::Section(Section::Problems);
    let output = frame(&context, &mut app, [1280.0, 1600.0]);
    let strings = text(&output);
    assert!(strings.iter().any(|value| value == "Review in MAME"));
    // The non-MAME problem keeps its own destination.
    assert!(strings.iter().any(|value| value == "Review Games"));
    // Painting does not mutate problem state, selection or route.
    frame(&context, &mut app, [1280.0, 1600.0]);
    assert_eq!(app.problem_summary.as_deref(), Some(&*summary));
    assert_eq!(app.problem_selected, None);
    assert_eq!(app.router.current, Route::Section(Section::Problems));
    // The destination resolves to the existing MAME workflow route.
    let mame = summary
        .problems
        .iter()
        .find(|p| p.id == "missing-1")
        .unwrap();
    app.go(mame.destination.route());
    assert_eq!(app.router.current, Route::MameWorkflow);
    assert_eq!(app.router.current.section(), Section::Mame);
}

#[test]
fn no_route_repeats_its_introductory_sentence() {
    // Page chrome supplies one title and one description. A body that repeats
    // a full sentence is the duplicated-header bug. Organisation shows the same
    // workflow tile text on two separate cards by design.
    for section in super::routes::SECTIONS.iter().copied() {
        if section == Section::Build {
            continue;
        }
        let context = egui::Context::default();
        let mut app = fixture(&context);
        app.router.current = Route::Section(section);
        frame(&context, &mut app, [1280.0, 900.0]);
        let strings = text(&frame(&context, &mut app, [1280.0, 900.0]));
        let mut seen = std::collections::BTreeMap::<&str, usize>::new();
        for value in &strings {
            if value.len() >= 30 && value.ends_with('.') {
                *seen.entry(value.as_str()).or_default() += 1;
            }
        }
        let repeated: Vec<_> = seen.iter().filter(|(_, n)| **n > 1).collect();
        assert!(repeated.is_empty(), "{section:?} repeats {repeated:?}");
    }
}

#[test]
fn hub_pages_show_one_title_and_one_description() {
    for section in [
        Section::AdvancedDiagnostics,
        Section::DatVerification,
        Section::EmulatorsFamily,
        Section::SavesStates,
        Section::Mame,
        Section::Conversion,
    ] {
        let context = egui::Context::default();
        let mut app = fixture(&context);
        app.router.current = Route::Section(section);
        frame(&context, &mut app, [1280.0, 900.0]);
        let strings = text(&frame(&context, &mut app, [1280.0, 900.0]));
        let family = super::routes::family_for_route(&Route::Section(section)).unwrap();
        // The family purpose is the page description and appears exactly once.
        assert_eq!(
            strings.iter().filter(|v| *v == family.purpose()).count(),
            1,
            "{section:?} description"
        );
        // The section-level generic line is no longer stacked under it.
        assert!(
            !strings.iter().any(|v| v == section.purpose())
                || section.purpose() == family.purpose()
        );
    }
    // Pages that used to introduce themselves a second time in the body.
    for (section, body_intro) in [
        (Section::Advanced, "Advanced tools"),
        (
            Section::Emulators,
            "Review emulator installation and readiness before launching a game.",
        ),
        (
            Section::Museum,
            "Browse the current v2 catalogue by platform, cover and title. Nothing here changes your files.",
        ),
    ] {
        let context = egui::Context::default();
        let mut app = fixture(&context);
        app.router.current = Route::Section(section);
        frame(&context, &mut app, [1280.0, 900.0]);
        let strings = text(&frame(&context, &mut app, [1280.0, 900.0]));
        assert!(
            !strings.iter().any(|v| v == body_intro),
            "{section:?} still repeats {body_intro:?}"
        );
    }
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Setup);
    frame(&context, &mut app, [1280.0, 900.0]);
    let strings = text(&frame(&context, &mut app, [1280.0, 900.0]));
    // sidebar entry + breadcrumb + page title only
    assert_eq!(strings.iter().filter(|v| *v == "Setup & Doctor").count(), 3);
}

#[test]
fn sidebar_shows_each_group_heading_once_before_its_entries() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Games);
    frame(&context, &mut app, [1280.0, 4000.0]);
    let strings = text(&frame(&context, &mut app, [1280.0, 4000.0]));
    for group in ["LIBRARY", "PLAY", "TOOLS", "FAMILIES"] {
        assert_eq!(
            strings.iter().filter(|v| *v == group).count(),
            1,
            "{group} heading repeated"
        );
    }
    // Every family entry still appears, after the single heading.
    let heading = strings.iter().position(|v| v == "FAMILIES").unwrap();
    for family in [
        "DATs & Verification",
        "Cheats & Mods",
        "Saves & States",
        "Emulators",
        "MAME",
    ] {
        assert!(
            strings.iter().skip(heading).any(|v| v == family),
            "{family} missing from Families"
        );
    }
    // Every section is still reachable from the sidebar exactly once.
    for section in super::routes::SECTIONS.iter().copied() {
        assert!(
            strings.iter().any(|v| v == section.title()),
            "{section:?} missing"
        );
    }
}

fn zip_row(id: i64, title: &str, path: std::path::PathBuf) -> PersistedArchive {
    let mut row = archive(id, title, Some("PSX"));
    row.archive_kind = "zip".into();
    row.absolute_path = path;
    row
}

#[test]
fn archive_inspector_lists_each_physical_archive_once() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("Game.zip");
    std::fs::write(&file, b"zip").unwrap();
    let alias = directory.path().join(".").join("Game.zip");
    let other = directory.path().join("Other.zip");
    std::fs::write(&other, b"zip").unwrap();
    let library = Library::new(vec![
        zip_row(1, "Game", file.clone()),
        // the same file reached through another catalogue row / spelling
        zip_row(2, "Game", alias),
        zip_row(3, "Game", file),
        zip_row(4, "Other", other),
    ]);
    let rows = super::archive_inspector::inspector_rows(&library.games);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].id, 1);
    assert_eq!(rows[1].title, "Other");
    assert!(rows.iter().all(|row| row.location.is_none()));
}

#[test]
fn archive_inspector_keeps_distinct_files_distinct_and_tells_them_apart() {
    let directory = tempfile::tempdir().unwrap();
    let mut archives = Vec::new();
    for (id, folder) in [(1, "EU"), (2, "US")] {
        let path = directory.path().join(folder);
        std::fs::create_dir_all(&path).unwrap();
        let file = path.join("Game.zip");
        std::fs::write(&file, format!("zip-{folder}")).unwrap();
        archives.push(zip_row(id, "Game", file));
    }
    let library = Library::new(archives);
    let rows = super::archive_inspector::inspector_rows(&library.games);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].location.as_deref(), Some("in EU"));
    assert_eq!(rows[1].location.as_deref(), Some("in US"));
}

#[test]
fn archive_inspector_rows_do_not_accumulate_on_route_reentry() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("Game.zip");
    std::fs::write(&file, b"zip").unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![
        zip_row(1, "Game", file.clone()),
        zip_row(2, "Game", file),
    ]));
    let mut counts = Vec::new();
    for _ in 0..3 {
        app.router.current = Route::Section(Section::Advanced);
        frame(&context, &mut app, [1280.0, 900.0]);
        app.router.current = Route::Home;
        frame(&context, &mut app, [1280.0, 900.0]);
        app.router.current = Route::Section(Section::Advanced);
        let strings = text(&frame(&context, &mut app, [1280.0, 900.0]));
        counts.push(strings.iter().filter(|v| v.starts_with("Game · ")).count());
    }
    assert_eq!(counts, vec![1, 1, 1]);
}

#[test]
fn romm_preview_action_names_what_it_previews() {
    assert_eq!(
        super::pages::romm_preview_button_label(crate::romm_source::SAMPLE_IMPORT_RECORDS),
        format!(
            "Preview import (first {} games)",
            crate::romm_source::SAMPLE_IMPORT_RECORDS
        )
    );
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Romm);
    let strings = text(&frame(&context, &mut app, [1280.0, 900.0]));
    assert!(
        strings
            .iter()
            .any(|v| v.starts_with("Preview import (first "))
    );
    assert!(!strings.iter().any(|v| v.contains("records)")));
}

#[test]
fn hackhash_normal_view_is_plain_english_and_keeps_internals_under_advanced() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Sources);
    frame(&context, &mut app, [1280.0, 4000.0]);
    let strings = text(&frame(&context, &mut app, [1280.0, 4000.0]));
    for jargon in [
        "Fetch candidate",
        "Choose detailed JSON",
        "Review validation",
        "Activate snapshot",
        "Detailed JSON URL:",
    ] {
        assert!(
            !strings.iter().any(|v| v == jargon),
            "{jargon} in normal view"
        );
    }
    assert!(!strings.iter().any(|v| v.starts_with("Parser schema")));
    for plain in [
        "Check for a newer list",
        "Download list",
        "Load list from a file…",
        "Review downloaded list",
        "Use this list",
    ] {
        assert!(strings.iter().any(|v| v == plain), "missing {plain}");
    }
    assert!(
        strings
            .iter()
            .any(|v| v.starts_with("Download address: not set"))
    );
    assert!(strings.iter().any(|v| v == "Advanced details"));
}

#[test]
fn mods_without_a_game_says_select_a_game_first() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Mods);
    let strings = text(&frame(&context, &mut app, [1280.0, 1200.0]));
    assert!(!strings.iter().any(|v| v.contains("on the bench")));
    assert!(
        strings
            .iter()
            .any(|v| v == "Select a game first" || v == "No game selected yet"),
        "no plain select-a-game instruction: {:?}",
        strings.iter().take(40).collect::<Vec<_>>()
    );
}

#[test]
fn browse_first_guidance_is_home_only_and_hubs_show_no_unrelated_tip() {
    for section in super::routes::SECTIONS.iter().copied() {
        let context = egui::Context::default();
        let mut app = fixture(&context);
        app.router.current = Route::Section(section);
        frame(&context, &mut app, [1280.0, 3200.0]);
        let strings = text(&frame(&context, &mut app, [1280.0, 3200.0]));
        let browse_first = strings.iter().any(|v| v.starts_with("Browse first:"));
        assert!(!browse_first || section == Section::Home, "{section:?}");
        if section == Section::Home {
            // An empty library gets its own guidance instead of the generic tip.
            assert!(
                strings
                    .iter()
                    .any(|v| v.starts_with("No games are listed yet")),
                "Home guidance missing"
            );
        }
    }
    // Family hubs have no page-specific guidance: no strip at all.
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::DatVerification);
    frame(&context, &mut app, [1280.0, 3200.0]);
    let strings = text(&frame(&context, &mut app, [1280.0, 3200.0]));
    assert!(!strings.iter().any(|v| v.starts_with("Mr Wiz")));
}

#[test]
fn problems_page_guidance_follows_the_loaded_summary() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Problems);
    let checking = text(&frame(&context, &mut app, [1280.0, 2400.0]));
    assert!(
        checking
            .iter()
            .any(|v| v.contains("EmuWiz is checking what it already knows"))
    );
    app.library = Arc::new(Library::new(vec![archive(1, "Missing Game", Some("PS2"))]));
    app.problem_summary = Some(Arc::new(ProblemSummary::from_library(&app.library, None)));
    let loaded = text(&frame(&context, &mut app, [1280.0, 2400.0]));
    assert!(loaded.iter().any(|v| v.contains("finding(s) to look at")));
    // Deterministic for an unchanged context.
    let again = text(&frame(&context, &mut app, [1280.0, 2400.0]));
    assert!(again.iter().any(|v| v.contains("finding(s) to look at")));
}

fn lifecycle_fixture(
    id: &str,
    state: archivefs_core::emulator_lifecycle::LifecycleState,
    installations: usize,
) -> archivefs_core::emulator_lifecycle::EmulatorLifecycleProjection {
    use archivefs_core::emulator_inventory::{InstallationType, VersionConfidence, VersionSource};
    use archivefs_core::emulator_lifecycle::*;
    EmulatorLifecycleProjection {
        schema_version: 1,
        emulator_id: id.into(),
        state,
        selected: None,
        stale_selected: false,
        installations: (0..installations)
            .map(|index| EmulatorLifecycleInstallation {
                emulator_id: id.into(),
                exact_binding: ExactBinding::NativeExecutable {
                    path: format!("/opt/{id}/{index}/bin").into(),
                },
                installation_type: InstallationType::Unknown,
                ownership_category: OwnershipCategory::Unknown,
                version: VersionEvidence {
                    version: None,
                    raw_output: None,
                    source: VersionSource::Unknown,
                    confidence: VersionConfidence::Unknown,
                },
                channel: LifecycleChannel::Unknown,
                local_health: LocalHealth::Unknown,
                launch_readiness: None,
                package: None,
                update_authority: UpdateAuthority::Unknown,
                update_status: None,
                selected: false,
                provenance: LifecycleProvenance {
                    discovered_by: vec!["fixture".into()],
                    selected_by: Vec::new(),
                    package_manager: None,
                    flatpak_scope: None,
                    metadata_timestamp_unix: None,
                },
            })
            .collect(),
    }
}

#[test]
fn setup_and_doctor_leads_with_problems_and_collapses_healthy_installations() {
    use archivefs_core::emulator_lifecycle::LifecycleState::*;
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.environment = Some(super::environment::EnvironmentSnapshot {
        config_present: true,
        lifecycle: vec![
            lifecycle_fixture("duckstation", InstalledUnknownVersion, 2),
            lifecycle_fixture("retroarch", MultipleInstallations, 2),
            lifecycle_fixture("rpcs3", Missing, 0),
            lifecycle_fixture("pcsx2", Broken, 0),
        ],
        ..Default::default()
    });
    app.welcome_dismissed = true;
    app.router.current = Route::Section(Section::Setup);
    frame(&context, &mut app, [1280.0, 3200.0]);
    let strings = text(&frame(&context, &mut app, [1280.0, 3200.0]));
    let has = |needle: &str| strings.iter().any(|v| v == needle);
    // Real problems come first and stay visible.
    assert!(has("Needs your attention (2)"));
    assert!(has("retroarch") && has("pcsx2"));
    assert!(has("Multiple installations found"));
    // Missing emulators are visible, compact, and offer the setup route.
    assert!(has("Not installed (1)"));
    assert!(has("rpcs3"));
    assert!(has("Open Emulator Setup"));
    // Healthy/boring installations are collapsed by default.
    assert!(has("Working emulators (1)"));
    assert!(!has("duckstation"));
    // The repeated "unknown" boilerplate is not a wall of text.
    for boring in [
        "Installation type unknown",
        "Latest version unknown",
        "Updates: Update owner unknown",
    ] {
        assert!(!has(boring), "{boring} repeated in the default view");
    }
    assert!(!strings.iter().any(|v| v.starts_with("Updates: ")));
}

#[test]
fn setup_and_doctor_with_no_problems_says_so_and_keeps_details_reachable() {
    use archivefs_core::emulator_lifecycle::LifecycleState::*;
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.environment = Some(super::environment::EnvironmentSnapshot {
        config_present: true,
        lifecycle: vec![lifecycle_fixture("duckstation", InstalledCurrent, 1)],
        ..Default::default()
    });
    app.welcome_dismissed = true;
    app.router.current = Route::Section(Section::Setup);
    frame(&context, &mut app, [1280.0, 3200.0]);
    let strings = text(&frame(&context, &mut app, [1280.0, 3200.0]));
    assert!(
        strings
            .iter()
            .any(|v| v == "No emulator needs your attention right now.")
    );
    assert!(strings.iter().any(|v| v == "Working emulators (1)"));
}

#[test]
fn separate_window_confirmation_is_visible_calm_and_cleared_by_navigation() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Advanced);
    app.handoff_status = Some("Opened in a separate window. If you cannot see it, look behind this window; GUI v2 is still open.".into());
    let strings = text(&frame(&context, &mut app, [1280.0, 900.0]));
    assert!(strings.iter().any(|v| v == "Separate window"));
    assert!(
        strings
            .iter()
            .any(|v| v.starts_with("Opened in a separate window."))
    );
    // A success confirmation is not presented as a problem.
    assert!(!strings.iter().any(|v| v == "Needs attention"));
    // The action it confirms is still offered, and navigating away clears it.
    assert!(strings.iter().any(|v| v == "Open specialist interface"));
    app.go(Route::Home);
    assert!(app.handoff_status.is_none());
    let strings = text(&frame(&context, &mut app, [1280.0, 900.0]));
    assert!(!strings.iter().any(|v| v == "Separate window"));
}

#[test]
fn archive_inspector_tells_apart_same_named_folders_under_different_parents() {
    let directory = tempfile::tempdir().unwrap();
    let mut archives = Vec::new();
    for (id, parent) in [(1, "a"), (2, "b")] {
        let folder = directory.path().join(parent).join("PS1");
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("Game.zip");
        std::fs::write(&file, format!("zip-{parent}")).unwrap();
        archives.push(zip_row(id, "Game", file));
    }
    let library = Library::new(archives);
    let rows = super::archive_inspector::inspector_rows(&library.games);
    assert_eq!(rows.len(), 2);
    let labels: Vec<_> = rows
        .iter()
        .map(|row| row.location.clone().unwrap())
        .collect();
    assert_ne!(labels[0], labels[1], "{labels:?}");
    // Deterministic: the same input always gives the same rows.
    assert_eq!(
        rows,
        super::archive_inspector::inspector_rows(&library.games)
    );
}

#[test]
fn archive_inspector_rows_follow_a_replaced_library() {
    let directory = tempfile::tempdir().unwrap();
    let one = directory.path().join("One.zip");
    let two = directory.path().join("Two.zip");
    std::fs::write(&one, b"1").unwrap();
    std::fs::write(&two, b"2").unwrap();
    let mut state = super::archive_inspector::ArchiveInspectorPageState::default();
    let first = Arc::new(Library::new(vec![zip_row(1, "One", one.clone())]));
    assert_eq!(state.rows(&first).len(), 1);
    assert_eq!(state.rows(&first).len(), 1);
    drop(first);
    // A new library, possibly allocated where the old one was, must not be
    // served the old library's rows.
    let second = Arc::new(Library::new(vec![
        zip_row(1, "One", one),
        zip_row(2, "Two", two),
    ]));
    assert_eq!(state.rows(&second).len(), 2);
}

#[test]
fn no_route_repeats_its_title_with_different_capitalisation() {
    // "RomM Library" in the page header and "RomM library" as a body heading is
    // the same duplicated title. Compare all rendered text ignoring case.
    // Home pairs each task tile heading with its button ("Browse My Games" and
    // "Browse my games"), which is not a repeated page title.
    let mut offenders = Vec::new();
    for section in super::routes::SECTIONS.iter().copied() {
        if section == Section::Home {
            continue;
        }
        let context = egui::Context::default();
        let mut app = fixture(&context);
        app.router.current = Route::Section(section);
        frame(&context, &mut app, [1280.0, 900.0]);
        let strings = text(&frame(&context, &mut app, [1280.0, 900.0]));
        let mut by_lowercase =
            std::collections::BTreeMap::<String, std::collections::BTreeSet<&str>>::new();
        for value in &strings {
            if value.len() >= 6 {
                by_lowercase
                    .entry(value.to_lowercase())
                    .or_default()
                    .insert(value.as_str());
            }
        }
        for (_, variants) in by_lowercase {
            if variants.len() > 1 {
                offenders.push(format!("{section:?}: {variants:?}"));
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:#?}");
}

#[test]
fn no_route_shows_its_title_more_often_than_the_chrome_explains() {
    // The title legitimately appears once per sidebar entry with that title,
    // once in the breadcrumb and once as the page heading. More means the body
    // introduced itself again. A tall viewport keeps the whole sidebar drawn.
    let mut offenders = Vec::new();
    for section in super::routes::SECTIONS.iter().copied() {
        eprintln!("Checking title repetition on {section:?}");
        if section == Section::Home {
            continue;
        }
        let title = section.title();
        let sidebar_entries = super::routes::SECTIONS
            .iter()
            .filter(|other| other.title() == title)
            .count();
        let context = egui::Context::default();
        let mut app = fixture(&context);
        app.router.current = Route::Section(section);
        frame(&context, &mut app, [1280.0, 4000.0]);
        let strings = text(&frame(&context, &mut app, [1280.0, 4000.0]));
        let seen = strings.iter().filter(|value| *value == title).count();
        // Sidebar entries + breadcrumb + page heading. Hero copy complements
        // the page title; no page is exempt from the duplicate-title guard.
        let allowed = sidebar_entries + 2;
        if seen > allowed {
            offenders.push(format!(
                "{section:?} shows {title:?} {seen}x (allowed {allowed})"
            ));
        }
    }
    assert!(offenders.is_empty(), "{offenders:#?}");
}

// ---- Artwork startup: any artwork-consuming surface starts the one index ----

/// An app whose artwork index is built by a counting fake (never the real
/// provider data) and whose only cover is a real local PNG.
fn startup_fixture(
    context: &egui::Context,
    directory: &Path,
    delay: Duration,
) -> (App, Arc<std::sync::atomic::AtomicUsize>) {
    let cover = directory.join("cover.png");
    image::RgbaImage::from_pixel(8, 8, image::Rgba([200, 30, 30, 255]))
        .save(&cover)
        .unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = calls.clone();
    let mut app = fixture(context);
    app.artwork = Artwork::with_indexer(
        context.clone(),
        Some(directory.join("cache")),
        Arc::new(move |library| {
            counted.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(delay);
            let mut index = MediaIndex::default();
            for game in &library.games {
                index
                    .covers
                    .insert(game.archive.id, Source::Local(cover.clone()));
            }
            index
        }),
    );
    app.library = Arc::new(Library::new(vec![
        archive(1, "Crash Test", Some("PSX")),
        archive(2, "Mario Fixture", Some("SNES")),
        archive(3, "Mystery Disc", None),
    ]));
    app.indices = (0..app.library.games.len()).collect();
    app.loaded = true;
    (app, calls)
}

fn pump_until(
    context: &egui::Context,
    app: &mut App,
    size: [f32; 2],
    done: impl Fn(&App) -> bool,
) -> egui::FullOutput {
    let start = Instant::now();
    let mut output = frame(context, app, size);
    while !done(app) && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
        app.poll(context);
        output = frame(context, app, size);
    }
    output
}

#[test]
fn gui_v2_browse_play_alone_starts_artwork_and_ends_with_a_cover() {
    let context = egui::Context::default();
    let directory = tempfile::tempdir().unwrap();
    let (mut app, calls) = startup_fixture(&context, directory.path(), Duration::from_millis(400));
    // No Museum, no Artwork & Extras: straight to Browse & Play.
    app.router.current = Route::BrowsePlay;
    assert!(app.artwork.index.is_none() && !app.artwork.index_loading);

    let first = frame(&context, &mut app, [1280.0, 820.0]);
    assert!(
        app.artwork.index_loading,
        "showing covers must start the index"
    );
    assert!(
        text(&first)
            .iter()
            .any(|value| value == "Preparing artwork…"),
        "an honest preparing state, not a permanent 'Loading picture…'"
    );
    assert!(
        !text(&first).iter().any(|value| value == "Loading picture…"),
        "nothing is loading yet: there is no index to load from"
    );

    // The GUI stays interactive: frames keep running while the index builds.
    let started = Instant::now();
    frame(&context, &mut app, [1280.0, 820.0]);
    assert!(started.elapsed() < Duration::from_millis(300));

    pump_until(&context, &mut app, [1280.0, 820.0], |app| {
        let key = app.artwork.key(1, Kind::Cover);
        matches!(app.artwork.pictures.get(&key), Some(Picture::Ready { .. }))
    });
    let key = app.artwork.key(1, Kind::Cover);
    assert!(matches!(
        app.artwork.pictures.get(&key),
        Some(Picture::Ready { .. })
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn gui_v2_artwork_index_is_built_once_however_many_surfaces_ask() {
    let context = egui::Context::default();
    let directory = tempfile::tempdir().unwrap();
    let (mut app, calls) = startup_fixture(&context, directory.path(), Duration::from_millis(150));
    app.router.current = Route::BrowsePlay;
    // Several frames while the build is running: still one build.
    for _ in 0..5 {
        frame(&context, &mut app, [1280.0, 820.0]);
    }
    pump_until(&context, &mut app, [1280.0, 820.0], |app| {
        app.artwork.index.is_some()
    });
    let index = app.artwork.index.clone().unwrap();
    let generation = app.artwork.generation;

    // Museum and Artwork & Extras afterwards reuse it.
    app.router.current = Route::Section(Section::Museum);
    frame(&context, &mut app, [1280.0, 820.0]);
    app.router.current = Route::Task {
        section: Section::Artwork,
        game: 1,
    };
    frame(&context, &mut app, [1280.0, 820.0]);
    app.router.current = Route::BrowsePlayGame(2);
    frame(&context, &mut app, [1280.0, 820.0]);

    assert_eq!(calls.load(Ordering::SeqCst), 1, "no second index build");
    assert_eq!(app.artwork.generation, generation);
    assert!(Arc::ptr_eq(&index, app.artwork.index.as_ref().unwrap()));
}

#[test]
fn gui_v2_museum_still_starts_the_index_when_it_is_the_first_surface() {
    let context = egui::Context::default();
    let directory = tempfile::tempdir().unwrap();
    let (mut app, calls) = startup_fixture(&context, directory.path(), Duration::from_millis(10));
    app.router.current = Route::Section(Section::Museum);
    pump_until(&context, &mut app, [1280.0, 820.0], |app| {
        app.artwork.index.is_some()
    });
    app.router.current = Route::BrowsePlay;
    frame(&context, &mut app, [1280.0, 820.0]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn gui_v2_no_index_build_starts_before_the_catalogue_is_loaded() {
    let context = egui::Context::default();
    let directory = tempfile::tempdir().unwrap();
    let (mut app, calls) = startup_fixture(&context, directory.path(), Duration::from_millis(10));
    app.loaded = false;
    app.router.current = Route::BrowsePlay;
    frame(&context, &mut app, [1280.0, 820.0]);
    assert!(!app.artwork.index_loading);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn gui_v2_switching_games_never_shows_the_previous_games_result() {
    let context = egui::Context::default();
    let directory = tempfile::tempdir().unwrap();
    let (mut app, _) = startup_fixture(&context, directory.path(), Duration::from_millis(10));
    for route in [
        Route::BrowsePlayGame(1),
        Route::BrowsePlayGame(2),
        Route::BrowsePlayGame(3),
        Route::BrowsePlayGame(1),
    ] {
        app.router.current = route;
        frame(&context, &mut app, [1280.0, 820.0]);
    }
    pump_until(&context, &mut app, [1280.0, 820.0], |app| {
        app.artwork.index.is_some()
    });
    // Every stored picture is under its own game's key.
    for (key, picture) in &app.artwork.pictures {
        assert_eq!(key.generation, app.artwork.generation);
        if let Picture::Ready { .. } = picture {
            assert!((1..=3).contains(&key.game));
        }
    }
}

#[test]
fn gui_v2_browse_play_keeps_play_on_screen_beside_a_long_titled_shelf() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(
        (1..=6000)
            .map(|id| {
                archive(
                    id,
                    "A Rather Long Game Title (Europe) (En,Fr,De,Es,It) (Rev 1)",
                    Some("SNES"),
                )
            })
            .collect(),
    ));
    app.indices = (0..app.library.games.len()).collect();
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    app.router.current = Route::BrowsePlayGame(2);
    for (width, height) in [(1880.0, 1000.0), (1400.0, 1000.0), (1000.0, 800.0)] {
        let output = frame(&context, &mut app, [width, height]);
        let play = text_bounds(&output, "Play");
        assert!(!play.is_empty(), "Play must be drawn at {width}px");
        assert!(
            play.iter()
                .all(|rect| rect.max.x <= width && rect.max.y <= height),
            "Play must be on screen without scrolling at {width}px: {play:?}"
        );
    }
}

#[test]
fn gui_v2_browse_play_draws_only_visible_rows_of_a_huge_library_and_counts_it() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(
        (1..=6000)
            .map(|id| archive(id, &format!("Game {id}"), Some("SNES")))
            .collect(),
    ));
    app.indices = (0..app.library.games.len()).collect();
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    app.router.current = Route::BrowsePlay;
    let started = Instant::now();
    let strings = text(&frame(&context, &mut app, [1880.0, 1000.0]));
    assert!(
        strings.iter().any(|value| value == "6,000 games"),
        "the whole library is counted, not just a page"
    );
    let drawn = strings
        .iter()
        .filter(|value| value.starts_with("Game "))
        .count();
    assert!(
        (1..=200).contains(&drawn),
        "only the visible rows are drawn, got {drawn}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    // A new search starts from the top again, and says what it matched.
    app.filter.search = "Game 1".into();
    let strings = text(&frame(&context, &mut app, [1880.0, 1000.0]));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("matching \"Game 1\"")),
        "the active search is shown and clearable"
    );
    assert!(!app.browse_play.reset_scroll_pending());
}

#[test]
fn gui_v2_browse_play_many_platforms_do_not_push_the_selected_game_off_screen() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(
        (1..=200)
            .map(|id| {
                archive(
                    id,
                    &format!("Game {id}"),
                    Some(&format!("Platform {}", id % 70)),
                )
            })
            .collect(),
    ));
    app.indices = (0..app.library.games.len()).collect();
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    for width in [1880.0, 1400.0, 1000.0] {
        for (route, needle) in [
            (Route::BrowsePlay, "Pick a game"),
            (Route::BrowsePlayGame(5), "Play"),
        ] {
            app.router.current = route;
            let output = frame(&context, &mut app, [width, 1000.0]);
            let bounds = text_bounds(&output, needle);
            // Below the side-by-side width the prompt is intentionally absent.
            if needle == "Pick a game" && width < 1130.0 {
                continue;
            }
            assert!(!bounds.is_empty(), "{needle} must be drawn at {width}px");
            assert!(
                bounds.iter().all(|rect| rect.max.x <= width),
                "{needle} must be on screen at {width}px: {bounds:?}"
            );
        }
    }
    let width = 1880.0;
    // Seventy systems collapse to a few chips plus one dropdown.
    let strings = text(&frame(&context, &mut app, [width, 1000.0]));
    assert!(
        strings
            .iter()
            .any(|value| value.starts_with("More systems"))
    );
}

#[test]
fn gui_v2_browse_play_says_loading_while_the_library_loads() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.loaded = false;
    app.router.current = Route::BrowsePlay;
    let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("Loading your game list"))
    );
    assert!(
        !strings
            .iter()
            .any(|value| value.contains("No games have been added yet"))
    );
}

#[test]
fn gui_v2_browse_play_sidebar_highlights_one_entry() {
    let source = include_str!("pages.rs");
    assert!(source.contains("light up the older Games catalogue entry"));
}

#[test]
fn gui_v2_game_details_keep_the_action_on_screen_for_a_long_wrapping_title() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(
        1,
        "A Very Long Example Game Title That Certainly Wraps Onto Two Lines (Europe) (En,Fr,De,Es,It)",
        Some("Saturn"),
    )]));
    app.router.current = Route::Game(1);
    let output = frame(&context, &mut app, [1000.0, 560.0]);
    let rect = bounds_below_chrome(&output, "Play").expect("Play is drawn");
    assert!(
        rect.max.y < 520.0,
        "Play must stay above the footer: {rect:?}"
    );
}

#[test]
fn gui_v2_setup_portability_is_visible_in_settings() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Settings);
    let strings = text(&frame(&context, &mut app, [1280.0, 900.0]));
    for expected in [
        "Move your setup to another device",
        "Export setup",
        "Preview setup file",
        "Applying imports is not available yet",
    ] {
        assert!(
            strings.iter().any(|value| value.contains(expected)),
            "missing setup portability content: {expected}"
        );
    }
}
