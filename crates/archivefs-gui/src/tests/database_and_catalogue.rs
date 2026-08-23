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
//! Predominant theme observed in this slice: database status, catalogue scanning, and row selection/sorting.

use super::*;

#[test]
fn gui_database_refresh_is_read_only_and_creates_no_sidecars() {
    let dir = database_test_dir("read-only-refresh");
    let database_path = dir.join("library.sqlite3");
    Database::open_or_create(&database_path)
        .unwrap()
        .close()
        .unwrap();
    let before = std::fs::read(&database_path).unwrap();
    let before_modified = std::fs::metadata(&database_path)
        .unwrap()
        .modified()
        .unwrap();

    let result = load_database_snapshot_at(&database_path, &dir.join("config.toml"), None);

    assert!(matches!(result, Ok(DatabaseOutcome::Loaded(_))));
    assert_eq!(std::fs::read(&database_path).unwrap(), before);
    assert_eq!(
        std::fs::metadata(&database_path)
            .unwrap()
            .modified()
            .unwrap(),
        before_modified
    );
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut sidecar = database_path.as_os_str().to_os_string();
        sidecar.push(suffix);
        assert!(!PathBuf::from(sidecar).exists());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cached_rows_appear_before_live_refresh_completes() {
    let snapshot = cached_snapshot(vec![
        persisted_archive(PathBuf::from("/roms/present.zip"), false),
        persisted_archive(PathBuf::from("/roms/missing.zip"), true),
    ]);

    let merged = build_display_rows(&[], &[], Some(&snapshot));

    assert_eq!(merged.len(), 2);
    assert!(
        merged
            .iter()
            .any(|row| row.origin == RowOrigin::CachedAwaitingValidation
                || row.origin == RowOrigin::CachedUnavailable)
    );
    assert!(
        merged
            .iter()
            .any(|row| row.origin == RowOrigin::CachedMissing)
    );
}

#[test]
fn cache_only_rows_cannot_resolve_to_a_live_record() {
    let snapshot = cached_snapshot(vec![persisted_archive(
        PathBuf::from("/roms/cache-only.zip"),
        false,
    )]);
    let merged = build_display_rows(&[], &[], Some(&snapshot));
    let cache_row = &merged[0];

    // No live records at all, so selecting the cache-only row's exact
    // path can never resolve to an ArchiveRecord - this is the same
    // fallback show_selected_archive already relies on to render zero
    // action buttons for `None`.
    assert_eq!(selected_record(&[], Some(&cache_row.path)), None);
}

#[test]
fn live_validation_enables_actions_for_a_confirmed_row() {
    let record = record_at(PathBuf::from("/roms/confirmed.zip"), MountState::Pending);
    let live_row = row_for(&record);
    let snapshot = cached_snapshot(vec![persisted_archive(
        PathBuf::from("/roms/confirmed.zip"),
        false,
    )]);

    let merged = build_display_rows(std::slice::from_ref(&record), &[live_row], Some(&snapshot));

    // The live row wins - the cache row for the same exact path is not
    // duplicated - and selecting it resolves to the live record, which
    // is what makes action buttons available.
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].origin, RowOrigin::Live);
    assert_eq!(
        selected_record(std::slice::from_ref(&record), Some(&merged[0].path)),
        Some(&record)
    );
}

#[test]
fn missing_cached_rows_remain_visible_in_the_merge() {
    let snapshot = cached_snapshot(vec![persisted_archive(
        PathBuf::from("/roms/gone.zip"),
        true,
    )]);

    let merged = build_display_rows(&[], &[], Some(&snapshot));

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].origin, RowOrigin::CachedMissing);
}

#[test]
fn newly_discovered_live_archive_not_yet_in_cache_appears_as_a_live_row() {
    let record = record_at(PathBuf::from("/roms/brand-new.zip"), MountState::Pending);
    let live_row = row_for(&record);
    let snapshot = cached_snapshot(vec![]);

    let merged = build_display_rows(std::slice::from_ref(&record), &[live_row], Some(&snapshot));

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].origin, RowOrigin::Live);
}

// -----------------------------------------------------------------
// Unknown-platform review workflow.
// -----------------------------------------------------------------

#[test]
fn live_row_with_a_persisted_manual_platform_is_not_classified_unknown() {
    // The crux of requirement 6: automatic detection found nothing
    // for this live record (no metadata/identity platform), but the
    // database already has a manual assignment for the same exact
    // path - the merged row must reflect the effective (manual)
    // platform, not the live-only "nothing detected" signal.
    let path = PathBuf::from("/roms/mystery.zip");
    let record = record_at(path.clone(), MountState::Pending);
    let live_row = row_for(&record);
    assert!(
        live_row.unknown_platform,
        "sanity check: live-only detection found nothing"
    );
    let snapshot = cached_snapshot(vec![persisted_archive_with_platform(
        path,
        1,
        "GameCube",
        MANUAL_PLATFORM_SOURCE,
    )]);

    let merged = build_display_rows(std::slice::from_ref(&record), &[live_row], Some(&snapshot));

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].origin, RowOrigin::Live);
    assert!(!merged[0].unknown_platform);
    assert_eq!(merged[0].platform, "GameCube");
}

#[test]
fn live_row_without_a_persisted_entry_keeps_its_live_only_classification() {
    // No database row exists for this archive yet (never scanned
    // into the library database) - there is no persisted effective
    // value to defer to, so the live-only signal is the only one
    // available and must be used as-is.
    let record = record_at(PathBuf::from("/roms/brand-new.zip"), MountState::Pending);
    let live_row = row_for(&record);
    let snapshot = cached_snapshot(vec![]);

    let merged = build_display_rows(std::slice::from_ref(&record), &[live_row], Some(&snapshot));

    assert_eq!(merged.len(), 1);
    assert!(merged[0].unknown_platform);
}

#[test]
fn cache_only_missing_row_with_no_platform_is_classified_unknown() {
    let snapshot = cached_snapshot(vec![persisted_archive(
        PathBuf::from("/roms/gone.zip"),
        true,
    )]);

    let merged = build_display_rows(&[], &[], Some(&snapshot));

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].origin, RowOrigin::CachedMissing);
    assert!(merged[0].unknown_platform);
}

#[test]
fn cache_only_missing_row_with_a_manual_platform_is_not_unknown() {
    let snapshot = cached_snapshot(vec![PersistedArchive {
        platform: Some("GameCube".to_string()),
        platform_source: Some(MANUAL_PLATFORM_SOURCE.to_string()),
        ..persisted_archive(PathBuf::from("/roms/gone.zip"), true)
    }]);

    let merged = build_display_rows(&[], &[], Some(&snapshot));

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].origin, RowOrigin::CachedMissing);
    assert!(!merged[0].unknown_platform);
}

#[test]
fn unknown_count_covers_every_merged_row_live_and_cache_only_alike() {
    let known_live_record = record_at(PathBuf::from("/roms/known-live.zip"), MountState::Pending);
    let known_live_row = row_for(&known_live_record);
    let unknown_live_record =
        record_at(PathBuf::from("/roms/unknown-live.zip"), MountState::Pending);
    let unknown_live_row = row_for(&unknown_live_record);
    let records = vec![known_live_record, unknown_live_record];
    let live_rows = vec![known_live_row, unknown_live_row];
    let snapshot = cached_snapshot(vec![
        persisted_archive_with_platform(
            PathBuf::from("/roms/known-live.zip"),
            1,
            "GameCube",
            MANUAL_PLATFORM_SOURCE,
        ),
        persisted_archive(PathBuf::from("/roms/unknown-cached.zip"), false),
        persisted_archive_with_platform(
            PathBuf::from("/roms/known-cached.zip"),
            2,
            "SNES",
            "folder_alias",
        ),
    ]);

    let merged = build_display_rows(&records, &live_rows, Some(&snapshot));

    assert_eq!(
        merged.len(),
        4,
        "sanity check: two live + two cache-only rows"
    );
    let unknown_count = merged.iter().filter(|row| row.unknown_platform).count();
    assert_eq!(
        unknown_count, 2,
        "unknown-live.zip and unknown-cached.zip - the manual and known-automatic rows must not count"
    );
}

#[test]
fn unknown_platform_aggregate_headline_uses_singular_and_plural_correctly() {
    assert_eq!(
        unknown_platform_aggregate_headline(1),
        "1 entry with unknown platform"
    );
    assert_eq!(
        unknown_platform_aggregate_headline(0),
        "0 entries with unknown platform"
    );
    assert_eq!(
        unknown_platform_aggregate_headline(8_257),
        "8257 entries with unknown platform"
    );
}

#[test]
fn detected_platform_counts_reflects_only_platforms_actually_present() {
    let platforms = [
        Some("GameCube"),
        Some("Sharp X68000"),
        Some("GameCube"),
        None,
        Some("Virtual Boy"),
        None,
    ];
    let summary = detected_platform_counts(platforms.into_iter());
    assert_eq!(
        summary.named,
        vec![
            ("GameCube".to_string(), 2),
            ("Sharp X68000".to_string(), 1),
            ("Virtual Boy".to_string(), 1),
        ],
        "every non-zero platform must appear, sorted, with its real count"
    );
    assert_eq!(summary.unknown, 2);

    // A canonical platform the registry recognises but that has zero
    // archives must never appear (no fixed list is consulted here).
    assert!(!summary.named.iter().any(|(platform, _)| platform == "PS3"),);
}

#[test]
fn unsupported_platform_banner_names_the_recognised_platform_not_generic_text() {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.platform = Some("Sharp X68000".to_string());
    workflow.adapter = CheatEmulatorAdapter::Unsupported;
    let mut clipboard = InMemoryClipboard::default();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_cheats_mods_page(
                ui,
                app.cheat_workflow.as_mut(),
                &app.retroarch_profiles,
                &app.pcsx2_profiles,
                &app.dolphin_profiles,
                &app.xenia_profiles,
                None,
                None,
                &app.history,
                false,
                &mut clipboard,
                &mut dolphin_texture_mod_page::DolphinTextureModPageState::default(),
            );
        });
    });
    assert!(rendered_text_contains(&output, "Sharp X68000 recognised"));
    assert!(rendered_text_contains(
        &output,
        "cheat support is not available yet"
    ));
    assert!(
        !rendered_text_contains(&output, "no Cheats & Mods adapter for this archive"),
        "the generic, unnamed message must be gone"
    );
}

#[test]
fn unknown_platform_banner_is_visible_only_when_the_filter_is_active_and_there_is_something_to_explain()
 {
    let mut filters = LibraryRowFilters::default();
    assert!(
        !unknown_platform_banner_visible(&filters, 5),
        "must not show before the user asks to see Unknown-platform rows"
    );

    filters.unknown_platform = true;
    assert!(
        !unknown_platform_banner_visible(&filters, 0),
        "must not show an explanation for zero entries"
    );
    assert!(unknown_platform_banner_visible(&filters, 5));
}

#[test]
fn catalogue_status_load_needed_covers_sources_and_cheats_mods_only() {
    for view in [MainView::Sources, MainView::CheatsMods] {
        assert!(
            catalogue_status_load_needed(view, &CatalogueManagerState::NotLoaded),
            "{view:?} must lazily load catalogue status when not yet loaded"
        );
    }
    for view in [MainView::Library, MainView::Selected, MainView::Mount] {
        assert!(
            !catalogue_status_load_needed(view, &CatalogueManagerState::NotLoaded),
            "{view:?} has no catalogue UI and must not trigger a load"
        );
    }
    let (_, _, list) = cheat_source_list_fixture();
    assert!(!catalogue_status_load_needed(
        MainView::Sources,
        &CatalogueManagerState::Ready(list)
    ));
}

#[test]
fn handle_catalogue_manager_action_review_then_confirm_requires_both_steps() {
    let mut app = app_for_operation_tests();
    assert!(app.catalogue_review.is_none());
    assert!(app.catalogue_retrieval.is_none());
    let context = egui::Context::default();

    app.handle_catalogue_manager_action(
        &context,
        CatalogueManagerAction::Review {
            source_id: "libretro-buildbot-cheats".into(),
            kind: CatalogueRetrievalKind::Update,
        },
    );
    assert!(
        app.catalogue_retrieval.is_none(),
        "reviewing an update must never itself start network access"
    );
    assert_eq!(
        app.catalogue_review
            .as_ref()
            .map(|review| &review.source_id),
        Some(&"libretro-buildbot-cheats".to_string())
    );

    app.handle_catalogue_manager_action(&context, CatalogueManagerAction::Confirm);
    assert!(
        app.catalogue_retrieval.is_some(),
        "confirming the reviewed action starts the retrieval"
    );
    assert!(
        app.catalogue_review.is_none(),
        "the review is consumed once confirmed"
    );
}

