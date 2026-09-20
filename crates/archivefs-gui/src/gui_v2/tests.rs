use super::*;
use super::{
    activity::Phase,
    artwork::{Artwork, Picture},
    library::{DuplicateGroup, DuplicateMember, DuplicateReport, Game},
    media_sources::{Kind, MediaIndex, Source},
};
use archivefs_core::PersistedArchive;
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
        mods: super::mods::ModsPageState::default(),
        mrwiz_dismissed: false,
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
fn gui_v2_build_library_is_a_native_plain_english_workflow() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Build);
    let strings = text(&frame(&context, &mut app, [1280.0, 820.0]));
    assert!(
        strings
            .iter()
            .any(|value| value == "Build a playing library")
    );
    assert!(
        strings
            .iter()
            .any(|value| value.contains("original collection untouched"))
    );
    assert!(strings.iter().any(|value| value.contains("Source library")));
    assert!(strings.iter().any(|value| value.contains("Inputs")));
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
    assert_eq!(unique.len(), 16);
}

#[test]
fn gui_v2_home_has_six_explained_tasks_with_correct_routes() {
    assert_eq!(routes::HOME_TASKS.len(), 6);
    assert_eq!(
        routes::HOME_TASKS
            .iter()
            .map(|task| task.0)
            .collect::<Vec<_>>(),
        vec![
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
        MainView::DatSources
    );
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
        }
    }
    let before = fs::read(&database_path).unwrap();
    let library = backend::load_library(&database_path).unwrap();
    assert_eq!(library.games.len(), 1);
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
    assert!(app.activity.jobs.is_empty());
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
        Section::Check,
        Section::Problems,
        Section::Build,
        Section::Emulators,
        Section::Sources,
        Section::Advanced,
    ] {
        for size in [[1024.0, 600.0], [640.0, 480.0]] {
            let context = egui::Context::default();
            let mut app = fixture(&context);
            app.router.current = Route::Section(section);
            frame(&context, &mut app, size);
            let output = frame(&context, &mut app, size);
            let actions = if section == Section::Check {
                vec!["Choose a platform"]
            } else if section == Section::Problems {
                vec!["Nothing needs attention right now."]
            } else if section == Section::Build {
                vec![
                    "Set up library",
                    "Preview playing library",
                    "Preview 1G1R Library",
                ]
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
