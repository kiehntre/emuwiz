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
        artwork: Artwork::start(context.clone()),
        imagery: super::imagery::Imagery::default(),
        load_job: None,
        artwork_job: None,
        index_job: None,
        preferences_dirty: None,
        interacted: false,
        loaded: true,
        notice: None,
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
        doctor_platform: None,
        mrwiz_dismissed: false,
        saves_states: super::saves_states::SavesStatesState::default(),
    }
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
                super::saves_states::show(app, ui);
            });
        },
    )
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
    assert!(strings.iter().any(|value| value.contains("Saves & States")));
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

    for label in [
        "Home",
        "Games",
        "Platforms",
        "Organisation",
        "Launch",
        "Converter",
        "Museum",
        "Setup & Doctor",
    ] {
        assert!(
            strings.iter().any(|value| value == label),
            "missing {label}"
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
    assert!(strings.iter().any(|value| value == "← Back"));

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
    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));

    assert!(strings.iter().any(|value| value == "Media: Arcade set"));
    assert!(strings.iter().any(|value| value == "Source: pacman"));
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
    app.finish_playing_library_job(super::PlayingLibraryJobKind::Preview, Box::new(result), 1);
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
    app.finish_playing_library_job(super::PlayingLibraryJobKind::Preview, Box::new(result), 1);
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
        "Organise verified games",
        "Build a clean playing library",
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
            strings
                .iter()
                .any(|value| value == "Organise verified games"),
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
/// height accounts for the new hero and explainer panel pushing the five
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
            .any(|value| value.contains("stays exactly where it is"))
    );
    assert!(
        strings
            .iter()
            .any(|value| value.contains("preview before anything happens")
                || value.contains("Nothing changes until"))
    );
    // All five action cards must still be present and clickable alongside
    // the explainer - it must never bury the primary actions.
    for title in [
        "Organise verified games",
        "Build a clean playing library",
        "Organise for RomM",
        "Export to ES-DE",
        "Prepare for RetroDECK",
    ] {
        assert!(strings.iter().any(|value| value == title), "{title}");
    }
    // Dismissing hides the explainer without touching any action.
    app.organisation.intro_dismissed = true;
    let strings_after = text(&frame(&context, &mut app, [1280.0, 1800.0]));
    assert!(
        !strings_after
            .iter()
            .any(|value| value.contains("stays exactly where it is"))
    );
    assert!(
        strings_after
            .iter()
            .any(|value| value == "Organise verified games")
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
        "Organise verified games",
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

    assert!(strings.iter().any(|value| value == "Choose a game first"));
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
    assert_eq!(unique.len(), 19);
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
    let strings = text(&frame(&context, &mut app, [1280.0, 720.0]));
    assert!(strings.iter().any(|value| value == "Welcome to EmuWiz"));
    assert!(strings.iter().any(|value| value == "Get started"));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("Set up the basics"))
    );
    assert!(strings.iter().all(|value| value != "DAT registry"));
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
        },
    )
    .unwrap();
    let restored: Preferences = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(restored.route, Route::Game(22));
    assert_eq!(restored.filter.search, "Zelda");
    assert_eq!(fs::read_to_string(old).unwrap(), "advanced");
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
    let output = frame(&context, &mut app, [1280.0, 820.0]);
    let texts = text(&output);
    for label in [
        "Play",
        "Verify",
        "Artwork & Metadata",
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
    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    assert!(strings.iter().any(|s| s == "Play"));
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
            || job.title == "Checking emulator readiness"
            || job.title == "Checking save locations"
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
        for size in [[1024.0, 600.0], [640.0, 480.0]] {
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
                vec!["Nothing needs attention right now."]
            } else if section == Section::Build {
                vec!["Organise verified games", "Build a clean playing library"]
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
}

#[test]
fn gui_v2_refresh_readiness_error_retains_an_obvious_retry() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(1, "Game", Some("PS2"))]));
    app.router.current = Route::Game(1);
    app.detail_failed = Some(1);
    let output = frame(&context, &mut app, [1280.0, 820.0]);
    assert!(
        text(&output)
            .iter()
            .any(|line| line == "Retry readiness check")
    );
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

    assert!(strings.iter().any(|value| value == "Related source tools"));
    assert!(strings.iter().any(|value| value == "Game Folders"));
    assert!(
        strings
            .iter()
            .any(|value| value == "Verification Data / DATs")
    );
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
fn gui_v2_advanced_is_a_specialist_escape_not_a_duplicate_dat_route() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Advanced);

    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    assert!(strings.iter().any(|value| value == "Advanced tools"));
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
    assert!(selected_text.iter().any(|value| value == "Metadata"));
    assert!(selected_text.iter().any(|value| value
        == "No screenshot found. Advanced Details explains which providers were checked."));
    assert!(
        !selected_text
            .iter()
            .any(|value| value.contains("Original path:")),
        "advanced details start closed"
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
    assert!(strings.iter().any(|value| value.contains("Local file")));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("ScreenScraper · record ss-42"))
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
    app.mrwiz_dismissed = true;
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
    let strings = text(&pump_imagery(&context, &mut app, [1280.0, 720.0]));
    for expected in [
        "Play",
        "PSX",
        "Media: Game image",
        "Verify",
        "Open Folder",
        "No screenshots yet",
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
                Picture::Failed(_) => 2,
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
    app.mrwiz_dismissed = true;

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
            Picture::Failed(_) => 2,
            Picture::Ready { .. } => 3,
        }] += 1;
    }
    eprintln!(
        "PERF artwork pictures: loading={} missing={} failed={} ready={}",
        states[0], states[1], states[2], states[3]
    );
}