#[test]
fn handle_catalogue_manager_action_cancel_review_clears_it_without_starting_retrieval() {
    let mut app = app_for_operation_tests();
    let context = egui::Context::default();
    app.handle_catalogue_manager_action(
        &context,
        CatalogueManagerAction::Review {
            source_id: "libretro-buildbot-cheats".into(),
            kind: CatalogueRetrievalKind::Download,
        },
    );
    app.handle_catalogue_manager_action(&context, CatalogueManagerAction::CancelReview);
    assert!(app.catalogue_review.is_none());
    assert!(app.catalogue_retrieval.is_none());
}

fn dolphin_catalogue_fixture(fetched_at_unix_seconds: u64) -> DolphinCatalogue {
    DolphinCatalogue {
            metadata: DolphinCatalogueMetadata {
                schema_version: DOLPHIN_CATALOGUE_SCHEMA_VERSION,
                repository: DOLPHIN_CATALOGUE_REPOSITORY.to_string(),
                canonical_repository_url: "https://github.com/dolphin-emu/dolphin".to_string(),
                resolved_commit: "d742aa8b4c4d052f7dceaa39022b1fe3996f1781".to_string(),
                source_archive_url:
                    "https://codeload.github.com/dolphin-emu/dolphin/zip/d742aa8b4c4d052f7dceaa39022b1fe3996f1781"
                        .to_string(),
                license: "GPL-2.0-or-later".to_string(),
                license_url: "https://github.com/dolphin-emu/dolphin/blob/master/COPYING".to_string(),
                attribution: "Gecko definitions from the Dolphin Emulator upstream Data/Sys/GameSettings dataset."
                    .to_string(),
                fetched_at_unix_seconds,
                archive_sha256: "0".repeat(64),
                downloaded_bytes: 21_269_178,
                archive_entry_count: 7_122,
                game_settings_files_inspected: 1_875,
                games_with_usable_gecko: 1,
                total_usable_gecko_entries: 1,
                malformed_or_skipped_files: 0,
                non_matching_files_skipped: 0,
                warnings: Vec::new(),
            },
            games: vec![DolphinCatalogueGame {
                game_id: "GAFE01".to_string(),
                title: Some("Animal Crossing".to_string()),
                region: GeckoRegion::Usa,
                source_relative_path: "Data/Sys/GameSettings/GAFE01.ini".to_string(),
                codes: vec![],
                file_warnings: vec![],
            }],
        }
}

#[test]
fn dolphin_catalogue_status_load_needed_covers_cheats_mods_only() {
    assert!(dolphin_catalogue_status_load_needed(
        MainView::CheatsMods,
        &DolphinCatalogueManagerState::NotLoaded
    ));
    for view in [
        MainView::Library,
        MainView::Sources,
        MainView::Selected,
        MainView::Mount,
    ] {
        assert!(
            !dolphin_catalogue_status_load_needed(view, &DolphinCatalogueManagerState::NotLoaded),
            "{view:?} has no Dolphin catalogue UI and must not trigger a load"
        );
    }
    assert!(!dolphin_catalogue_status_load_needed(
        MainView::CheatsMods,
        &DolphinCatalogueManagerState::Ready(Box::new(DolphinCatalogueStatusSnapshot {
            catalogue: None,
            last_check_unix_seconds: None,
        }))
    ));
}

#[test]
fn dolphin_catalogue_card_shows_the_no_catalogue_prompt_when_nothing_is_downloaded() {
    let ctx = egui::Context::default();
    let mut clipboard = InMemoryClipboard::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_dolphin_catalogue_manager(
                ui,
                &DolphinCatalogueManagerState::Ready(Box::new(DolphinCatalogueStatusSnapshot {
                    catalogue: None,
                    last_check_unix_seconds: None,
                })),
                None,
                None,
                DolphinCatalogueCardContext {
                    review: None,
                    update_available: None,
                    remove_confirm: false,
                    now_unix_seconds: 1_700_000_000,
                },
                &mut clipboard,
            );
        });
    });
    assert!(rendered_text_contains(
        &output,
        "Dolphin cheat catalogue not downloaded"
    ));
    assert!(rendered_text_contains(&output, "Download catalogue"));
    assert!(!rendered_text_contains(&output, "Update catalogue"));
}

#[test]
fn dolphin_catalogue_card_shows_ready_summary_and_flags_an_available_update() {
    let ctx = egui::Context::default();
    let mut clipboard = InMemoryClipboard::default();
    let catalogue = dolphin_catalogue_fixture(1_700_000_000);
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_dolphin_catalogue_manager(
                ui,
                &DolphinCatalogueManagerState::Ready(Box::new(DolphinCatalogueStatusSnapshot {
                    catalogue: Some(catalogue.clone()),
                    last_check_unix_seconds: Some(1_700_000_000),
                })),
                None,
                None,
                DolphinCatalogueCardContext {
                    review: None,
                    update_available: None,
                    remove_confirm: false,
                    now_unix_seconds: 1_700_000_000,
                },
                &mut clipboard,
            );
        });
    });
    assert!(rendered_text_contains(&output, "Dolphin catalogue ready"));
    assert!(rendered_text_contains(&output, "1 games"));
    assert!(rendered_text_contains(&output, "1 cheats"));
    assert!(!rendered_text_contains(&output, "Update available"));

    // Flagged by an explicit "Check for updates" result, even though
    // the catalogue is not old enough to be stale on its own.
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_dolphin_catalogue_manager(
                ui,
                &DolphinCatalogueManagerState::Ready(Box::new(DolphinCatalogueStatusSnapshot {
                    catalogue: Some(catalogue.clone()),
                    last_check_unix_seconds: Some(1_700_000_000),
                })),
                None,
                None,
                DolphinCatalogueCardContext {
                    review: None,
                    update_available: Some(true),
                    remove_confirm: false,
                    now_unix_seconds: 1_700_000_000,
                },
                &mut clipboard,
            );
        });
    });
    assert!(rendered_text_contains(&output, "Update available"));
}

#[test]
fn handle_dolphin_catalogue_manager_action_review_then_confirm_requires_both_steps() {
    let mut app = app_for_operation_tests();
    assert!(app.dolphin_catalogue_review.is_none());
    assert!(app.dolphin_catalogue_retrieval.is_none());
    let context = egui::Context::default();

    app.handle_dolphin_catalogue_manager_action(
        &context,
        DolphinCatalogueManagerAction::Review(DolphinCatalogueRetrievalKind::Download),
    );
    assert!(
        app.dolphin_catalogue_retrieval.is_none(),
        "reviewing a download must never itself start network access"
    );
    assert_eq!(
        app.dolphin_catalogue_review,
        Some(DolphinCatalogueRetrievalKind::Download)
    );

    app.handle_dolphin_catalogue_manager_action(&context, DolphinCatalogueManagerAction::Confirm);
    assert!(
        app.dolphin_catalogue_retrieval.is_some(),
        "confirming the reviewed action starts the retrieval"
    );
    assert!(
        app.dolphin_catalogue_review.is_none(),
        "the review is consumed once confirmed"
    );
}

#[test]
fn handle_dolphin_catalogue_manager_action_cancel_review_clears_it_without_starting_retrieval() {
    let mut app = app_for_operation_tests();
    let context = egui::Context::default();
    app.handle_dolphin_catalogue_manager_action(
        &context,
        DolphinCatalogueManagerAction::Review(DolphinCatalogueRetrievalKind::Update),
    );
    app.handle_dolphin_catalogue_manager_action(
        &context,
        DolphinCatalogueManagerAction::CancelReview,
    );
    assert!(app.dolphin_catalogue_review.is_none());
    assert!(app.dolphin_catalogue_retrieval.is_none());
}

#[test]
fn handle_dolphin_catalogue_manager_action_remove_requires_explicit_confirmation() {
    let mut app = app_for_operation_tests();
    let context = egui::Context::default();
    assert!(!app.dolphin_catalogue_remove_confirm);

    app.handle_dolphin_catalogue_manager_action(
        &context,
        DolphinCatalogueManagerAction::RequestRemove,
    );
    assert!(
        app.dolphin_catalogue_remove_confirm,
        "removal must require a confirmation dialog before anything happens"
    );

    app.handle_dolphin_catalogue_manager_action(
        &context,
        DolphinCatalogueManagerAction::CancelRemove,
    );
    assert!(!app.dolphin_catalogue_remove_confirm);
}

#[test]
fn dolphin_local_lookup_state_defaults_to_not_attempted_on_a_fresh_workflow() {
    let app = dolphin_workflow_with_matched_identity(Path::new("/isolated/dolphin-test"), "GALE01");
    assert_eq!(
        app.cheat_workflow.as_ref().unwrap().dolphin_local_lookup,
        DolphinLocalLookupState::NotAttempted
    );
}

#[test]
fn dolphin_beginner_status_distinguishes_still_looking_from_nothing_found_locally() {
    let mut app =
        dolphin_workflow_with_matched_identity(Path::new("/isolated/dolphin-test"), "GALE01");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.dolphin_profile_selection = Some(EmulatorProfileSelection::Auto {
        profile_id: "profile".to_string(),
        reason: archivefs_core::patch_manager::EmulatorProfileSelectReason::ExplicitChoice,
    });
    // Nothing attempted yet: still the "finding compatible cheats" spinner.
    assert_eq!(
        dolphin_beginner_status(workflow),
        BeginnerCheatStatus::FindingCompatibleCheats
    );
    // The local catalogue/cache lookup ran and found nothing: an honest
    // "nothing found" state, not a spinner that would never resolve.
    workflow.dolphin_local_lookup = DolphinLocalLookupState::NoCatalogueInstalled;
    assert_eq!(
        dolphin_beginner_status(workflow),
        BeginnerCheatStatus::NoCompatibleCheatsFound
    );
}

#[test]
fn platform_section_explains_unknown_platform_only_when_it_is_actually_unknown() {
    let persisted = persisted_archive(PathBuf::from("/roms/mystery.zip"), false);
    let unknown_details = PlatformProvenanceDetails {
        platform: None,
        source: None,
        matched_component: None,
        automatic_fallback: None,
    };
    let mut clipboard = InMemoryClipboard::default();
    let mut choice = None;
    let mut custom = String::new();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_platform_section(
                ui,
                Some(&persisted),
                Some(&unknown_details),
                &mut choice,
                &mut custom,
                false,
                &mut clipboard,
            );
        });
    });
    assert!(rendered_text_contains(&output, "Why is this Unknown?"));
    assert!(rendered_text_contains(
        &output,
        "Assign a platform manually below"
    ));

    let known_details = PlatformProvenanceDetails {
        platform: Some("SNES".to_string()),
        source: Some("heuristic-path-detector".to_string()),
        matched_component: None,
        automatic_fallback: None,
    };
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_platform_section(
                ui,
                Some(&persisted),
                Some(&known_details),
                &mut choice,
                &mut custom,
                false,
                &mut clipboard,
            );
        });
    });
    assert!(
        !rendered_text_contains(&output, "Why is this Unknown?"),
        "a recognised platform must not show the Unknown explanation"
    );
}

#[test]
fn corrupt_database_is_non_fatal() {
    let dir = database_test_dir("corrupt");
    let database_path = dir.join("library.sqlite3");
    std::fs::write(&database_path, b"not a sqlite database").unwrap();

    let config_path = dir.join("config.toml");
    let result = load_database_snapshot_at(&database_path, &config_path, None);

    assert!(matches!(result, Err(DatabaseLoadError::Failed { .. })));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn classify_unhealthy_database_reports_future_schema_as_a_clear_upgrade_message() {
    let health = DatabaseHealth {
        resolved_path: PathBuf::from("/config/library.sqlite3"),
        database_exists: true,
        database_opens: true,
        schema_version: Some(latest_schema_version() + 1),
        migrations_current: false,
        foreign_keys_enabled: true,
        error: None,
    };

    let error = classify_unhealthy_database(health);

    match error {
        DatabaseLoadError::Failed { message } => {
            assert!(message.contains("newer than this build"));
        }
        DatabaseLoadError::NotCreated { .. } => panic!("expected Failed, got NotCreated"),
        DatabaseLoadError::Outdated { .. } => panic!("expected Failed, got Outdated"),
    }
}

#[test]
fn classify_unhealthy_database_reports_a_merely_old_schema_as_outdated_not_an_error() {
    let health = DatabaseHealth {
        resolved_path: PathBuf::from("/config/library.sqlite3"),
        database_exists: true,
        database_opens: true,
        schema_version: Some(0),
        migrations_current: false,
        foreign_keys_enabled: true,
        error: None,
    };

    let error = classify_unhealthy_database(health);

    assert!(matches!(error, DatabaseLoadError::Outdated { .. }));
}

#[test]
fn classify_unhealthy_database_reports_unopenable_database_as_failed_with_its_error() {
    let health = DatabaseHealth {
        resolved_path: PathBuf::from("/config/library.sqlite3"),
        database_exists: true,
        database_opens: false,
        schema_version: None,
        migrations_current: false,
        foreign_keys_enabled: false,
        error: Some("disk I/O error".to_string()),
    };

    let error = classify_unhealthy_database(health);

    match error {
        DatabaseLoadError::Failed { message } => assert_eq!(message, "disk I/O error"),
        _ => panic!("expected Failed"),
    }
}

#[test]
fn scan_partial_success_reports_folder_errors_without_failing_the_scan() {
    let dir = database_test_dir("scan-partial");
    let source_a = dir.join("source-a");
    let source_b = dir.join("source-b");
    let mount = dir.join("mount");
    write_archive_file(&source_a, "a.zip", b"a");
    write_archive_file(&source_b, "b.zip", b"b");
    let config = Config {
        source_folders: vec![source_a.clone(), source_b.clone()],
        mount_root: mount,
        ratarmount_bin: "ratarmount".to_string(),
        master_rom_root: None,
    };
    let database_path = dir.join("library.sqlite3");
    {
        let mut database = Database::open_or_create(&database_path).unwrap();
        scan_and_persist(&mut database, &config, "test").unwrap();
    }
    std::fs::remove_dir_all(&source_a).unwrap();

    let config_path = dir.join("config.toml");
    let result = load_database_snapshot_at(&database_path, &config_path, Some(&config));

    match result {
        Ok(DatabaseOutcome::Scanned {
            snapshot,
            scan_summary,
        }) => {
            assert_eq!(scan_summary.folder_errors.len(), 1);
            assert_eq!(scan_summary.folder_errors[0].0, source_a);
            // Archives under the still-reachable folder remain in the
            // catalogue - a partial failure does not crash or discard
            // the rest of the scan.
            assert!(
                snapshot
                    .archives
                    .iter()
                    .any(|archive| archive.relative_path == Path::new("b.zip"))
            );
        }
        _ => panic!("expected a partially-successful Scanned outcome"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn successful_scan_refreshes_cached_counts() {
    let dir = database_test_dir("scan-success");
    let source = dir.join("source");
    let mount = dir.join("mount");
    write_archive_file(&source, "game.zip", b"game data");
    let config = config_for(&source, &mount);
    let database_path = dir.join("library.sqlite3");

    let config_path = dir.join("config.toml");
    let result = load_database_snapshot_at(&database_path, &config_path, Some(&config));

    match result {
        Ok(DatabaseOutcome::Scanned {
            snapshot,
            scan_summary,
        }) => {
            assert_eq!(scan_summary.counts.archives_added, 1);
            assert_eq!(snapshot.stats.total_archives, 1);
            assert_eq!(snapshot.archives.len(), 1);
        }
        _ => panic!("expected a successful Scanned outcome"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn database_worker_disconnect_is_surfaced_as_an_error() {
    let mut app = app_for_operation_tests();
    let generation = DatabaseGeneration::INITIAL.next();
    app.database_generation = generation;
    let (sender, receiver) = mpsc::channel::<DatabaseMessage>();
    drop(sender);
    app.database_state = DatabaseState::Loading {
        generation,
        receiver,
        worker: None,
        previous: None,
        scanning: false,
    };

    app.poll_database_load(&egui::Context::default());

    match &app.database_state {
        DatabaseState::Error { message, .. } => {
            assert!(message.contains("stopped unexpectedly"));
        }
        _ => panic!("expected a disconnected worker to surface as DatabaseState::Error"),
    }
}

#[test]
fn late_database_results_from_an_older_generation_are_ignored() {
    let mut app = app_for_operation_tests();
    let stale_generation = DatabaseGeneration::INITIAL;
    let current_generation = stale_generation.next();
    app.database_generation = current_generation;
    let (sender, receiver) = mpsc::channel::<DatabaseMessage>();
    app.database_state = DatabaseState::Loading {
        generation: stale_generation,
        receiver,
        worker: None,
        previous: None,
        scanning: false,
    };
    let stale_snapshot = cached_snapshot(vec![persisted_archive(
        PathBuf::from("/roms/stale.zip"),
        false,
    )]);
    sender
        .send((
            stale_generation,
            Ok(DatabaseOutcome::Loaded(stale_snapshot)),
        ))
        .unwrap();

    app.poll_database_load(&egui::Context::default());

    // The message's generation does not match the current generation,
    // so it must be dropped entirely - the state must still be the
    // same stale Loading value, never overwritten with the stale
    // snapshot's data.
    assert!(matches!(
        &app.database_state,
        DatabaseState::Loading { generation, .. } if *generation == stale_generation
    ));
}

#[test]
#[cfg(unix)]
fn reconciliation_uses_exact_path_bytes_not_lossy_display_strings() {
    // Two distinct invalid-UTF-8 byte sequences that both decode to
    // the same lossy "fo<REPLACEMENT>o.zip" under Path::display() -
    // see database.rs's own non_utf8_path_round_trips_exactly_through_a_blob_column
    // test for why 0x80/0x81 alone are never valid UTF-8 continuation
    // bytes here.
    let bytes_a: Vec<u8> = vec![0x66, 0x6f, 0x80, 0x6f, b'.', b'z', b'i', b'p'];
    let bytes_b: Vec<u8> = vec![0x66, 0x6f, 0x81, 0x6f, b'.', b'z', b'i', b'p'];
    let path_a = PathBuf::from(OsString::from_vec(bytes_a));
    let path_b = PathBuf::from(OsString::from_vec(bytes_b));
    assert_ne!(
        path_a, path_b,
        "the two test paths must differ in exact bytes"
    );
    assert_eq!(
        path_a.display().to_string(),
        path_b.display().to_string(),
        "the two test paths must collide under lossy display - that is the point"
    );

    let record = record_at(path_a, MountState::Pending);
    let live_row = row_for(&record);
    let snapshot = cached_snapshot(vec![persisted_archive(path_b, false)]);

    let merged = build_display_rows(std::slice::from_ref(&record), &[live_row], Some(&snapshot));

    // If reconciliation had compared lossy display strings instead of
    // exact bytes, these two different archives would have been
    // wrongly treated as the same one and collapsed into a single row.
    assert_eq!(merged.len(), 2);
}

#[test]
#[cfg(unix)]
fn non_utf8_paths_reconcile_correctly_on_unix() {
    let bytes: Vec<u8> = vec![0x66, 0x6f, 0x80, 0x6f, b'.', b'z', b'i', b'p'];
    let path = PathBuf::from(OsString::from_vec(bytes));
    assert!(
        path.to_str().is_none(),
        "test path must actually be invalid UTF-8"
    );

    let record = record_at(path.clone(), MountState::Pending);
    let live_row = row_for(&record);
    let snapshot = cached_snapshot(vec![persisted_archive(path, false)]);

    let merged = build_display_rows(std::slice::from_ref(&record), &[live_row], Some(&snapshot));

    // Identical non-UTF-8 bytes on both sides must be recognized as
    // the same archive - the cache-only entry must be suppressed, not
    // duplicated.
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].origin, RowOrigin::Live);
}

#[test]
fn scan_library_action_does_not_block_on_a_slow_background_worker() {
    // Deliberately does not call the real start_database_action/
    // start_database_load (those spawn a real thread that touches the
    // real default database/config paths - see load_database_snapshot -
    // which every other stage 4 test in this file avoids for exactly
    // that reason, matching how the existing live-snapshot tests never
    // call the real start_load/refresh either). Instead this drives
    // the same Loading state and channel those functions would have
    // produced by hand, with nothing sent on it yet, and proves
    // poll_database_load's use of try_recv (not recv) means polling an
    // in-progress scan never blocks the UI thread waiting for a result.
    let mut app = app_for_operation_tests();
    let generation = DatabaseGeneration::INITIAL;
    app.database_generation = generation;
    let (_sender, receiver) = mpsc::channel::<DatabaseMessage>();
    app.database_state = DatabaseState::Loading {
        generation,
        receiver,
        worker: None,
        previous: None,
        scanning: true,
    };

    app.poll_database_load(&egui::Context::default());

    assert!(matches!(
        app.database_state,
        DatabaseState::Loading { scanning: true, .. }
    ));
}

#[test]
fn active_database_worker_cannot_be_replaced_and_is_joined_on_shutdown() {
    let mut app = app_for_operation_tests();
    let generation = DatabaseGeneration::INITIAL.next();
    app.database_generation = generation;
    let (_sender, receiver) = mpsc::channel::<DatabaseMessage>();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let completed = Arc::new(AtomicBool::new(false));
    let worker_barrier = Arc::clone(&barrier);
    let worker_completed = Arc::clone(&completed);
    let worker = thread::spawn(move || {
        worker_barrier.wait();
        worker_completed.store(true, Ordering::SeqCst);
    });
    app.database_state = DatabaseState::Loading {
        generation,
        receiver,
        worker: Some(worker),
        previous: None,
        scanning: true,
    };

    app.start_database_action(egui::Context::default(), false);
    assert_eq!(app.database_generation, generation);
    assert!(matches!(
        app.database_state,
        DatabaseState::Loading { scanning: true, .. }
    ));

    barrier.wait();
    drop(app);
    assert!(completed.load(Ordering::SeqCst));
}

#[test]
fn database_scan_completing_while_a_live_refresh_is_active_does_not_panic() {
    let mut app = app_for_operation_tests();
    app.state = LoadState::Loading {
        generation: RefreshGeneration::INITIAL,
        receiver: mpsc::channel().1,
        previous: None,
    };
    let generation = DatabaseGeneration::INITIAL.next();
    app.database_generation = generation;
    let (sender, receiver) = mpsc::channel::<DatabaseMessage>();
    app.database_state = DatabaseState::Loading {
        generation,
        receiver,
        worker: None,
        previous: None,
        scanning: false,
    };
    let snapshot = cached_snapshot(vec![persisted_archive(
        PathBuf::from("/roms/during-refresh.zip"),
        false,
    )]);
    sender
        .send((generation, Ok(DatabaseOutcome::Loaded(snapshot))))
        .unwrap();

    app.poll_database_load(&egui::Context::default());

    // No live snapshot exists yet, so the cached filtered-index
    // recompute is a no-op - the important thing is that resolving
    // the database mid-live-refresh does not panic and leaves the
    // database state correctly Ready.
    assert!(matches!(app.database_state, DatabaseState::Ready { .. }));
    assert!(matches!(app.state, LoadState::Loading { .. }));
}

// -------------------------------------------------------------------
// Sources-page scan results feeding `last_scan_summary`
// (bug: SourceActionOutcome::Scanned's summary was discarded, so the
// Database Status "Skipped files -> Inspect..." control was only ever
// reachable via a separate Database Status -> Scan library run).
// -------------------------------------------------------------------

/// Drives `poll_source_action` with a `SourceActionOutcome::Scanned`
/// result, then drives the plain reload it triggers to completion via a
/// manually-controlled channel - never the real
/// `start_database_action`/`start_database_load` worker (which would
/// touch the process's real default database/config paths; the same
/// concern `start_source_action_does_not_start_a_second_concurrent_action`
/// documents). `database_state` is pre-seeded already `Loading` at the
/// current generation so `start_database_action` (called internally by
/// `poll_source_action`) sees `is_loading() == true` and returns
/// immediately without spawning a thread or touching `database_state`,
/// leaving our channel in full control of what "reload" observes.
fn drive_sources_scan_through_to_ready(
    summary: ScanPersistSummary,
) -> (ArchiveFsApp, ScanPersistSummary) {
    let mut app = app_for_operation_tests();
    let generation = DatabaseGeneration::INITIAL.next();
    app.database_generation = generation;
    let (db_sender, db_receiver) = mpsc::channel::<DatabaseMessage>();
    app.database_state = DatabaseState::Loading {
        generation,
        receiver: db_receiver,
        worker: None,
        previous: None,
        scanning: false,
    };

    let (source_sender, source_receiver) = mpsc::channel();
    app.source_action = Some(RunningSourceAction {
        action: SourceAction::ScanAll,
        receiver: source_receiver,
        worker: None,
    });
    source_sender
        .send(Ok(SourceActionOutcome::Scanned(summary.clone())))
        .unwrap();

    app.poll_source_action(&egui::Context::default());

    // The reload `poll_source_action` asked for was a no-op (already
    // loading), so `database_state` is still exactly the `Loading` we
    // seeded - now complete it as a plain (non-scan) reload, exactly
    // as the real worker would.
    let snapshot = cached_snapshot(Vec::new());
    db_sender
        .send((generation, Ok(DatabaseOutcome::Loaded(snapshot))))
        .unwrap();
    app.poll_database_load(&egui::Context::default());

    (app, summary)
}

#[test]
fn sources_page_scan_with_skipped_files_populates_last_scan_summary() {
    let summary = skipped_files_summary(
        vec![archivefs_core::SkippedFile {
            path: PathBuf::from("/roms/c128/boxart.png"),
            reason: archivefs_core::SkipReason::UnsupportedExtension,
        }],
        2,
        1,
    );
    let (app, _) = drive_sources_scan_through_to_ready(summary);

    match &app.database_state {
        DatabaseState::Ready {
            last_scan_summary, ..
        } => {
            assert!(
                last_scan_summary.is_some(),
                "a Sources-page scan with skipped files must populate last_scan_summary"
            );
        }
        _ => panic!("expected DatabaseState::Ready"),
    }
    // Consumed exactly once: it must never leak into a later, unrelated
    // reload.
    assert!(app.pending_source_scan_summary.is_none());
}

#[test]
fn sources_page_scan_preserves_the_exact_skipped_files_total() {
    let summary = skipped_files_summary(Vec::new(), 5, 2);
    let expected_total = summary.skipped_files_total();
    assert_eq!(expected_total, 7);
    let (app, _) = drive_sources_scan_through_to_ready(summary);

    let DatabaseState::Ready {
        last_scan_summary: Some(stored),
        ..
    } = &app.database_state
    else {
        panic!("expected Ready with a stored last_scan_summary");
    };
    assert_eq!(stored.skipped_files_total(), expected_total);
}

#[test]
fn sources_page_scan_status_text_is_unchanged_by_the_last_scan_summary_fix() {
    // Same wording assertion as
    // `source_action_success_messages_are_specific_per_variant`'s own
    // Scanned coverage - this fix only changes what happens to the
    // summary afterward, never the feedback text shown for the action
    // itself.
    let summary = skipped_files_summary(Vec::new(), 2, 1);
    let (app, _) = drive_sources_scan_through_to_ready(summary);

    let feedback = app.feedback.as_ref().expect("expected feedback to be set");
    assert!(feedback.succeeded);
    assert_eq!(
        feedback.message,
        "Scan complete: 0 source(s) scanned, 0 archive(s) found, 0 missing."
    );
}

#[test]
fn database_status_inspector_condition_becomes_true_after_a_sources_page_scan() {
    let summary = skipped_files_summary(
        vec![archivefs_core::SkippedFile {
            path: PathBuf::from("/roms/megadrive/RESOURCE.GEN"),
            reason: archivefs_core::SkipReason::AmbiguousPlatform,
        }],
        0,
        1,
    );
    let (app, _) = drive_sources_scan_through_to_ready(summary);

    // The exact guard `show_database_panel` uses to decide whether to
    // render the "Skipped files" row and its "Inspect..." button.
    let inspector_reachable = matches!(
        &app.database_state,
        DatabaseState::Ready {
            last_scan_summary: Some(summary),
            ..
        } if summary.skipped_files_total() > 0
    );
    assert!(
        inspector_reachable,
        "Database Status -> Skipped files -> Inspect... must become reachable \
             immediately after a Sources-page scan with skipped files"
    );
}

#[test]
fn plain_reloads_unrelated_to_a_sources_scan_still_leave_last_scan_summary_none() {
    // No regression: a reload not preceded by a Sources-page scan
    // (pending_source_scan_summary left at its default None) must
    // behave exactly as before this fix.
    let mut app = app_for_operation_tests();
    let generation = DatabaseGeneration::INITIAL.next();
    app.database_generation = generation;
    let (sender, receiver) = mpsc::channel::<DatabaseMessage>();
    app.database_state = DatabaseState::Loading {
        generation,
        receiver,
        worker: None,
        previous: None,
        scanning: false,
    };
    assert!(app.pending_source_scan_summary.is_none());
    let snapshot = cached_snapshot(Vec::new());
    sender
        .send((generation, Ok(DatabaseOutcome::Loaded(snapshot))))
        .unwrap();

    app.poll_database_load(&egui::Context::default());

    match &app.database_state {
        DatabaseState::Ready {
            last_scan_summary, ..
        } => assert!(last_scan_summary.is_none()),
        _ => panic!("expected DatabaseState::Ready"),
    }
}

#[test]
fn the_database_status_scan_library_path_still_populates_last_scan_summary_directly() {
    // No regression to `DatabaseOutcome::Scanned` (the separate
    // Database Status -> "Scan library" flow): it must keep populating
    // `last_scan_summary` itself, exactly as before this fix, and must
    // never consult `pending_source_scan_summary`.
    let mut app = app_for_operation_tests();
    let generation = DatabaseGeneration::INITIAL.next();
    app.database_generation = generation;
    app.pending_source_scan_summary = None;
    let (sender, receiver) = mpsc::channel::<DatabaseMessage>();
    app.database_state = DatabaseState::Loading {
        generation,
        receiver,
        worker: None,
        previous: None,
        scanning: true,
    };
    let snapshot = cached_snapshot(Vec::new());
    let scan_summary = skipped_files_summary(Vec::new(), 1, 0);
    sender
        .send((
            generation,
            Ok(DatabaseOutcome::Scanned {
                snapshot,
                scan_summary: scan_summary.clone(),
            }),
        ))
        .unwrap();

    app.poll_database_load(&egui::Context::default());

    match &app.database_state {
        DatabaseState::Ready {
            last_scan_summary: Some(stored),
            ..
        } => assert_eq!(stored.skipped_files_total(), 1),
        _ => panic!("expected Ready with a scan summary"),
    }
}

fn row_with_origin(origin: RowOrigin, unknown_platform: bool) -> ArchiveRow {
    let mut row = row("");
    row.origin = origin;
    row.unknown_platform = unknown_platform;
    row
}

#[test]
fn library_row_filters_default_hides_nothing() {
    let filters = LibraryRowFilters::default();
    assert!(!filters.is_active());
    assert!(filters.matches(&row_with_origin(RowOrigin::Live, false)));
    assert!(filters.matches(&row_with_origin(RowOrigin::CachedMissing, true)));
}

#[test]
fn library_row_filters_present_only_shows_only_live_rows() {
    let filters = LibraryRowFilters {
        present: true,
        ..LibraryRowFilters::default()
    };

    assert!(filters.matches(&row_with_origin(RowOrigin::Live, false)));
    assert!(!filters.matches(&row_with_origin(RowOrigin::CachedMissing, false)));
    assert!(!filters.matches(&row_with_origin(RowOrigin::CachedAwaitingValidation, false)));
}

#[test]
fn library_row_filters_platform_groups_are_independent_of_state_groups() {
    let filters = LibraryRowFilters {
        missing: true,
        known_platform: true,
        ..LibraryRowFilters::default()
    };

    // A missing row with an unknown platform must fail the platform
    // group even though it passes the state group - both active
    // groups must match (AND across groups, OR within a group).
    assert!(!filters.matches(&row_with_origin(RowOrigin::CachedMissing, true)));
    assert!(filters.matches(&row_with_origin(RowOrigin::CachedMissing, false)));
    assert!(!filters.matches(&row_with_origin(RowOrigin::Live, false)));
}

#[test]
fn show_unknown_only_combines_with_the_present_filter() {
    // Requirement 3's exact example: present + unknown-only means
    // present unknown rows only - missing rows stay excluded even
    // though they are also unknown.
    let filters = LibraryRowFilters {
        present: true,
        unknown_platform: true,
        ..LibraryRowFilters::default()
    };

    assert!(filters.matches(&row_with_origin(RowOrigin::Live, true)));
    assert!(!filters.matches(&row_with_origin(RowOrigin::Live, false)));
    assert!(!filters.matches(&row_with_origin(RowOrigin::CachedMissing, true)));
}

#[test]
fn show_unknown_only_combines_with_the_missing_filter() {
    let filters = LibraryRowFilters {
        missing: true,
        unknown_platform: true,
        ..LibraryRowFilters::default()
    };

    assert!(filters.matches(&row_with_origin(RowOrigin::CachedMissing, true)));
    assert!(!filters.matches(&row_with_origin(RowOrigin::CachedMissing, false)));
    assert!(!filters.matches(&row_with_origin(RowOrigin::Live, true)));
}

#[test]
fn show_unknown_only_combines_with_text_search() {
    // Mirrors exactly how `show_loaded_data` composes the two:
    // `matching_row_indices` (text search) intersected with
    // `library_filters.matches` (checkbox filters).
    let known_record = record_at(
        PathBuf::from("/roms/mystery-known.zip"),
        MountState::Pending,
    );
    let mut known_row = row_for(&known_record);
    known_row.archive_path = "/roms/mystery-known.zip".to_string();
    known_row.unknown_platform = false;
    known_row.platform = "GameCube".to_string();
    known_row.search_text = "mystery-known.zip\n\ngamecube\npending".to_string();
    let unknown_record = record_at(
        PathBuf::from("/roms/mystery-unknown.zip"),
        MountState::Pending,
    );
    let unknown_row = row_for(&unknown_record);
    let rows = vec![known_row, unknown_row];

    let text_matches = matching_row_indices(&rows, "mystery").unwrap();
    assert_eq!(
        text_matches.len(),
        2,
        "sanity check: both rows match the text search"
    );

    let filters = LibraryRowFilters {
        unknown_platform: true,
        ..LibraryRowFilters::default()
    };
    let combined: Vec<usize> = text_matches
        .into_iter()
        .filter(|&index| filters.matches(&rows[index]))
        .collect();

    assert_eq!(combined.len(), 1);
    assert_eq!(rows[combined[0]].archive_path, "/roms/mystery-unknown.zip");
}

// -------------------------------------------------------------
// Manual platform assignment.
// -------------------------------------------------------------

#[test]
fn resolved_platform_choice_uses_canonical_selection_or_trimmed_custom_text() {
    assert_eq!(
        resolved_platform_choice(Some("GameCube"), ""),
        Some("GameCube")
    );
    assert_eq!(resolved_platform_choice(None, "anything"), None);
    assert_eq!(
        resolved_platform_choice(Some(CUSTOM_PLATFORM_CHOICE), "  NeoGeo64  "),
        Some("NeoGeo64")
    );
    assert_eq!(
        resolved_platform_choice(Some(CUSTOM_PLATFORM_CHOICE), "   "),
        None,
        "blank custom text must not resolve to an empty platform"
    );
}

#[test]
fn selected_persisted_archive_finds_a_cache_only_missing_row() {
    let path = PathBuf::from("/roms/mystery.zip");
    let snapshot = cached_snapshot(vec![persisted_archive(path.clone(), true)]);

    assert_eq!(
        selected_persisted_archive(Some(&snapshot), Some(&path)),
        Some(&snapshot.archives[0]),
        "a cache-only/missing row must still be classifiable - it is metadata only, not a mount action"
    );
    assert_eq!(selected_persisted_archive(Some(&snapshot), None), None);
    assert_eq!(
        selected_persisted_archive(Some(&snapshot), Some(Path::new("/roms/other.zip"))),
        None
    );
    assert_eq!(selected_persisted_archive(None, Some(&path)), None);
}

#[test]
fn platform_action_available_requires_no_running_action_or_database_load() {
    let mut app = app_for_operation_tests();
    assert!(app.platform_action_available());

    let (_sender, receiver) = mpsc::channel();
    app.platform_action = Some(RunningPlatformAction {
        archive_path: PathBuf::from("/roms/game.zip"),
        receiver,
    });
    assert!(!app.platform_action_available());
    app.platform_action = None;

    let (_sender, receiver) = mpsc::channel();
    app.database_state = DatabaseState::Loading {
        generation: DatabaseGeneration::INITIAL,
        receiver,
        worker: None,
        previous: None,
        scanning: false,
    };
    assert!(!app.platform_action_available());
}

#[test]
fn poll_platform_action_success_refreshes_the_database_cache_asynchronously() {
    let mut app = app_for_operation_tests();
    let archive_path = PathBuf::from("/roms/n64/Luigis_Mansion.zip");
    let (sender, receiver) = mpsc::channel();
    app.platform_action = Some(RunningPlatformAction {
        archive_path: archive_path.clone(),
        receiver,
    });
    sender
        .send(Ok(PlatformAssignmentChange {
            old_platform: Some("N64".to_string()),
            old_source: Some("folder_alias".to_string()),
            new_platform: Some("GameCube".to_string()),
            new_source: Some(MANUAL_PLATFORM_SOURCE.to_string()),
        }))
        .unwrap();

    app.poll_platform_action(&egui::Context::default());

    assert!(app.platform_action.is_none());
    let feedback = app.feedback.as_ref().unwrap();
    assert!(feedback.succeeded);
    assert!(feedback.message.contains("N64 (folder_alias)"));
    assert!(feedback.message.contains("GameCube (manual)"));
    assert!(
        app.history
            .entries()
            .any(
                |entry| entry.archive_path.as_deref() == Some(archive_path.as_path())
                    && entry.outcome == ActivityOutcome::Completed
            ),
    );
    // Refreshing the cache is asynchronous - poll_platform_action only
    // starts a new background database load, it does not block
    // waiting for it, and the live snapshot is untouched.
    assert!(app.database_state.is_loading());
    assert!(matches!(app.state, LoadState::Ready(_)));
}

#[test]
fn poll_platform_action_failure_preserves_the_cached_row_and_shows_the_error() {
    let mut app = app_for_operation_tests();
    let stale_snapshot = cached_snapshot(vec![persisted_archive_with_platform(
        PathBuf::from("/roms/mystery.zip"),
        1,
        "N64",
        "folder_alias",
    )]);
    app.database_state = DatabaseState::Ready {
        snapshot: Box::new(stale_snapshot.clone()),
        last_scan_summary: None,
    };
    let archive_path = PathBuf::from("/roms/mystery.zip");
    let (sender, receiver) = mpsc::channel();
    app.platform_action = Some(RunningPlatformAction {
        archive_path: archive_path.clone(),
        receiver,
    });
    sender
        .send(Err(
            "mystery.zip is not yet in the library database".to_string()
        ))
        .unwrap();

    app.poll_platform_action(&egui::Context::default());

    assert!(app.platform_action.is_none());
    let feedback = app.feedback.as_ref().unwrap();
    assert!(!feedback.succeeded);
    assert!(feedback.message.contains("not yet in the library database"));
    assert!(
        app.history
            .entries()
            .any(
                |entry| entry.archive_path.as_deref() == Some(archive_path.as_path())
                    && entry.outcome == ActivityOutcome::Failed
            ),
    );
    // A failure must never trigger a database reload - the existing
    // cached row is left exactly as it was.
    match &app.database_state {
        DatabaseState::Ready { snapshot, .. } => {
            assert_eq!(snapshot.archives, stale_snapshot.archives);
            assert_eq!(snapshot.database_path, stale_snapshot.database_path);
        }
        other => panic!(
            "expected the stale Ready snapshot to survive untouched, got status {}",
            other.status_label()
        ),
    }
}

#[test]
fn database_reload_removes_a_newly_known_row_from_the_unknown_only_filtered_list() {
    // The second half of "assigning a platform removes the row after
    // the asynchronous database refresh" - the first half
    // (poll_platform_action starting the reload) is covered by
    // poll_platform_action_success_refreshes_the_database_cache_asynchronously;
    // this covers what happens once that reload actually settles,
    // exactly as poll_database_load's own other tests do (a
    // synthetic channel/message, not a real database path).
    let mut app = app_for_operation_tests();
    app.library_filters.unknown_platform = true;
    let archive_path = PathBuf::from("/roms/mystery.zip");
    app.archive_context.focused = Some(archive_path.clone());
    let generation = app.database_generation;
    let (sender, receiver) = mpsc::channel::<DatabaseMessage>();
    app.database_state = DatabaseState::Loading {
        generation,
        receiver,
        worker: None,
        previous: Some(Box::new(cached_snapshot(vec![persisted_archive(
            archive_path.clone(),
            false,
        )]))),
        scanning: false,
    };
    let after_snapshot = cached_snapshot(vec![persisted_archive_with_platform(
        archive_path.clone(),
        1,
        "GameCube",
        MANUAL_PLATFORM_SOURCE,
    )]);
    sender
        .send((generation, Ok(DatabaseOutcome::Loaded(after_snapshot))))
        .unwrap();

    app.poll_database_load(&egui::Context::default());

    let merged = build_display_rows(&[], &[], app.database_state.snapshot());
    assert_eq!(merged.len(), 1);
    assert!(
        !merged[0].unknown_platform,
        "the row must now reflect the manual assignment"
    );
    assert!(
        !app.library_filters.matches(&merged[0]),
        "it must no longer match Show unknown only"
    );

    // Selection safety (requirement 4): the path-based selection is
    // untouched and still resolves the archive's up-to-date details,
    // even though it is no longer visible in the unknown-only
    // filtered list - the existing, intentional
    // selection-independent-of-filter-visibility behavior (see
    // `RowOrigin`'s doc comment), not a new special case.
    assert_eq!(
        app.archive_context.focused.as_deref(),
        Some(archive_path.as_path())
    );
    assert_eq!(
        selected_persisted_archive(
            app.database_state.snapshot(),
            app.archive_context.focused.as_deref()
        )
        .and_then(|persisted| persisted.platform.as_deref()),
        Some("GameCube")
    );
}

#[test]
fn database_reload_adds_a_newly_unknown_row_when_a_manual_platform_is_cleared() {
    let mut app = app_for_operation_tests();
    app.library_filters.unknown_platform = true;
    let archive_path = PathBuf::from("/roms/mystery.zip");
    let generation = app.database_generation;
    let (sender, receiver) = mpsc::channel::<DatabaseMessage>();
    app.database_state = DatabaseState::Loading {
        generation,
        receiver,
        worker: None,
        previous: Some(Box::new(cached_snapshot(vec![
            persisted_archive_with_platform(
                archive_path.clone(),
                1,
                "GameCube",
                MANUAL_PLATFORM_SOURCE,
            ),
        ]))),
        scanning: false,
    };
    let after_snapshot = cached_snapshot(vec![persisted_archive(archive_path, false)]);
    sender
        .send((generation, Ok(DatabaseOutcome::Loaded(after_snapshot))))
        .unwrap();

    app.poll_database_load(&egui::Context::default());

    let merged = build_display_rows(&[], &[], app.database_state.snapshot());
    assert_eq!(merged.len(), 1);
    assert!(merged[0].unknown_platform);
    assert!(
        app.library_filters.matches(&merged[0]),
        "clearing manual back to unknown must make the row match Show unknown only again"
    );
}

#[test]
fn filtered_rows_index_cache_is_recomputed_not_left_stale_after_a_database_reload() {
    let mut app = app_for_operation_tests();
    app.filter = "mystery".to_string();
    // A deliberately stale/out-of-bounds cached index list, as if
    // left over from a previous, now-invalid merged row shape -
    // poll_database_load must never trust or reuse this without
    // recomputing it fresh against the new merge.
    app.filtered_rows = Some(vec![0, 1, 2, 99]);
    let generation = app.database_generation;
    let (sender, receiver) = mpsc::channel::<DatabaseMessage>();
    app.database_state = DatabaseState::Loading {
        generation,
        receiver,
        worker: None,
        previous: None,
        scanning: false,
    };
    let snapshot = cached_snapshot(vec![persisted_archive(
        PathBuf::from("/roms/mystery.zip"),
        false,
    )]);
    sender
        .send((generation, Ok(DatabaseOutcome::Loaded(snapshot))))
        .unwrap();

    app.poll_database_load(&egui::Context::default());

    let recomputed = app
        .filtered_rows
        .as_ref()
        .expect("filtered_rows must be recomputed, not left stale");
    assert_eq!(
        recomputed,
        &vec![0],
        "must be freshly computed against the new merged row set, not the stale placeholder"
    );
}

#[test]
fn toggling_show_unknown_only_performs_no_database_write_or_scan() {
    let mut app = app_for_operation_tests();
    let generation_before = app.database_generation;
    let refresh_generation_before = app.refresh_generation;

    app.library_filters.unknown_platform = true;
    app.library_filters.unknown_platform = false;

    assert_eq!(
        app.database_generation, generation_before,
        "toggling the filter must never start a database load"
    );
    assert_eq!(
        app.refresh_generation, refresh_generation_before,
        "toggling the filter must never trigger a live rescan either"
    );
    assert!(matches!(
        app.database_state,
        DatabaseState::NotCreated { .. }
    ));
}

#[test]
fn mount_action_availability_is_unaffected_by_library_filters() {
    let mut app = app_for_operation_tests();
    let busy_before = app.is_busy();

    app.library_filters.unknown_platform = true;
    app.library_filters.present = true;
    app.library_filters.missing = true;

    assert_eq!(
        app.is_busy(),
        busy_before,
        "library_filters must never influence mount/unmount action-safety gating"
    );
}

#[test]
fn apply_platform_action_sets_and_clears_a_manual_platform() {
    let dir = database_test_dir("apply-platform-set-clear");
    let source = dir.join("source");
    let mount = dir.join("mount");
    let archive_path = write_archive_file(&source, "n64/Luigis_Mansion.zip", b"contents");
    let config = config_for(&source, &mount);
    let database_path = dir.join("library.sqlite3");
    {
        let mut database = Database::open_or_create(&database_path).unwrap();
        scan_and_persist(&mut database, &config, "test").unwrap();
    }

    let change = apply_platform_action_at(
        &database_path,
        &archive_path,
        &PlatformAction::Set("GameCube".to_string()),
    )
    .unwrap();
    assert_eq!(change.old_platform.as_deref(), Some("N64"));
    assert_eq!(change.new_platform.as_deref(), Some("GameCube"));
    assert_eq!(change.new_source.as_deref(), Some(MANUAL_PLATFORM_SOURCE));

    let clear_change =
        apply_platform_action_at(&database_path, &archive_path, &PlatformAction::Clear).unwrap();
    // Immediate exposure of the automatic result, no rescan involved.
    assert_eq!(clear_change.new_platform.as_deref(), Some("N64"));
    assert_eq!(clear_change.new_source.as_deref(), Some("folder_alias"));

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn apply_platform_action_errors_clearly_when_not_yet_scanned() {
    let dir = database_test_dir("apply-platform-not-scanned");
    let database_path = dir.join("library.sqlite3");
    Database::open_or_create(&database_path).unwrap();

    let error = apply_platform_action_at(
        &database_path,
        Path::new("/roms/never-scanned.zip"),
        &PlatformAction::Set("GameCube".to_string()),
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("not yet in the library database")
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[cfg(unix)]
fn apply_platform_action_assigns_a_non_utf8_archive_path_on_unix() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let dir = database_test_dir("apply-platform-non-utf8");
    let source = dir.join("source");
    let mount = dir.join("mount");
    std::fs::create_dir_all(&source).unwrap();
    let mut invalid_name = b"fo".to_vec();
    invalid_name.push(0x80);
    invalid_name.extend_from_slice(b"o.zip");
    let archive_path = source.join(OsString::from_vec(invalid_name));
    assert!(
        archive_path.to_str().is_none(),
        "test path must actually be invalid UTF-8"
    );
    std::fs::write(&archive_path, b"contents").unwrap();
    let config = config_for(&source, &mount);
    let database_path = dir.join("library.sqlite3");
    {
        let mut database = Database::open_or_create(&database_path).unwrap();
        scan_and_persist(&mut database, &config, "test").unwrap();
    }

    let change = apply_platform_action_at(
        &database_path,
        &archive_path,
        &PlatformAction::Set("GameCube".to_string()),
    )
    .unwrap();

    assert_eq!(change.new_platform.as_deref(), Some("GameCube"));

    std::fs::remove_dir_all(&dir).unwrap();
}

// -------------------------------------------------------------
// Bulk manual platform assignment
// -------------------------------------------------------------

#[test]
fn single_click_replaces_the_whole_selection_with_one_row() {
    let mut selected_archives: HashSet<PathBuf> =
        [PathBuf::from("/roms/a.zip"), PathBuf::from("/roms/b.zip")]
            .into_iter()
            .collect();
    let mut selected_archive = Some(PathBuf::from("/roms/a.zip"));
    let clicked = PathBuf::from("/roms/c.zip");

    apply_row_click(
        &mut selected_archives,
        &mut selected_archive,
        clicked.clone(),
        false,
    );

    assert_eq!(selected_archives, [clicked.clone()].into_iter().collect());
    assert_eq!(selected_archive, Some(clicked));
}

#[test]
fn ctrl_click_toggles_individual_rows_without_touching_others() {
    let path_a = PathBuf::from("/roms/a.zip");
    let path_b = PathBuf::from("/roms/b.zip");
    let mut selected_archives: HashSet<PathBuf> = [path_a.clone()].into_iter().collect();
    let mut selected_archive = Some(path_a.clone());

    // Ctrl-click an unselected row: added, path_a untouched.
    apply_row_click(
        &mut selected_archives,
        &mut selected_archive,
        path_b.clone(),
        true,
    );
    assert_eq!(
        selected_archives,
        [path_a.clone(), path_b.clone()].into_iter().collect()
    );
    assert_eq!(selected_archive, Some(path_b.clone()));

    // Ctrl-click an already-selected row: removed, the other stays.
    apply_row_click(
        &mut selected_archives,
        &mut selected_archive,
        path_a.clone(),
        true,
    );
    assert_eq!(selected_archives, [path_b].into_iter().collect());
    assert_eq!(selected_archive, Some(path_a));
}

#[test]
fn ctrl_click_can_deselect_the_last_remaining_row() {
    let path = PathBuf::from("/roms/a.zip");
    let mut selected_archives: HashSet<PathBuf> = [path.clone()].into_iter().collect();
    let mut selected_archive = Some(path.clone());

    apply_row_click(&mut selected_archives, &mut selected_archive, path, true);

    assert!(selected_archives.is_empty());
}

/// Simulates a real two-frame click gesture on the row `render_row`
/// paints (press in frame 1, release in frame 2 - egui requires a
/// widget to already be known/hovered from a prior frame before the
/// frame that releases on it can register `Response::clicked()`;

#[test]
fn real_egui_click_on_the_row_registers_and_reports_no_modifier() {
    let ctx = egui::Context::default();
    let path = PathBuf::from("/roms/a.zip");

    let (response, ctrl_held) = simulate_row_click(
        &ctx,
        egui::pos2(50.0, 12.0),
        egui::Modifiers::default(),
        |ui| {
            show_data_row(
                ui,
                &test_row_cells(),
                24.0,
                &path,
                false,
                false,
                None,
                &COLUMN_WIDTHS,
            )
        },
    );

    assert!(
        response.clicked(),
        "the real row Response must register the click"
    );
    assert!(!ctrl_held, "no modifier key was simulated as held");
}

#[test]
fn real_egui_ctrl_click_on_the_row_reaches_the_selection_helper() {
    // This is the actual bug report: verify Ctrl reaches the row's
    // click handling through the real egui event path, not just
    // through apply_row_click called directly with a hand-built bool.
    let ctx = egui::Context::default();
    let path = PathBuf::from("/roms/a.zip");

    let (response, ctrl_held) =
        simulate_row_click(&ctx, egui::pos2(50.0, 12.0), egui::Modifiers::CTRL, |ui| {
            show_data_row(
                ui,
                &test_row_cells(),
                24.0,
                &path,
                false,
                false,
                None,
                &COLUMN_WIDTHS,
            )
        });
    assert!(response.clicked(), "the click itself must still register");
    assert!(
        ctrl_held,
        "Ctrl must read as held during the real click's frame"
    );

    let mut selected_archives: HashSet<PathBuf> = HashSet::new();
    let mut selected_archive = None;
    apply_row_click(
        &mut selected_archives,
        &mut selected_archive,
        path.clone(),
        ctrl_held,
    );
    assert_eq!(
        selected_archives,
        [path].into_iter().collect::<HashSet<_>>()
    );
}

#[test]
fn real_ordinary_click_replaces_the_selection() {
    let ctx = egui::Context::default();
    let path_a = PathBuf::from("/roms/a.zip");
    let path_b = PathBuf::from("/roms/b.zip");
    let mut selected_archives: HashSet<PathBuf> = [path_a.clone()].into_iter().collect();
    let mut selected_archive = Some(path_a);

    let (response, ctrl_held) = simulate_row_click(
        &ctx,
        egui::pos2(50.0, 12.0),
        egui::Modifiers::default(),
        |ui| {
            show_data_row(
                ui,
                &test_row_cells(),
                24.0,
                &path_b,
                false,
                false,
                None,
                &COLUMN_WIDTHS,
            )
        },
    );
    assert!(response.clicked());
    assert!(!ctrl_held);

    apply_row_click(
        &mut selected_archives,
        &mut selected_archive,
        path_b.clone(),
        ctrl_held,
    );

    assert_eq!(
        selected_archives,
        [path_b].into_iter().collect::<HashSet<_>>(),
        "an ordinary click through the real event path must replace the selection"
    );
}

#[test]
fn real_ctrl_click_adds_a_second_exact_path() {
    let ctx = egui::Context::default();
    let path_a = PathBuf::from("/roms/a.zip");
    let path_b = PathBuf::from("/roms/b.zip");
    let mut selected_archives: HashSet<PathBuf> = [path_a.clone()].into_iter().collect();
    let mut selected_archive = Some(path_a.clone());

    let (response, ctrl_held) =
        simulate_row_click(&ctx, egui::pos2(50.0, 12.0), egui::Modifiers::CTRL, |ui| {
            show_data_row(
                ui,
                &test_row_cells(),
                24.0,
                &path_b,
                false,
                false,
                None,
                &COLUMN_WIDTHS,
            )
        });
    assert!(response.clicked());
    assert!(ctrl_held);

    apply_row_click(
        &mut selected_archives,
        &mut selected_archive,
        path_b.clone(),
        ctrl_held,
    );

    assert_eq!(
        selected_archives,
        [path_a, path_b].into_iter().collect::<HashSet<_>>(),
        "a real Ctrl-click must add to, not replace, the selection"
    );
}

#[test]
fn real_ctrl_click_removes_an_already_selected_path() {
    let ctx = egui::Context::default();
    let path_a = PathBuf::from("/roms/a.zip");
    let path_b = PathBuf::from("/roms/b.zip");
    let mut selected_archives: HashSet<PathBuf> =
        [path_a.clone(), path_b.clone()].into_iter().collect();
    let mut selected_archive = Some(path_b.clone());

    // is_selected = true, since path_a is already in the set - the
    // row's own highlighted/pressed styling must not prevent the
    // click (or its modifiers) from registering.
    let (response, ctrl_held) =
        simulate_row_click(&ctx, egui::pos2(50.0, 12.0), egui::Modifiers::CTRL, |ui| {
            show_data_row(
                ui,
                &test_row_cells(),
                24.0,
                &path_a,
                true,
                false,
                None,
                &COLUMN_WIDTHS,
            )
        });
    assert!(response.clicked());
    assert!(ctrl_held);

    apply_row_click(
        &mut selected_archives,
        &mut selected_archive,
        path_a,
        ctrl_held,
    );

    assert_eq!(
        selected_archives,
        [path_b].into_iter().collect::<HashSet<_>>(),
        "a real Ctrl-click on an already-selected row must remove it"
    );
}

#[test]
fn clicking_text_inside_the_row_behaves_the_same_as_blank_row_space() {
    let ctx = egui::Context::default();
    let path = PathBuf::from("/roms/a.zip");

    // COLUMN_WIDTHS = [120.0, 120.0, 440.0, 520.0]; a position early
    // in the first column lands squarely on rendered text, while a
    // position just past the first column (in the item-spacing gap
    // the old four-separate-Buttons layout never sensed clicks in)
    // must click exactly as reliably - proving there is now one
    // consistent Sense::click response for the whole row, not one
    // per cell with unsensed gaps between them.
    let (on_text, _) = simulate_row_click(
        &ctx,
        egui::pos2(10.0, 12.0),
        egui::Modifiers::default(),
        |ui| {
            show_data_row(
                ui,
                &test_row_cells(),
                24.0,
                &path,
                false,
                false,
                None,
                &COLUMN_WIDTHS,
            )
        },
    );
    assert!(on_text.clicked(), "a click on rendered text must register");

    let (on_gap, _) = simulate_row_click(
        &ctx,
        egui::pos2(121.0, 12.0),
        egui::Modifiers::default(),
        |ui| {
            show_data_row(
                ui,
                &test_row_cells(),
                24.0,
                &path,
                false,
                false,
                None,
                &COLUMN_WIDTHS,
            )
        },
    );
    assert!(
        on_gap.clicked(),
        "a click in the inter-column gap must register exactly the same as on text"
    );
}

#[cfg(unix)]
#[test]
fn show_data_row_handles_a_long_non_utf8_cell_without_panicking() {
    // Exercises the new hover-tooltip text-measurement path
    // (`ui.fonts(|f| f.layout_no_wrap(...))` inside `show_data_row`)
    // against exactly the kind of value most likely to upset it: very
    // long, and containing bytes that are not valid UTF-8 on a Unix
    // path. `Path::display()` never panics on such bytes (it
    // lossily substitutes replacement characters), and this proves
    // the new measurement code path does not either.
    let mut path = PathBuf::from("/roms");
    let mut long_name: Vec<u8> = b"a-very-long-archive-name-segment-".repeat(20);
    long_name.extend_from_slice(b"\x80\x81\x82.zip");
    path.push(OsString::from_vec(long_name));
    let long_display = path.display().to_string();
    let cells = ["Xbox", "Pending", long_display.as_str(), "/mnt/Xbox/a"];

    let ctx = egui::Context::default();
    // x=300 lands inside the (default-width) Archive path column
    // (COLUMN_WIDTHS = [120, 120, 440, 520]), so the pointer is
    // hovering that exact cell across every frame of the click
    // simulation, guaranteeing the measurement branch actually runs.
    let (response, _) = simulate_row_click(
        &ctx,
        egui::pos2(300.0, 12.0),
        egui::Modifiers::default(),
        |ui| show_data_row(ui, &cells, 24.0, &path, false, false, None, &COLUMN_WIDTHS),
    );
    // Reaching this assertion without panicking is the real proof;
    // the row otherwise behaving like any other is a bonus check.
    assert!(response.clicked());
}

/// The Ctrl+Up/Down "focus doesn't visibly move" bug, reproduced and
/// fixed at the paint level: before this fix, `multi_selected` and
/// `focused` collapsed into one `is_selected` flag that painted the
/// exact same fill either way, so moving focus (via Ctrl+arrow)
/// between two rows that were both already multi-selected painted
/// nothing different at all. Inspects `FullOutput::shapes` - the real
/// painted output, not a re-implementation of the paint logic - to
/// prove multi-selection (a fill) and focus (a stroke) are genuinely
/// distinct paint calls, independently present or absent.
#[test]
fn focused_row_paints_a_distinct_stroke_from_the_multi_selected_fill() {
    let ctx = egui::Context::default();
    let path = PathBuf::from("/roms/a.zip");

    let capture = |multi_selected: bool, focused: bool| -> egui::FullOutput {
        ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_data_row(
                    ui,
                    &test_row_cells(),
                    24.0,
                    &path,
                    multi_selected,
                    focused,
                    None,
                    &COLUMN_WIDTHS,
                );
            });
        })
    };
    let visuals = ctx.style().visuals.clone();

    fn has_stroke(output: &egui::FullOutput, color: egui::Color32) -> bool {
        output.shapes.iter().any(|clipped| {
                matches!(&clipped.shape, egui::Shape::Rect(rect) if rect.stroke.width > 0.0 && rect.stroke.color == color)
            })
    }
    fn has_fill(output: &egui::FullOutput, color: egui::Color32) -> bool {
        output
            .shapes
            .iter()
            .any(|clipped| matches!(&clipped.shape, egui::Shape::Rect(rect) if rect.fill == color))
    }

    let neither = capture(false, false);
    assert!(!has_fill(&neither, visuals.selection.bg_fill));
    assert!(!has_stroke(&neither, visuals.warn_fg_color));

    let multi_selected_only = capture(true, false);
    assert!(has_fill(&multi_selected_only, visuals.selection.bg_fill));
    assert!(
        !has_stroke(&multi_selected_only, visuals.warn_fg_color),
        "a multi-selected but unfocused row must not show the focus ring"
    );

    let focused_only = capture(false, true);
    assert!(has_stroke(&focused_only, visuals.warn_fg_color));
    assert!(
        !has_fill(&focused_only, visuals.selection.bg_fill),
        "a focused but not multi-selected row must not show the multi-select fill"
    );

    let both = capture(true, true);
    assert!(
        has_fill(&both, visuals.selection.bg_fill) && has_stroke(&both, visuals.warn_fg_color),
        "a row that is both focused and multi-selected must show both the fill and the ring - \
             this is the exact case Ctrl+Up/Down moving focus within a multi-selection hits"
    );
}

#[test]
fn bulk_action_bar_renders_only_when_more_than_one_row_is_selected() {
    // Proves the *rendering function itself* stays empty/grows the
    // layout appropriately, not just its extracted visibility
    // predicate (bulk_action_bar_requires_more_than_one_selected_row
    // below already covers that in isolation) - `ui.cursor()`
    // advancing down the panel is a real, observable side effect of
    // `show_bulk_platform_action_bar` actually painting the
    // separator/frame/combo box, not a stand-in for it.
    let ctx = egui::Context::default();
    let mut bulk_platform_choice: Option<String> = None;

    let mut one_selected_extra_height = -1.0;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let before = ui.cursor().top();
            let mut selected: HashSet<PathBuf> =
                [PathBuf::from("/roms/a.zip")].into_iter().collect();
            let _ =
                show_bulk_platform_action_bar(ui, &mut selected, &mut bulk_platform_choice, false);
            one_selected_extra_height = ui.cursor().top() - before;
        });
    });
    assert_eq!(
        one_selected_extra_height, 0.0,
        "one selected row must render nothing"
    );

    let mut two_selected_extra_height = 0.0;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let before = ui.cursor().top();
            let mut selected: HashSet<PathBuf> =
                [PathBuf::from("/roms/a.zip"), PathBuf::from("/roms/b.zip")]
                    .into_iter()
                    .collect();
            let _ =
                show_bulk_platform_action_bar(ui, &mut selected, &mut bulk_platform_choice, false);
            two_selected_extra_height = ui.cursor().top() - before;
        });
    });
    assert!(
        two_selected_extra_height > 0.0,
        "two selected rows must actually render the bulk action bar"
    );
}

#[test]
fn real_show_loaded_data_hides_the_bulk_bar_for_one_selected_row() {
    let ctx = egui::Context::default();
    let data = empty_loaded_data("/mount");

    let mut one_selected = RealLoadedDataHarness::new();
    one_selected.archive_context.selected = [PathBuf::from("/roms/a.zip")].into_iter().collect();
    let one_selected_height = one_selected.render(&ctx, &data, bounded_test_input());

    let mut none_selected = RealLoadedDataHarness::new();
    let none_selected_height = none_selected.render(&ctx, &data, bounded_test_input());

    assert_eq!(
        one_selected_height, none_selected_height,
        "one selected row must render exactly like no selection - no bulk bar"
    );
}

#[test]
fn real_show_loaded_data_shows_the_bulk_bar_for_two_selected_rows() {
    // This is the exact scenario the Nobara bug report described:
    // 3+ rows selected, but the bar never appeared because it was
    // being rendered after a `ScrollArea::auto_shrink([false, false])`
    // that claimed all remaining vertical space. Rendering the real
    // `show_loaded_data` end to end - not a helper condition, not
    // `show_bulk_platform_action_bar` alone - is what proves that
    // regression is actually fixed.
    let ctx = egui::Context::default();
    let data = empty_loaded_data("/mount");

    let mut one_selected = RealLoadedDataHarness::new();
    one_selected.archive_context.selected = [PathBuf::from("/roms/a.zip")].into_iter().collect();
    let one_selected_height = one_selected.render(&ctx, &data, bounded_test_input());

    let mut three_selected = RealLoadedDataHarness::new();
    three_selected.archive_context.selected = [
        PathBuf::from("/roms/a.zip"),
        PathBuf::from("/roms/b.zip"),
        PathBuf::from("/roms/c.zip"),
    ]
    .into_iter()
    .collect();
    let three_selected_height = three_selected.render(&ctx, &data, bounded_test_input());

    assert!(
        three_selected_height > one_selected_height,
        "3 selected archives must render additional content (the bulk action bar) that \
             1 selected archive does not - got {one_selected_height} vs {three_selected_height}"
    );
}

#[test]
fn clear_selection_button_click_empties_the_same_selected_archives_set() {
    let ctx = egui::Context::default();
    let selected_archives = std::cell::RefCell::new(
        [PathBuf::from("/roms/a.zip"), PathBuf::from("/roms/b.zip")]
            .into_iter()
            .collect::<HashSet<PathBuf>>(),
    );
    let bulk_platform_choice = std::cell::RefCell::new(None::<String>);

    // Renders the real `show_bulk_platform_action_bar` - the same
    // production function `show_loaded_data` calls - through a
    // `RefCell` so this closure can implement `Fn` (required by
    // `simulate_row_click`/`run_frame`, which call it repeatedly
    // across the 3-frame click sequence) while still mutating the
    // *same* selection set on every call.
    let render = |ui: &mut egui::Ui| -> egui::Response {
        let mut selected = selected_archives.borrow_mut();
        let mut choice = bulk_platform_choice.borrow_mut();
        ui.scope(|ui| {
            let _ = show_bulk_platform_action_bar(ui, &mut selected, &mut choice, false);
        })
        .response
    };

    // Measurement pass: find the rendered bar's bounding rect using
    // the exact same production function, before attempting to click
    // it - never a hardcoded/guessed pixel position.
    let mut bar_rect = None;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            bar_rect = Some(render(ui).rect);
        });
    });
    let bar_rect = bar_rect.unwrap();
    assert_eq!(
        selected_archives.borrow().len(),
        2,
        "the measurement pass must not itself change the selection"
    );

    // "Clear selection" is the rightmost control in this row (no
    // spinner, since bulk_platform_busy is false here).
    let click_pos = egui::pos2(bar_rect.right() - 15.0, bar_rect.center().y);
    simulate_row_click(&ctx, click_pos, egui::Modifiers::default(), render);

    assert!(
        selected_archives.borrow().is_empty(),
        "clicking Clear selection must empty the exact same HashSet row highlighting reads from"
    );
}

#[test]
fn bulk_action_bar_requires_more_than_one_selected_row() {
    let mut selected: HashSet<PathBuf> = HashSet::new();
    assert!(!bulk_action_bar_visible(&selected));

    selected.insert(PathBuf::from("/roms/a.zip"));
    assert!(!bulk_action_bar_visible(&selected));

    selected.insert(PathBuf::from("/roms/b.zip"));
    assert!(bulk_action_bar_visible(&selected));
}

#[test]
fn selection_status_text_matches_the_hashset_count() {
    assert_eq!(selection_status_text(0), "No archives selected");
    assert_eq!(selection_status_text(1), "1 archive selected");
    assert_eq!(selection_status_text(2), "2 archives selected");
    assert_eq!(selection_status_text(11), "11 archives selected");
}

#[test]
fn library_table_message_distinguishes_empty_library_from_zero_filter_results() {
    assert_eq!(
        library_table_message(true, 0),
        Some(LibraryTableMessage::EmptyLibrary),
        "an empty library must report EmptyLibrary regardless of visible_count"
    );
    assert_eq!(
        library_table_message(false, 0),
        Some(LibraryTableMessage::NoFilterResults),
        "archives exist but none are visible must report NoFilterResults"
    );
    assert_eq!(
        library_table_message(false, 3),
        None,
        "archives exist and some are visible: no message, render the table"
    );
}

#[test]
fn select_all_visible_returns_only_the_visible_paths_not_the_hidden_ones() {
    let merged_rows = vec![
        row_with_fields("/roms/a.zip", "SNES", "Live", "a.zip", "/mnt/a"),
        row_with_fields("/roms/b.zip", "SNES", "Live", "b.zip", "/mnt/b"),
        row_with_fields("/roms/c.zip", "SNES", "Live", "c.zip", "/mnt/c"),
    ];
    // Only rows 0 and 2 pass the current filter - row 1 is hidden.
    let visible_indices = vec![0usize, 2usize];

    let selected = select_all_visible(&merged_rows, &visible_indices);

    assert_eq!(
        selected,
        [PathBuf::from("/roms/a.zip"), PathBuf::from("/roms/c.zip")]
            .into_iter()
            .collect::<HashSet<_>>(),
        "Ctrl+A must select exactly the visible rows, never the filtered-out one"
    );
}

// -----------------------------------------------------------------
// v0.4.2-alpha follow-up: explicit "Select all visible" button.
// -----------------------------------------------------------------

fn row_with_fields_and_origin(path: &str, origin: RowOrigin) -> ArchiveRow {
    let mut row = row_with_fields(path, "SNES", "state", path, path);
    row.origin = origin;
    row
}

#[test]
fn select_all_visible_button_enabled_requires_at_least_one_visible_row() {
    assert!(!select_all_visible_button_enabled(0));
    assert!(select_all_visible_button_enabled(1));
    assert!(select_all_visible_button_enabled(3));
}

#[test]
fn select_all_visible_button_click_selects_all_currently_visible_rows() {
    let ctx = egui::Context::default();
    let merged_rows = vec![
        row_with_fields("/roms/a.zip", "SNES", "Live", "a.zip", "/mnt/a"),
        row_with_fields("/roms/b.zip", "SNES", "Live", "b.zip", "/mnt/b"),
        row_with_fields("/roms/c.zip", "SNES", "Live", "c.zip", "/mnt/c"),
    ];
    let visible_indices = vec![0usize, 1usize, 2usize];
    let selected_archives = std::cell::RefCell::new(HashSet::<PathBuf>::new());

    let render = |ui: &mut egui::Ui| -> egui::Response {
        let mut selected = selected_archives.borrow_mut();
        ui.scope(|ui| {
            show_selection_controls_row(ui, &merged_rows, &visible_indices, &mut selected);
        })
        .response
    };

    // Measurement pass: locate the rendered row's bounding rect via the
    // real production function, before attempting to click it.
    let mut row_rect = None;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            row_rect = Some(render(ui).rect);
        });
    });
    let row_rect = row_rect.unwrap();
    assert!(
        selected_archives.borrow().is_empty(),
        "the measurement pass must not itself change the selection"
    );

    // "Select all visible" is the rightmost control in this row.
    let click_pos = egui::pos2(row_rect.right() - 15.0, row_rect.center().y);
    simulate_row_click(&ctx, click_pos, egui::Modifiers::default(), render);

    assert_eq!(
        *selected_archives.borrow(),
        [
            PathBuf::from("/roms/a.zip"),
            PathBuf::from("/roms/b.zip"),
            PathBuf::from("/roms/c.zip"),
        ]
        .into_iter()
        .collect::<HashSet<_>>(),
        "clicking Select all visible must select every currently visible row"
    );
}

#[test]
fn select_all_visible_button_click_never_selects_hidden_filtered_rows() {
    let ctx = egui::Context::default();
    let merged_rows = vec![
        row_with_fields("/roms/a.zip", "SNES", "Live", "a.zip", "/mnt/a"),
        row_with_fields("/roms/b.zip", "SNES", "Live", "b.zip", "/mnt/b"),
        row_with_fields("/roms/c.zip", "SNES", "Live", "c.zip", "/mnt/c"),
    ];
    // Row 1 (b.zip) is filtered out - only positions 0 and 2 are
    // currently visible.
    let visible_indices = vec![0usize, 2usize];
    // b.zip was selected before the filter hid it (e.g. a leftover
    // selection from before the current search) - the button must
    // drop it, not merely fail to add it.
    let selected_archives = std::cell::RefCell::new(
        [PathBuf::from("/roms/b.zip")]
            .into_iter()
            .collect::<HashSet<PathBuf>>(),
    );

    let render = |ui: &mut egui::Ui| -> egui::Response {
        let mut selected = selected_archives.borrow_mut();
        ui.scope(|ui| {
            show_selection_controls_row(ui, &merged_rows, &visible_indices, &mut selected);
        })
        .response
    };

    let mut row_rect = None;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            row_rect = Some(render(ui).rect);
        });
    });
    let row_rect = row_rect.unwrap();

    let click_pos = egui::pos2(row_rect.right() - 15.0, row_rect.center().y);
    simulate_row_click(&ctx, click_pos, egui::Modifiers::default(), render);

    assert_eq!(
        *selected_archives.borrow(),
        [PathBuf::from("/roms/a.zip"), PathBuf::from("/roms/c.zip")]
            .into_iter()
            .collect::<HashSet<_>>(),
        "hidden b.zip must never end up selected, even though it was selected before \
             the current filter hid it"
    );
}

#[test]
fn select_all_visible_button_click_selects_only_search_filtered_rows() {
    let merged_rows = vec![
        row_with_fields("/roms/alpha.zip", "SNES", "Live", "alpha.zip", "/mnt/a"),
        row_with_fields("/roms/bravo.zip", "GBA", "Live", "bravo.zip", "/mnt/b"),
        row_with_fields("/roms/charlie.zip", "SNES", "Live", "charlie.zip", "/mnt/c"),
    ];
    // Mirrors the exact state right after a real search-filter frame
    // (see `real_ctrl_a_selects_only_the_currently_visible_filtered_rows`):
    // only positions 0 and 2 currently pass the search text, computed
    // the same way `show_loaded_data` derives `visible_indices` from
    // `filtered_rows` when no checkbox filter is active.
    let filtered_rows: Option<Vec<usize>> = Some(vec![0usize, 2usize]);
    let library_filters = LibraryRowFilters::default();
    let base_indices = filtered_rows
        .clone()
        .unwrap_or_else(|| (0..merged_rows.len()).collect());
    let visible_indices: Vec<usize> = if library_filters.is_active() {
        base_indices
            .into_iter()
            .filter(|&index| library_filters.matches(&merged_rows[index]))
            .collect()
    } else {
        base_indices
    };

    let ctx = egui::Context::default();
    let selected_archives = std::cell::RefCell::new(HashSet::<PathBuf>::new());
    let render = |ui: &mut egui::Ui| -> egui::Response {
        let mut selected = selected_archives.borrow_mut();
        ui.scope(|ui| {
            show_selection_controls_row(ui, &merged_rows, &visible_indices, &mut selected);
        })
        .response
    };

    let mut row_rect = None;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            row_rect = Some(render(ui).rect);
        });
    });
    let row_rect = row_rect.unwrap();

    let click_pos = egui::pos2(row_rect.right() - 15.0, row_rect.center().y);
    simulate_row_click(&ctx, click_pos, egui::Modifiers::default(), render);

    assert_eq!(
        *selected_archives.borrow(),
        [
            PathBuf::from("/roms/alpha.zip"),
            PathBuf::from("/roms/charlie.zip"),
        ]
        .into_iter()
        .collect::<HashSet<_>>(),
        "only the archives that survive the current search text must be selected"
    );
}

#[test]
fn select_all_visible_button_click_selects_only_missing_only_filtered_rows() {
    let merged_rows = vec![
        row_with_fields_and_origin("/roms/present.zip", RowOrigin::Live),
        row_with_fields_and_origin("/roms/missing-a.zip", RowOrigin::CachedMissing),
        row_with_fields_and_origin("/roms/missing-b.zip", RowOrigin::CachedMissing),
    ];
    // Mirrors `show_loaded_data`'s own `visible_indices` derivation
    // with the "Missing" checkbox filter active and no search text.
    let library_filters = LibraryRowFilters {
        missing: true,
        ..LibraryRowFilters::default()
    };
    let base_indices: Vec<usize> = (0..merged_rows.len()).collect();
    let visible_indices: Vec<usize> = base_indices
        .into_iter()
        .filter(|&index| library_filters.matches(&merged_rows[index]))
        .collect();

    let ctx = egui::Context::default();
    let selected_archives = std::cell::RefCell::new(HashSet::<PathBuf>::new());
    let render = |ui: &mut egui::Ui| -> egui::Response {
        let mut selected = selected_archives.borrow_mut();
        ui.scope(|ui| {
            show_selection_controls_row(ui, &merged_rows, &visible_indices, &mut selected);
        })
        .response
    };

    let mut row_rect = None;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            row_rect = Some(render(ui).rect);
        });
    });
    let row_rect = row_rect.unwrap();

    let click_pos = egui::pos2(row_rect.right() - 15.0, row_rect.center().y);
    simulate_row_click(&ctx, click_pos, egui::Modifiers::default(), render);

    assert_eq!(
        *selected_archives.borrow(),
        [
            PathBuf::from("/roms/missing-a.zip"),
            PathBuf::from("/roms/missing-b.zip"),
        ]
        .into_iter()
        .collect::<HashSet<_>>(),
        "with 'Show missing only' active, the present row must never be selected"
    );
}

#[test]
fn select_all_visible_button_click_does_nothing_when_zero_rows_are_visible() {
    let ctx = egui::Context::default();
    let merged_rows = vec![row_with_fields(
        "/roms/a.zip",
        "SNES",
        "Live",
        "a.zip",
        "/mnt/a",
    )];
    // The current search/filters hide every row.
    let visible_indices: Vec<usize> = Vec::new();
    let selected_archives = std::cell::RefCell::new(HashSet::<PathBuf>::new());

    let render = |ui: &mut egui::Ui| -> egui::Response {
        let mut selected = selected_archives.borrow_mut();
        ui.scope(|ui| {
            show_selection_controls_row(ui, &merged_rows, &visible_indices, &mut selected);
        })
        .response
    };

    let mut row_rect = None;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            row_rect = Some(render(ui).rect);
        });
    });
    let row_rect = row_rect.unwrap();

    let click_pos = egui::pos2(row_rect.right() - 15.0, row_rect.center().y);
    simulate_row_click(&ctx, click_pos, egui::Modifiers::default(), render);

    assert!(
        selected_archives.borrow().is_empty(),
        "the disabled button must ignore the click when zero rows are visible"
    );
}

#[test]
fn select_all_visible_button_click_is_idempotent_when_already_fully_selected() {
    let ctx = egui::Context::default();
    let merged_rows = vec![
        row_with_fields("/roms/a.zip", "SNES", "Live", "a.zip", "/mnt/a"),
        row_with_fields("/roms/b.zip", "SNES", "Live", "b.zip", "/mnt/b"),
    ];
    let visible_indices = vec![0usize, 1usize];
    let already_selected: HashSet<PathBuf> =
        [PathBuf::from("/roms/a.zip"), PathBuf::from("/roms/b.zip")]
            .into_iter()
            .collect();
    let selected_archives = std::cell::RefCell::new(already_selected.clone());

    let render = |ui: &mut egui::Ui| -> egui::Response {
        let mut selected = selected_archives.borrow_mut();
        ui.scope(|ui| {
            show_selection_controls_row(ui, &merged_rows, &visible_indices, &mut selected);
        })
        .response
    };

    let mut row_rect = None;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            row_rect = Some(render(ui).rect);
        });
    });
    let row_rect = row_rect.unwrap();

    let click_pos = egui::pos2(row_rect.right() - 15.0, row_rect.center().y);
    simulate_row_click(&ctx, click_pos, egui::Modifiers::default(), render);

    assert_eq!(
        *selected_archives.borrow(),
        already_selected,
        "clicking Select all visible while every visible row is already selected must \
             leave the selection unchanged"
    );
}

#[test]
fn real_ctrl_a_keyboard_selection_is_unchanged_by_the_selection_controls_refactor() {
    // Guards against the "Select all visible" button's introduction -
    // and the resulting factoring of the selection-controls row into
    // `show_selection_controls_row` - having disturbed Ctrl+A, which
    // dispatches to the exact same `select_all_visible` helper from
    // `show_loaded_data` directly (see `real_ctrl_a_selects_only_the_currently_visible_filtered_rows`
    // for the original coverage this mirrors).
    let ctx = egui::Context::default();
    let data = loaded_data_with_rows(
        "/mount",
        vec![
            row_with_fields("/roms/a.zip", "SNES", "Live", "alpha.zip", "/mnt/a"),
            row_with_fields("/roms/b.zip", "GBA", "Live", "bravo.zip", "/mnt/b"),
            row_with_fields("/roms/c.zip", "SNES", "Live", "charlie.zip", "/mnt/c"),
        ],
    );
    let mut harness = RealLoadedDataHarness::new();
    harness.filtered_rows = Some(vec![0usize, 2usize]);

    harness.render(
        &ctx,
        &data,
        key_press_input(egui::Key::A, egui::Modifiers::CTRL),
    );

    assert_eq!(
        harness.archive_context.selected,
        [PathBuf::from("/roms/a.zip"), PathBuf::from("/roms/c.zip")]
            .into_iter()
            .collect(),
        "Ctrl+A must still select exactly the visible rows after adding the button"
    );
}

#[test]
fn select_all_visible_button_click_never_touches_duplicate_review_selection() {
    let mut app = app_for_operation_tests();
    // Duplicate Review's own independent selection.
    app.selected_duplicate_group = Some(DuplicateGroupIdentity {
        normalized_title: "sonic_the_hedgehog".to_string(),
        platform: "Genesis".to_string(),
    });
    app.selected_duplicate_archive = Some(PathBuf::from("/backup/Sonic.7z"));

    let merged_rows = vec![
        row_with_fields("/roms/a.zip", "SNES", "Live", "a.zip", "/mnt/a"),
        row_with_fields("/roms/b.zip", "SNES", "Live", "b.zip", "/mnt/b"),
    ];
    let visible_indices = vec![0usize, 1usize];

    let ctx = egui::Context::default();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_selection_controls_row(
                ui,
                &merged_rows,
                &visible_indices,
                &mut app.archive_context.selected,
            );
        });
    });
    // A direct call proves the button's own code path, not just its
    // rendering, is exercised - `show_selection_controls_row` only
    // ever receives `&mut app.archive_context.selected`, never the duplicate
    // fields, so it is structurally unable to touch them.
    app.archive_context.selected = select_all_visible(&merged_rows, &visible_indices);

    assert_eq!(
        app.archive_context.selected,
        [PathBuf::from("/roms/a.zip"), PathBuf::from("/roms/b.zip")]
            .into_iter()
            .collect(),
        "the ordinary-library selection must still update normally"
    );
    assert_eq!(
        app.selected_duplicate_group,
        Some(DuplicateGroupIdentity {
            normalized_title: "sonic_the_hedgehog".to_string(),
            platform: "Genesis".to_string(),
        }),
        "Duplicate Review's selected group must remain untouched"
    );
    assert_eq!(
        app.selected_duplicate_archive,
        Some(PathBuf::from("/backup/Sonic.7z")),
        "Duplicate Review's selected archive must remain untouched"
    );
}

#[test]
fn next_focus_in_visible_order_steps_through_visible_sorted_order() {
    let merged_rows = vec![
        row_with_fields("/roms/a.zip", "Z", "Live", "a.zip", "/mnt/a"),
        row_with_fields("/roms/b.zip", "Y", "Live", "b.zip", "/mnt/b"),
        row_with_fields("/roms/c.zip", "X", "Live", "c.zip", "/mnt/c"),
    ];
    // Sorted by platform ascending would put c (X), b (Y), a (Z) in
    // that screen order - verify arrow navigation follows *this*
    // order, not the merged_rows insertion order.
    let visible_indices = vec![2usize, 1usize, 0usize];

    let first =
        next_focus_in_visible_order(&merged_rows, &visible_indices, None, ArrowDirection::Down);
    assert_eq!(first, Some(PathBuf::from("/roms/c.zip")));

    let second = next_focus_in_visible_order(
        &merged_rows,
        &visible_indices,
        first.as_deref(),
        ArrowDirection::Down,
    );
    assert_eq!(second, Some(PathBuf::from("/roms/b.zip")));

    let third = next_focus_in_visible_order(
        &merged_rows,
        &visible_indices,
        second.as_deref(),
        ArrowDirection::Down,
    );
    assert_eq!(third, Some(PathBuf::from("/roms/a.zip")));

    let back = next_focus_in_visible_order(
        &merged_rows,
        &visible_indices,
        third.as_deref(),
        ArrowDirection::Up,
    );
    assert_eq!(back, Some(PathBuf::from("/roms/b.zip")));
}

#[test]
fn next_focus_in_visible_order_clamps_at_both_ends_without_wrapping() {
    let merged_rows = vec![
        row_with_fields("/roms/a.zip", "SNES", "Live", "a.zip", "/mnt/a"),
        row_with_fields("/roms/b.zip", "SNES", "Live", "b.zip", "/mnt/b"),
    ];
    let visible_indices = vec![0usize, 1usize];

    let at_last = next_focus_in_visible_order(
        &merged_rows,
        &visible_indices,
        Some(Path::new("/roms/b.zip")),
        ArrowDirection::Down,
    );
    assert_eq!(
        at_last,
        Some(PathBuf::from("/roms/b.zip")),
        "Down at the last visible row must stay put, not wrap to the first"
    );

    let at_first = next_focus_in_visible_order(
        &merged_rows,
        &visible_indices,
        Some(Path::new("/roms/a.zip")),
        ArrowDirection::Up,
    );
    assert_eq!(
        at_first,
        Some(PathBuf::from("/roms/a.zip")),
        "Up at the first visible row must stay put, not wrap to the last"
    );
}

#[test]
fn next_focus_in_visible_order_does_not_use_a_stale_index_after_filtering() {
    // Reproduces the exact stale-index hazard requirement 1 calls
    // out: focus is on a row that a *new* filter has just hidden.
    // `next_focus_in_visible_order` must re-derive the focus's
    // position from its exact path every call, never trust a
    // previously-computed row index into the old (pre-filter)
    // `visible_indices`.
    let merged_rows = vec![
        row_with_fields("/roms/a.zip", "SNES", "Live", "a.zip", "/mnt/a"),
        row_with_fields("/roms/b.zip", "SNES", "Live", "b.zip", "/mnt/b"),
        row_with_fields("/roms/c.zip", "SNES", "Live", "c.zip", "/mnt/c"),
    ];
    // Before filtering: focus sits on b.zip at visible position 1.
    let focus = Some(PathBuf::from("/roms/b.zip"));

    // A filter change just ran: b.zip no longer matches, only a.zip
    // and c.zip remain visible (at *new* positions 0 and 1).
    let visible_indices_after_filter = vec![0usize, 2usize];

    let next = next_focus_in_visible_order(
        &merged_rows,
        &visible_indices_after_filter,
        focus.as_deref(),
        ArrowDirection::Down,
    );

    // b.zip can no longer be found in the new visible list, so this
    // must fall back to the first visible row (a.zip) - never panic,
    // never silently keep pointing at the now-hidden b.zip, and never
    // misinterpret a stale numeric index as still meaning position 1
    // (which would incorrectly land on c.zip).
    assert_eq!(next, Some(PathBuf::from("/roms/a.zip")));
}
