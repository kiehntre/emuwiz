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
//! Predominant theme observed in this slice: mount/unmount, missing-file cleanup, and activity/history recording.

use super::*;

#[test]
fn build_display_rows_resolves_owning_source_by_exact_database_id() {
    let mut roms = persisted_archive(PathBuf::from("/roms/a.zip"), false);
    roms.source_folder_id = 1;
    let mut backup = persisted_archive(PathBuf::from("/backup/b.zip"), false);
    backup.source_folder_id = 2;
    let snapshot = CachedLibrarySnapshot {
        source_views: vec![
            source_view_fixture(1, "/roms", true),
            source_view_fixture(2, "/backup", true),
        ],
        ..cached_snapshot(vec![roms, backup])
    };

    let merged = build_display_rows(&[], &[], Some(&snapshot));

    let roms_row = merged
        .iter()
        .find(|row| row.path == Path::new("/roms/a.zip"))
        .unwrap();
    assert_eq!(roms_row.source_path, Some(PathBuf::from("/roms")));
    let backup_row = merged
        .iter()
        .find(|row| row.path == Path::new("/backup/b.zip"))
        .unwrap();
    assert_eq!(backup_row.source_path, Some(PathBuf::from("/backup")));
}

#[test]
fn build_display_rows_reports_unassigned_when_no_configured_source_matches() {
    let mut orphaned = persisted_archive(PathBuf::from("/old-drive/c.zip"), false);
    orphaned.source_folder_id = 99;
    let snapshot = CachedLibrarySnapshot {
        source_views: vec![source_view_fixture(1, "/roms", true)],
        ..cached_snapshot(vec![orphaned])
    };

    let merged = build_display_rows(&[], &[], Some(&snapshot));

    assert_eq!(merged[0].source_path, None);
}

#[test]
fn library_source_filter_restricts_visible_rows_to_the_selected_source() {
    let merged_rows = [
        ArchiveRow {
            source_path: Some(PathBuf::from("/roms")),
            ..row("alpha in roms")
        },
        ArchiveRow {
            source_path: Some(PathBuf::from("/backup")),
            ..row("bravo in backup")
        },
        ArchiveRow {
            source_path: None,
            ..row("charlie unassigned")
        },
    ];
    // Mirrors `show_loaded_data`'s exact Source-filter application:
    // an independent `.filter` stage after `library_filters.matches`,
    // never folded into `LibraryRowFilters` itself.
    let library_source_filter: Option<Option<PathBuf>> = Some(Some(PathBuf::from("/roms")));
    let visible: Vec<usize> = (0..merged_rows.len())
        .filter(|&index| match &library_source_filter {
            None => true,
            Some(wanted) => merged_rows[index].source_path.as_ref() == wanted.as_ref(),
        })
        .collect();
    assert_eq!(visible, vec![0]);

    let unassigned_only: Option<Option<PathBuf>> = Some(None);
    let visible: Vec<usize> = (0..merged_rows.len())
        .filter(|&index| match &unassigned_only {
            None => true,
            Some(wanted) => merged_rows[index].source_path.as_ref() == wanted.as_ref(),
        })
        .collect();
    assert_eq!(visible, vec![2]);

    let all_sources: Option<Option<PathBuf>> = None;
    let visible: Vec<usize> = (0..merged_rows.len())
        .filter(|&index| match &all_sources {
            None => true,
            Some(wanted) => merged_rows[index].source_path.as_ref() == wanted.as_ref(),
        })
        .collect();
    assert_eq!(visible, vec![0, 1, 2]);
}

#[test]
fn duplicate_review_filters_count_groups_entries_and_exact_paths() {
    let report = catalogue_filename_duplicates(&duplicate_catalogue_for_gui());
    let mut filters = DuplicateReviewFilters::initial();

    let all = visible_duplicate_group_indices(&report, &filters, DuplicateSortField::Title, true);
    assert_eq!(all.len(), 2);
    assert_eq!(
        all.iter()
            .map(|index| report.groups[*index].entries.len())
            .sum::<usize>(),
        4
    );
    assert!(report.groups.iter().any(|group| {
        group
            .entries
            .iter()
            .any(|entry| entry.path == Path::new("/backup/Sonic the Hedgehog.7z"))
    }));

    filters.search = "/backup/sonic".to_string();
    let searched =
        visible_duplicate_group_indices(&report, &filters, DuplicateSortField::Title, true);
    assert_eq!(searched.len(), 1);
    assert_eq!(report.groups[searched[0]].platform, "Mega Drive");

    filters.search.clear();
    filters.platform = Some("SNES".to_string());
    assert_eq!(
        visible_duplicate_group_indices(&report, &filters, DuplicateSortField::Title, true).len(),
        1
    );
}

#[test]
fn duplicate_review_include_missing_and_more_than_two_filters_are_truthful() {
    let mut archives = duplicate_catalogue_for_gui();
    let mut third_sonic = persisted_archive_with_platform(
        PathBuf::from("/old/Sonic the Hedgehog.rar"),
        5,
        "Mega Drive",
        "heuristic-path-detector",
    );
    third_sonic.last_verified_missing_at = Some("2026-02-02T00:00:00Z".to_string());
    archives.push(third_sonic);
    let report = catalogue_filename_duplicates(&archives);
    let mut filters = DuplicateReviewFilters::initial();
    filters.more_than_two = true;
    assert_eq!(
        visible_duplicate_group_indices(&report, &filters, DuplicateSortField::Entries, true).len(),
        1
    );

    filters.include_missing = false;
    assert!(
        visible_duplicate_group_indices(&report, &filters, DuplicateSortField::Entries, true)
            .is_empty()
    );
}

#[test]
fn duplicate_review_sorting_is_deterministic_with_stable_tiebreakers() {
    let report = catalogue_filename_duplicates(&duplicate_catalogue_for_gui());
    let filters = DuplicateReviewFilters::initial();
    let first =
        visible_duplicate_group_indices(&report, &filters, DuplicateSortField::Entries, true);
    let second =
        visible_duplicate_group_indices(&report, &filters, DuplicateSortField::Entries, true);
    assert_eq!(first, second);
    assert_eq!(report.groups[first[0]].normalized_title, "another_game");
    let descending =
        visible_duplicate_group_indices(&report, &filters, DuplicateSortField::Title, false);
    assert_eq!(
        report.groups[descending[0]].normalized_title,
        "sonic_the_hedgehog"
    );
}

#[test]
fn duplicate_review_state_is_separate_from_library_state_and_activity() {
    let mut app = app_for_operation_tests();
    app.filter = "ordinary search".to_string();
    app.library_filters.missing = true;
    app.sort_field = Some(SortField::State);
    app.archive_context.focused = Some(PathBuf::from("/roms/library.zip"));
    let history_len = app.history.entries.len();

    app.view = MainView::Duplicates;
    app.duplicate_filters.search = "sonic".to_string();
    app.selected_duplicate_archive = Some(PathBuf::from("/backup/Sonic.7z"));
    app.view = MainView::Library;

    assert_eq!(app.filter, "ordinary search");
    assert!(app.library_filters.missing);
    assert_eq!(app.sort_field, Some(SortField::State));
    assert_eq!(
        app.archive_context.focused,
        Some(PathBuf::from("/roms/library.zip"))
    );
    assert_eq!(app.history.entries.len(), history_len);
}

#[test]
fn duplicate_cache_rebuilds_after_platform_change_and_catalogue_cleanup() {
    let archives = duplicate_catalogue_for_gui();
    let original = cached_snapshot(archives.clone());
    assert_eq!(original.duplicate_report.groups.len(), 2);

    let mut platform_changed = archives.clone();
    platform_changed[1].platform = Some("Master System".to_string());
    let regrouped = cached_snapshot(platform_changed);
    assert_eq!(regrouped.duplicate_report.groups.len(), 1);

    let cleaned = cached_snapshot(archives.into_iter().skip(1).collect());
    assert_eq!(cleaned.duplicate_report.groups.len(), 1);
    assert!(
        cleaned
            .duplicate_report
            .groups
            .iter()
            .all(|group| group.normalized_title != "sonic_the_hedgehog")
    );
}

#[test]
fn duplicate_refresh_prunes_only_vanished_duplicate_selections() {
    let report = catalogue_filename_duplicates(&duplicate_catalogue_for_gui());
    let sonic = report
        .groups
        .iter()
        .find(|group| group.normalized_title == "sonic_the_hedgehog")
        .unwrap();
    let mut selected_group = Some(DuplicateGroupIdentity::from(sonic));
    let mut selected_archive = Some(sonic.entries[0].path.clone());
    let remaining = CatalogueDuplicateReport {
        groups: report
            .groups
            .into_iter()
            .filter(|group| group.normalized_title != "sonic_the_hedgehog")
            .collect(),
        archives_in_groups: 2,
    };

    prune_duplicate_review_selection(&mut selected_group, &mut selected_archive, Some(&remaining));

    assert!(selected_group.is_none());
    assert!(selected_archive.is_none());
}

#[test]
fn duplicate_display_wording_is_review_only_and_metadata_is_explicit() {
    let report = catalogue_filename_duplicates(&duplicate_catalogue_for_gui());
    let group = report
        .groups
        .iter()
        .find(|group| group.platform == "Mega Drive")
        .unwrap();
    assert_eq!(group.reason, "Matching normalized filename and platform");
    assert!(
        group
            .entries
            .iter()
            .map(|entry| format_duplicate_size(entry.size_bytes))
            .any(|size| size == "1.0 KiB (1024 bytes)")
    );
    assert_eq!(format_duplicate_size(None), "Unknown");
    assert_eq!(format_modified_time(None), "Unknown");
    assert_eq!(format_modified_time(Some(0)), "1970-01-01T00:00:00Z");
    assert!(group.entries.iter().any(|entry| entry.present));
    assert!(group.entries.iter().any(|entry| !entry.present));
}

#[test]
fn real_duplicate_review_renders_paths_states_details_and_no_deletion_controls() {
    fn collect_text(shape: &egui::Shape, output: &mut String) {
        match shape {
            egui::Shape::Text(text) => {
                output.push_str(&text.galley.job.text);
                output.push('\n');
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, output);
                }
            }
            _ => {}
        }
    }

    let report = catalogue_filename_duplicates(&duplicate_catalogue_for_gui());
    let group = report
        .groups
        .iter()
        .find(|group| group.platform == "Mega Drive")
        .unwrap();
    let mut filters = DuplicateReviewFilters::initial();
    filters.platform = Some("Mega Drive".to_string());
    let mut sort_field = DuplicateSortField::Title;
    let mut ascending = true;
    let mut selected_group = Some(DuplicateGroupIdentity::from(group));
    let mut selected_archive = Some(group.entries[0].path.clone());
    let mut clipboard = InMemoryClipboard::default();
    let context = egui::Context::default();
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 1400.0),
            )),
            ..Default::default()
        },
        |context| {
            egui::CentralPanel::default().show(context, |ui| {
                let _ = show_duplicate_review_panel(
                    ui,
                    &report,
                    DuplicateReviewViewState {
                        filters: &mut filters,
                        sort_field: &mut sort_field,
                        sort_ascending: &mut ascending,
                        selected_group: &mut selected_group,
                        selected_archive: &mut selected_archive,
                        clipboard: &mut clipboard,
                    },
                );
            });
        },
    );
    let mut painted_text = String::new();
    for clipped in &output.shapes {
        collect_text(&clipped.shape, &mut painted_text);
    }

    for expected in [
        "Duplicates",
        "Likely duplicate group",
        "Filename and platform",
        "Matching normalized filename and platform",
        "/backup/Sonic the Hedgehog.7z",
        "/roms/a/Sonic the Hedgehog.zip",
        "Present",
        "Missing",
        "Mega Drive",
        "Selected duplicate archive",
        "Exact archive path",
    ] {
        assert!(
            painted_text.contains(expected),
            "expected rendered duplicate-review text {expected:?}, got:\n{painted_text}"
        );
    }
    assert!(!painted_text.contains("Remove Missing Entries"));
    assert!(!painted_text.contains("Delete"));
}

fn provenance_line_map(details: &PlatformProvenanceDetails) -> HashMap<&'static str, String> {
    platform_provenance_lines(details).into_iter().collect()
}

#[test]
fn missing_removal_selection_requires_missing_only_and_nonempty_selection() {
    let missing_path = PathBuf::from("/roms/missing.zip");
    let present_path = PathBuf::from("/roms/present.zip");
    let mut missing = persisted_archive(missing_path.clone(), true);
    missing.id = 1;
    let mut present = persisted_archive(present_path.clone(), false);
    present.id = 2;
    let snapshot = cached_snapshot(vec![missing, present]);

    assert!(selected_missing_paths(Some(&snapshot), &HashSet::new()).is_err());
    assert_eq!(
        selected_missing_paths(
            Some(&snapshot),
            &[missing_path.clone()].into_iter().collect()
        )
        .unwrap(),
        vec![missing_path.clone()]
    );
    let mixed = selected_missing_paths(
        Some(&snapshot),
        &[missing_path, present_path].into_iter().collect(),
    )
    .unwrap_err();
    assert!(mixed.contains("currently present"));
    assert!(mixed.contains("nothing was removed"));
}

#[test]
fn missing_review_mode_reuses_filters_without_resetting_platform_filters() {
    let mut filters = LibraryRowFilters {
        present: true,
        awaiting_validation: true,
        known_platform: true,
        ..LibraryRowFilters::default()
    };

    set_missing_review_mode(&mut filters, true);

    assert!(filters.missing);
    assert!(!filters.present);
    assert!(!filters.awaiting_validation);
    assert!(filters.known_platform);
    set_missing_review_mode(&mut filters, false);
    assert!(!filters.missing);
    assert!(filters.known_platform);
}

#[test]
fn missing_removal_confirmation_is_explicit_about_catalogue_only_safety() {
    let wording = missing_removal_confirmation_text(3);

    assert!(wording.contains("Remove 3 missing entries from the EmuWiz catalogue?"));
    assert!(wording.contains("only EmuWiz database records"));
    assert!(wording.contains("will not delete archive files or mounted contents"));
    assert!(wording.contains("return if the archives are found in a later scan"));
    assert_eq!(REMOVE_MISSING_CANCEL_LABEL, "Cancel");
    assert_eq!(REMOVE_MISSING_CONFIRM_LABEL, "Remove Missing Entries");
}

#[test]
fn apply_missing_removal_uses_exact_paths_and_rejects_a_mixed_selection() {
    let root = database_test_dir("remove-missing-exact-paths");
    let source = root.join("source");
    let mount = root.join("mount");
    let database_path = root.join("library.sqlite3");
    let gone = write_archive_file(&source, "gone.zip", b"gone");
    let present = write_archive_file(&source, "present.zip", b"present");
    let config = config_for(&source, &mount);
    let mut database = Database::open_or_create(&database_path).unwrap();
    scan_and_persist(&mut database, &config, "initial").unwrap();
    std::fs::remove_file(&gone).unwrap();
    scan_and_persist(&mut database, &config, "missing").unwrap();
    database.close().unwrap();

    let error =
        apply_missing_removal_at(&database_path, &[gone.clone(), present.clone()]).unwrap_err();

    assert!(error.to_string().contains("currently present"));
    let database = Database::open_or_create(&database_path).unwrap();
    assert_eq!(database.load_archives().unwrap().len(), 2);
    database.close().unwrap();
    assert_eq!(
        apply_missing_removal_at(&database_path, &[gone])
            .unwrap()
            .removed,
        1
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn missing_removal_availability_requires_a_healthy_idle_database() {
    let mut app = app_for_operation_tests();
    assert!(!app.missing_removal_action_available());
    app.database_state = DatabaseState::Ready {
        snapshot: Box::new(cached_snapshot(Vec::new())),
        last_scan_summary: None,
    };
    assert!(app.missing_removal_action_available());
    let (_sender, receiver) = mpsc::channel();
    app.alias_action = Some(RunningAliasAction {
        action: AliasAction::Remove {
            alias: "busy".to_string(),
        },
        receiver,
    });
    assert!(!app.missing_removal_action_available());
}

#[test]
fn successful_missing_removal_records_one_activity_and_refreshes_without_resetting_view() {
    let path = PathBuf::from("/roms/missing.zip");
    let mut app = app_for_operation_tests();
    app.database_state = DatabaseState::Ready {
        snapshot: Box::new(cached_snapshot(vec![persisted_archive(path.clone(), true)])),
        last_scan_summary: None,
    };
    app.archive_context.selected.insert(path.clone());
    app.archive_context.focused = Some(path);
    app.library_filters.missing = true;
    app.sort_field = Some(SortField::ArchivePath);
    app.sort_ascending = false;
    let (sender, receiver) = mpsc::channel();
    sender
        .send(Ok(MissingArchiveRemovalResult {
            requested: 1,
            removed: 1,
            archive_ids: vec![1],
        }))
        .unwrap();
    app.missing_removal = Some(RunningMissingRemoval {
        requested_paths: 1,
        receiver,
    });

    app.poll_missing_removal(&egui::Context::default());

    assert!(matches!(app.database_state, DatabaseState::Loading { .. }));
    let entries: Vec<_> = app.history.entries().collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].action, ActivityAction::CatalogueCleanup);
    assert!(entries[0].message.contains("No archive files were deleted"));
    assert!(app.library_filters.missing);
    assert_eq!(app.sort_field, Some(SortField::ArchivePath));
    assert!(!app.sort_ascending);
}

#[test]
fn failed_missing_removal_preserves_selection_cached_rows_filters_and_sort() {
    let path = PathBuf::from("/roms/missing.zip");
    let mut app = app_for_operation_tests();
    app.database_state = DatabaseState::Ready {
        snapshot: Box::new(cached_snapshot(vec![persisted_archive(path.clone(), true)])),
        last_scan_summary: None,
    };
    app.archive_context.selected.insert(path.clone());
    app.archive_context.focused = Some(path.clone());
    app.library_filters.missing = true;
    app.sort_field = Some(SortField::State);
    let (sender, receiver) = mpsc::channel();
    sender.send(Err("simulated failure".to_string())).unwrap();
    app.missing_removal = Some(RunningMissingRemoval {
        requested_paths: 1,
        receiver,
    });

    app.poll_missing_removal(&egui::Context::default());

    assert!(matches!(app.database_state, DatabaseState::Ready { .. }));
    assert_eq!(
        app.archive_context.selected,
        [path.clone()].into_iter().collect()
    );
    assert_eq!(app.archive_context.focused, Some(path));
    assert!(app.library_filters.missing);
    assert_eq!(app.sort_field, Some(SortField::State));
    assert_eq!(app.database_state.snapshot().unwrap().archives.len(), 1);
}

#[test]
fn vanished_missing_selections_are_pruned_after_cache_refresh() {
    let path = PathBuf::from("/roms/removed.zip");
    let mut app = app_for_operation_tests();
    app.archive_context.selected.insert(path.clone());
    app.archive_context.focused = Some(path);

    app.prune_selection(&[]);

    assert!(app.archive_context.selected.is_empty());
    assert!(app.archive_context.focused.is_none());
}

#[test]
fn manual_platform_provenance_uses_human_wording_and_unknown_fallback() {
    let details = PlatformProvenanceDetails {
        platform: Some("GameCube".to_string()),
        source: Some(MANUAL_PLATFORM_SOURCE.to_string()),
        matched_component: None,
        automatic_fallback: None,
    };

    let lines = provenance_line_map(&details);
    // The canonical display name is shown with the stored identifier in
    // parentheses, so a person reads the real hardware name while still
    // seeing exactly what the library has recorded.
    assert_eq!(lines["Platform"], "Nintendo GameCube (GameCube)");
    assert_eq!(lines["Source"], "Manual assignment");
    assert_eq!(lines["Assignment"], "Manually assigned");
    assert_eq!(
        lines["Confidence"], "Confirmed",
        "a platform a person chose is not a guess"
    );
    assert_eq!(lines["Automatic fallback"], "Unknown");
}

#[test]
fn an_automatically_detected_platform_is_labelled_as_such_with_its_confidence() {
    let details = PlatformProvenanceDetails {
        platform: Some("MegaDrive".to_string()),
        source: Some("folder_alias".to_string()),
        matched_component: Some("genesis".to_string()),
        automatic_fallback: None,
    };
    let lines = provenance_line_map(&details);
    assert_eq!(lines["Platform"], "Sega Mega Drive / Genesis (MegaDrive)");
    assert_eq!(lines["Assignment"], "Automatically detected");
    assert_eq!(
        lines["Confidence"], "Probable",
        "a folder name is good evidence, not proof"
    );
    assert_eq!(lines["Matched folder"], "genesis");
}

#[test]
fn an_unassigned_platform_reads_as_unknown_rather_than_as_a_detection() {
    let details = PlatformProvenanceDetails {
        platform: None,
        source: None,
        matched_component: None,
        automatic_fallback: None,
    };
    let lines = provenance_line_map(&details);
    assert_eq!(lines["Platform"], "Unknown");
    assert_eq!(lines["Confidence"], "Unknown");
    assert_eq!(lines["Assignment"], "Not assigned");
    assert!(
        lines["Reason"].contains("No explicit override"),
        "a person must be told why detection found nothing"
    );
}

#[test]
fn a_header_identified_platform_is_confirmed() {
    let details = PlatformProvenanceDetails {
        platform: Some("Wii".to_string()),
        source: Some("header_identity".to_string()),
        matched_component: None,
        automatic_fallback: None,
    };
    assert_eq!(
        provenance_line_map(&details)["Confidence"],
        "Confirmed",
        "a disc magic word is decisive"
    );
}

#[test]
fn provider_enrichment_provenance_is_clear_and_does_not_overclaim() {
    for (source, label, confidence) in [
        (ROMM_PLATFORM_SOURCE, "Detected from RomM", "High"),
        (VERIFIED_DAT_PLATFORM_SOURCE, "Verified by DAT", "Confirmed"),
        (
            DAT_ROMM_AGREEMENT_SOURCE,
            "Verified by DAT and RomM",
            "Confirmed",
        ),
    ] {
        let details = PlatformProvenanceDetails {
            platform: Some("PSP".to_string()),
            source: Some(source.to_string()),
            matched_component: None,
            automatic_fallback: None,
        };
        let lines = provenance_line_map(&details);
        assert_eq!(lines["Platform"], "Sony PlayStation Portable (PSP)");
        assert_eq!(lines["Source"], label);
        assert_eq!(lines["Confidence"], confidence);
    }
}

#[test]
fn manual_platform_provenance_shows_the_correct_detailed_automatic_fallback() {
    let details = PlatformProvenanceDetails {
        platform: Some("GameCube".to_string()),
        source: Some(MANUAL_PLATFORM_SOURCE.to_string()),
        matched_component: None,
        automatic_fallback: Some(archivefs_core::AutomaticPlatformDetails {
            platform: "Amiga CD32".to_string(),
            source: CUSTOM_FOLDER_ALIAS_SOURCE.to_string(),
            matched_component: Some("am".to_string()),
        }),
    };

    let lines = provenance_line_map(&details);
    assert_eq!(lines["Source"], "Manual assignment");
    assert_eq!(lines["Automatic fallback"], "Amiga CD32");
    assert_eq!(lines["Fallback source"], "Custom folder alias");
    assert_eq!(lines["Fallback matched alias"], "am");
}

#[test]
fn custom_and_built_in_alias_provenance_show_their_matches() {
    let custom = PlatformProvenanceDetails {
        platform: Some("Amiga CD32".to_string()),
        source: Some(CUSTOM_FOLDER_ALIAS_SOURCE.to_string()),
        matched_component: Some("am".to_string()),
        automatic_fallback: None,
    };
    let built_in = PlatformProvenanceDetails {
        platform: Some("Intellivision".to_string()),
        source: Some("folder_alias".to_string()),
        matched_component: Some("intellivision".to_string()),
        automatic_fallback: None,
    };

    let custom_lines = provenance_line_map(&custom);
    assert_eq!(custom_lines["Source"], "Custom folder alias");
    assert_eq!(custom_lines["Matched alias"], "am");
    let built_in_lines = provenance_line_map(&built_in);
    assert_eq!(built_in_lines["Source"], "Built-in folder alias");
    assert_eq!(built_in_lines["Matched folder"], "intellivision");
}

#[test]
fn heuristic_and_unknown_provenance_are_clear_and_never_show_raw_sources() {
    let heuristic = PlatformProvenanceDetails {
        platform: Some("MSX".to_string()),
        source: Some("heuristic-path-detector".to_string()),
        matched_component: None,
        automatic_fallback: None,
    };
    let unknown = PlatformProvenanceDetails {
        platform: None,
        source: None,
        matched_component: None,
        automatic_fallback: None,
    };

    let heuristic_lines = provenance_line_map(&heuristic);
    assert_eq!(heuristic_lines["Source"], "Filename/path heuristic");
    assert!(
        !platform_provenance_lines(&heuristic)
            .iter()
            .any(|(_, value)| value.contains("heuristic-path-detector"))
    );
    let unknown_lines = provenance_line_map(&unknown);
    assert_eq!(unknown_lines["Platform"], "Unknown");
    assert_eq!(unknown_lines["Source"], "Unknown");
}

#[test]
fn scan_completion_and_activity_use_every_truthful_non_overlapping_count() {
    let summary = ScanPersistSummary {
        scan_run_id: 42,
        counts: archivefs_core::ScanRunCounts {
            archives_seen: 1_236,
            archives_added: 3,
            archives_changed: 2,
            archives_restored: 1,
            archives_unchanged: 1_230,
            archives_updated: 3,
            archives_missing: 4,
            skipped_unsupported_extension: 5,
            skipped_ambiguous_platform: 2,
            errors_count: 1,
            source_folders_scanned: 2,
        },
        folder_errors: vec![(PathBuf::from("/offline"), "unavailable".to_string())],
        platform_assignment_warnings: Vec::new(),
        skipped_files: Vec::new(),
    };

    assert_eq!(
        format_scan_completion(&summary),
        "Scan completed\nSeen: 1236\nAdded: 3\nUpdated: 3 (including 1 restored)\nNewly missing: 4\nUnchanged: 1230\nSkipped unsupported: 5\nSkipped ambiguous: 2\nErrors: 1"
    );
    let activity = format_scan_activity(&summary);
    assert_eq!(
        activity,
        "Scan completed: seen 1236, added 3, updated 3 (including 1 restored), newly missing 4, unchanged 1230, skipped 7, errors 1."
    );
    assert_eq!(
        activity.lines().count(),
        1,
        "activity must stay one concise entry"
    );
}

/// A never-before-seen `egui::Window` needs one settling frame before
/// its content actually paints (its `Area` has no remembered
/// position/size yet on the very first frame) - true of any floating
/// window in this codebase, not specific to this one. A real running
/// app repaints continuously, so this is purely a test-harness detail:
/// render once to let the window settle, then render again and return
/// that output for assertions.
fn run_skipped_files_window_twice(
    summary: &ScanPersistSummary,
    open: &mut bool,
    filter: &mut Option<archivefs_core::SkipReason>,
) -> egui::FullOutput {
    let ctx = egui::Context::default();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        show_skipped_files_window(ctx, open, Some(summary), filter);
    });
    ctx.run(egui::RawInput::default(), |ctx| {
        show_skipped_files_window(ctx, open, Some(summary), filter);
    })
}

#[test]
fn skipped_files_window_displays_both_reasons_and_their_paths() {
    let summary = skipped_files_summary(
        vec![
            archivefs_core::SkippedFile {
                path: PathBuf::from("/roms/c128/boxart.png"),
                reason: archivefs_core::SkipReason::UnsupportedExtension,
            },
            archivefs_core::SkippedFile {
                path: PathBuf::from("/roms/megadrive/RESOURCE.GEN"),
                reason: archivefs_core::SkipReason::AmbiguousPlatform,
            },
        ],
        1,
        1,
    );
    let mut open = true;
    let mut filter = None;
    let output = run_skipped_files_window_twice(&summary, &mut open, &mut filter);

    // Reason badges (shown because the filter is "All" - see the next
    // test for the filtered case, where they are deliberately omitted)
    // and the filename shown prominently.
    assert!(rendered_text_contains(&output, "Unsupported extension"));
    assert!(rendered_text_contains(&output, "Ambiguous platform"));
    assert!(rendered_text_contains(&output, "boxart.png"));
    assert!(rendered_text_contains(&output, "RESOURCE.GEN"));
    // The full path is secondary (a hover tooltip via `on_hover_text`,
    // not painted without a hover), so it must not dominate the base
    // rendered row the way it used to.
    assert!(!rendered_text_contains(&output, "/roms/c128/boxart.png"));
    assert!(!rendered_text_contains(
        &output,
        "/roms/megadrive/RESOURCE.GEN"
    ));
}

#[test]
fn skipped_files_window_omits_the_reason_badge_when_already_filtered_to_one_reason() {
    let summary = skipped_files_summary(
        vec![
            archivefs_core::SkippedFile {
                path: PathBuf::from("/roms/c128/boxart.png"),
                reason: archivefs_core::SkipReason::UnsupportedExtension,
            },
            archivefs_core::SkippedFile {
                path: PathBuf::from("/roms/megadrive/RESOURCE.GEN"),
                reason: archivefs_core::SkipReason::AmbiguousPlatform,
            },
        ],
        1,
        1,
    );
    let mut open = true;
    let mut filter = Some(archivefs_core::SkipReason::UnsupportedExtension);
    let output = run_skipped_files_window_twice(&summary, &mut open, &mut filter);

    // The filter chip itself already says "Unsupported extension" -
    // repeating it on every one of the (potentially hundreds of) rows
    // it already applies to would be exactly the "giant text prefix"
    // this pass removes. The other reason's row (filtered out) must
    // not appear at all - note "Ambiguous platform" itself is not
    // asserted absent here, since the filter bar's own selectable
    // label for that reason always renders regardless of which filter
    // is active; what must not render is its *row*.
    assert!(rendered_text_contains(&output, "boxart.png"));
    assert!(!rendered_text_contains(&output, "RESOURCE.GEN"));
}

#[test]
fn skipped_files_window_reports_truncation_honestly() {
    let full: Vec<archivefs_core::SkippedFile> = (0..3)
        .map(|index| archivefs_core::SkippedFile {
            path: PathBuf::from(format!("/roms/junk{index}.xyz")),
            reason: archivefs_core::SkipReason::UnsupportedExtension,
        })
        .collect();
    // The real total (5) exceeds how many detail entries were retained
    // (3) - exactly the shape a capped, real scan produces.
    let summary = skipped_files_summary(full, 5, 0);
    assert!(summary.skipped_files_truncated());

    let mut open = true;
    let mut filter = None;
    let output = run_skipped_files_window_twice(&summary, &mut open, &mut filter);
    assert!(
        rendered_text_contains(&output, "Showing 3 of 5"),
        "a capped list must say so, never present itself as complete"
    );
}

#[test]
fn skipped_files_window_never_claims_truncation_when_the_list_is_complete() {
    let full: Vec<archivefs_core::SkippedFile> = (0..3)
        .map(|index| archivefs_core::SkippedFile {
            path: PathBuf::from(format!("/roms/junk{index}.xyz")),
            reason: archivefs_core::SkipReason::UnsupportedExtension,
        })
        .collect();
    let summary = skipped_files_summary(full, 3, 0);
    assert!(!summary.skipped_files_truncated());

    let mut open = true;
    let mut filter = None;
    let output = run_skipped_files_window_twice(&summary, &mut open, &mut filter);
    assert!(rendered_text_contains(&output, "3 skipped file(s)"));
    assert!(!rendered_text_contains(&output, "Showing"));
}

/// Rendering the drill-down window is purely informational: it must
/// never mutate the summary it was given, and the underlying skipped
/// paths (which never actually exist on disk here, but the principle is
/// the same as a real file) are never touched by opening or scrolling
/// the window - the function's own signature (`&ScanPersistSummary`,
/// never `&mut`) already proves this at the type level; this test
/// proves it in practice by rendering twice and checking nothing about
/// the input changed.
#[test]
fn viewing_skipped_file_details_is_read_only() {
    let summary = skipped_files_summary(
        vec![archivefs_core::SkippedFile {
            path: PathBuf::from("/roms/c128/boxart.png"),
            reason: archivefs_core::SkipReason::UnsupportedExtension,
        }],
        1,
        0,
    );
    let before = summary.clone();
    let mut open = true;
    let mut filter = None;
    let output = run_skipped_files_window_twice(&summary, &mut open, &mut filter);
    assert!(
        rendered_text_contains(&output, "boxart.png"),
        "sanity check: the window must actually have rendered content for this test to \
             prove anything"
    );
    assert_eq!(summary, before, "viewing details must never mutate them");
}

#[test]
fn fixed_row_height_matches_the_larger_rendering_constraint() {
    assert_eq!(fixed_row_height(14.0, 18.0), 18.0);
    assert_eq!(fixed_row_height(22.0, 18.0), 22.0);
}

#[test]
fn table_width_uses_all_shared_columns_and_spacing() {
    let spacing = 8.0;
    let expected = COLUMN_WIDTHS.iter().sum::<f32>() + spacing * 3.0;

    assert_eq!(COLUMN_HEADERS.len(), COLUMN_WIDTHS.len());
    assert_eq!(table_width(spacing, &COLUMN_WIDTHS), expected);
}

#[test]
fn library_column_widths_defaults_match_the_original_fixed_constants() {
    assert_eq!(LibraryColumnWidths::default().as_array(), COLUMN_WIDTHS);
}

#[test]
fn responsive_library_columns_expand_both_paths_and_prioritise_destination() {
    let compact = responsive_library_column_widths(900.0, 10.0);
    let wide = responsive_library_column_widths(1_536.0, 10.0);

    assert!(compact.mount_path > compact.archive_path);
    assert!(wide.mount_path > compact.mount_path);
    assert!(wide.archive_path > compact.archive_path);
    assert!(wide.archive_path >= 240.0);
    assert!(wide.mount_path >= 280.0);
}

#[test]
fn health_metric_columns_wrap_before_cards_become_unreadably_narrow() {
    assert_eq!(responsive_card_columns(120.0, 148.0, 10.0, 11), 1);
    assert_eq!(responsive_card_columns(320.0, 148.0, 10.0, 11), 2);
    assert_eq!(responsive_card_columns(700.0, 148.0, 10.0, 11), 4);
    assert_eq!(responsive_card_columns(1_536.0, 148.0, 10.0, 11), 9);
    assert_eq!(responsive_card_columns(1_536.0, 148.0, 10.0, 0), 0);
}

#[test]
fn library_view_dialog_sizing_is_bounded_and_uses_two_columns_only_when_readable() {
    assert_eq!(
        library_view_dialog_size(egui::vec2(1_536.0, 864.0)),
        egui::vec2(780.0, 720.0)
    );
    assert_eq!(
        library_view_dialog_size(egui::vec2(800.0, 600.0)),
        egui::vec2(776.0, 576.0)
    );
    assert!(!library_view_selections_side_by_side(679.0));
    assert!(library_view_selections_side_by_side(680.0));
}

#[test]
fn library_view_add_blocker_explains_each_missing_requirement() {
    assert_eq!(
        library_view_submit_blocker("", "/views", false),
        Some("Enter a name for this Library View.")
    );
    assert_eq!(
        library_view_submit_blocker("Arcade", "", false),
        Some("Choose a destination folder for this Library View.")
    );
    assert!(library_view_submit_blocker("Arcade", "/views", false).is_none());
    assert!(library_view_submit_blocker("Arcade", "/views", true).is_some());
}

#[test]
fn cell_index_at_maps_pointer_positions_to_the_correct_column() {
    let widths = [120.0, 120.0, 440.0, 520.0];
    let spacing = 8.0;
    let row_left = 0.0;

    assert_eq!(cell_index_at(0.0, row_left, &widths, spacing), Some(0));
    assert_eq!(cell_index_at(119.9, row_left, &widths, spacing), Some(0));
    // Inside the inter-column spacing gap - not part of any cell.
    assert_eq!(cell_index_at(122.0, row_left, &widths, spacing), None);
    assert_eq!(cell_index_at(128.0, row_left, &widths, spacing), Some(1));
    // Past the last column entirely.
    assert_eq!(cell_index_at(10_000.0, row_left, &widths, spacing), None);
}

#[test]
fn hovered_cell_full_text_only_fires_when_the_cell_actually_clips() {
    let widths = [120.0, 120.0, 440.0, 520.0];
    let spacing = 8.0;
    let cells = [
        "Xbox",
        "Pending",
        "/roms/a-very-long-archive-path.zip",
        "/mnt/a",
    ];

    // A deterministic stand-in for real font metrics: any text over 10
    // bytes is "wider than its column" here, well clear of every cell
    // width above, so this exercises the clipped branch without
    // depending on actual glyph rendering.
    let wide_measure = |text: &str| if text.len() > 10 { 10_000.0 } else { 1.0 };

    // Pointer over column 2 (Archive path), which the fixture
    // deliberately made long enough to clip.
    let pointer_over_archive_path = Some(120.0 + spacing + 120.0 + spacing + 50.0);
    assert_eq!(
        hovered_cell_full_text(
            pointer_over_archive_path,
            0.0,
            &cells,
            &widths,
            spacing,
            wide_measure,
        ),
        Some("/roms/a-very-long-archive-path.zip"),
        "the full, untruncated cell text must be returned when it clips"
    );

    // Same position, but every cell reports as fitting - no tooltip.
    assert_eq!(
        hovered_cell_full_text(
            pointer_over_archive_path,
            0.0,
            &cells,
            &widths,
            spacing,
            |_| 1.0,
        ),
        None,
        "a cell that fits must never get a tooltip"
    );
}

#[test]
fn hovered_cell_full_text_returns_none_without_a_hover_position_or_outside_every_cell() {
    let widths = [120.0, 120.0, 440.0, 520.0];
    let spacing = 8.0;
    let cells = ["Xbox", "Pending", "/roms/a.zip", "/mnt/a"];
    let always_clipped = |_: &str| 10_000.0;

    assert_eq!(
        hovered_cell_full_text(None, 0.0, &cells, &widths, spacing, always_clipped),
        None,
        "no current hover position means no tooltip"
    );
    // In the gap between Platform and State.
    assert_eq!(
        hovered_cell_full_text(Some(122.0), 0.0, &cells, &widths, spacing, always_clipped),
        None,
        "the inter-column gap belongs to no cell"
    );
}

/// Simulates a real drag gesture - press, move while held, release -
/// mirroring `simulate_row_click`'s three-frame structure for clicks
/// (see its own doc comment for why hit-testing needs the extra
/// frames). `render` must paint the *same* interactive widget (same
/// `egui::Id`) every call, so egui recognizes the drag as one
/// continuous gesture across frames rather than several unrelated
/// presses.
fn simulate_drag(
    ctx: &egui::Context,
    start: egui::Pos2,
    end: egui::Pos2,
    render: impl Fn(&mut egui::Ui) -> egui::Response,
) -> egui::Response {
    run_frame(
        ctx,
        egui::RawInput {
            events: vec![egui::Event::PointerMoved(start)],
            ..Default::default()
        },
        &render,
    );
    run_frame(
        ctx,
        egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            }],
            ..Default::default()
        },
        &render,
    );
    let (response, _) = run_frame(
        ctx,
        egui::RawInput {
            events: vec![egui::Event::PointerMoved(end)],
            ..Default::default()
        },
        &render,
    );
    run_frame(
        ctx,
        egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos: end,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }],
            ..Default::default()
        },
        &render,
    );
    response
}

/// Shared by every header-resize test below: renders the real header,
/// returns its rect and the actual `item_spacing.x` this `Context`'s
/// default style used - never hard-coded, so these tests keep working
/// even if egui's own default spacing ever changes.
fn render_header_and_measure(
    ctx: &egui::Context,
    column_widths: &std::cell::RefCell<LibraryColumnWidths>,
    clicked_field: &std::cell::RefCell<Option<SortField>>,
) -> (egui::Rect, f32) {
    let spacing = std::cell::RefCell::new(0.0_f32);
    let render = |ui: &mut egui::Ui| -> egui::Response {
        *spacing.borrow_mut() = ui.spacing().item_spacing.x;
        ui.scope(|ui| {
            let mut widths = column_widths.borrow_mut();
            if let Some(field) = show_header_row(
                ui,
                &COLUMN_HEADERS,
                &COLUMN_SORT_FIELDS,
                20.0,
                None,
                true,
                &mut widths,
            ) {
                *clicked_field.borrow_mut() = Some(field);
            }
        })
        .response
    };
    let mut header_rect = None;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            header_rect = Some(render(ui).rect);
        });
    });
    (header_rect.unwrap(), *spacing.borrow())
}

/// The Archive path column's own resize-handle x position, given the
/// header rect/spacing `render_header_and_measure` returns and the
/// widths in effect *before* any drag - matches `show_header_row`'s
/// own boundary math exactly (Platform, then State, then Archive
/// path's own trailing edge).
fn archive_path_handle_x(
    header_rect: egui::Rect,
    spacing: f32,
    widths: &LibraryColumnWidths,
) -> f32 {
    let array = widths.as_array();
    header_rect.left() + array[0] + spacing + array[1] + spacing + array[2]
        - COLUMN_RESIZE_HANDLE_WIDTH / 2.0
}

#[test]
fn dragging_the_archive_path_handle_grows_only_that_column_and_never_sorts() {
    let ctx = egui::Context::default();
    let column_widths = std::cell::RefCell::new(LibraryColumnWidths::default());
    let clicked_field: std::cell::RefCell<Option<SortField>> = std::cell::RefCell::new(None);

    let (header_rect, spacing) = render_header_and_measure(&ctx, &column_widths, &clicked_field);
    let before = *column_widths.borrow();
    let handle_x = archive_path_handle_x(header_rect, spacing, &before);
    let handle_y = header_rect.center().y;

    let render = |ui: &mut egui::Ui| -> egui::Response {
        ui.scope(|ui| {
            let mut widths = column_widths.borrow_mut();
            if let Some(field) = show_header_row(
                ui,
                &COLUMN_HEADERS,
                &COLUMN_SORT_FIELDS,
                20.0,
                None,
                true,
                &mut widths,
            ) {
                *clicked_field.borrow_mut() = Some(field);
            }
        })
        .response
    };
    simulate_drag(
        &ctx,
        egui::pos2(handle_x, handle_y),
        egui::pos2(handle_x + 60.0, handle_y),
        render,
    );

    let after = *column_widths.borrow();
    assert!(
        after.archive_path > before.archive_path,
        "dragging right must grow Archive path (before {}, after {})",
        before.archive_path,
        after.archive_path
    );
    assert_eq!(
        after.mount_path, before.mount_path,
        "resizing Archive path must never change Mount path's own width"
    );
    assert_eq!(
        *clicked_field.borrow(),
        None,
        "a drag on the resize handle must never register as a column-sort click"
    );
}

#[test]
fn resize_handle_clamps_to_the_minimum_width() {
    let ctx = egui::Context::default();
    let column_widths = std::cell::RefCell::new(LibraryColumnWidths::default());
    let clicked_field: std::cell::RefCell<Option<SortField>> = std::cell::RefCell::new(None);

    let (header_rect, spacing) = render_header_and_measure(&ctx, &column_widths, &clicked_field);
    let before = *column_widths.borrow();
    let handle_x = archive_path_handle_x(header_rect, spacing, &before);
    let handle_y = header_rect.center().y;

    let render = |ui: &mut egui::Ui| -> egui::Response {
        ui.scope(|ui| {
            let mut widths = column_widths.borrow_mut();
            let _ = show_header_row(
                ui,
                &COLUMN_HEADERS,
                &COLUMN_SORT_FIELDS,
                20.0,
                None,
                true,
                &mut widths,
            );
        })
        .response
    };
    // A single wild drag far to the left - must clamp, never collapse
    // into a sliver or go negative.
    simulate_drag(
        &ctx,
        egui::pos2(handle_x, handle_y),
        egui::pos2(handle_x - 10_000.0, handle_y),
        render,
    );

    assert_eq!(
        column_widths.borrow().archive_path,
        MIN_RESIZABLE_COLUMN_WIDTH,
        "a column can never collapse below the minimum usable width"
    );
}

#[test]
fn resize_handle_clamps_to_the_maximum_width() {
    let ctx = egui::Context::default();
    let column_widths = std::cell::RefCell::new(LibraryColumnWidths::default());
    let clicked_field: std::cell::RefCell<Option<SortField>> = std::cell::RefCell::new(None);

    let (header_rect, spacing) = render_header_and_measure(&ctx, &column_widths, &clicked_field);
    let before = *column_widths.borrow();
    let handle_x = archive_path_handle_x(header_rect, spacing, &before);
    let handle_y = header_rect.center().y;

    let render = |ui: &mut egui::Ui| -> egui::Response {
        ui.scope(|ui| {
            let mut widths = column_widths.borrow_mut();
            let _ = show_header_row(
                ui,
                &COLUMN_HEADERS,
                &COLUMN_SORT_FIELDS,
                20.0,
                None,
                true,
                &mut widths,
            );
        })
        .response
    };
    simulate_drag(
        &ctx,
        egui::pos2(handle_x, handle_y),
        egui::pos2(handle_x + 100_000.0, handle_y),
        render,
    );

    assert_eq!(
        column_widths.borrow().archive_path,
        MAX_RESIZABLE_COLUMN_WIDTH,
        "a column can never grow into an unbounded runaway size"
    );
}

#[test]
fn empty_filter_uses_all_rows_without_an_index_allocation() {
    let rows = vec![row("Halo Xbox Mounted")];

    assert_eq!(matching_row_indices(&rows, "  "), None);
}

#[test]
fn filter_indices_match_each_displayed_field_case_insensitively() {
    let rows = vec![
        row("/roms/Halo.zip /mnt/archivefs/Xbox/Halo Xbox Mounted"),
        row("/roms/Ridge.7z /mnt/archivefs/PSP/Ridge PSP Pending"),
    ];

    for query in ["HALO", "archivefs/xbox", "xBoX", "mounted"] {
        assert_eq!(matching_row_indices(&rows, query), Some(vec![0]));
    }
    assert_eq!(matching_row_indices(&rows, "playstation"), Some(Vec::new()));
}

#[test]
fn ordinary_mount_all_render_state_uses_only_pending_count() {
    assert!(mount_all_available(500_000, false));
    assert!(!mount_all_available(0, false));
    assert!(!mount_all_available(500_000, true));
}

#[test]
fn mount_all_selects_only_pending_archives() {
    let records = vec![
        record("/roms/Pending.zip", MountState::Pending),
        record("/roms/Mounted.zip", MountState::Mounted),
        record("/roms/Existing.zip", MountState::MountPathExists),
    ];

    let selected = pending_mount_items(&records);

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].archive_path, PathBuf::from("/roms/Pending.zip"));
}

#[test]
fn mount_all_processes_sequentially_and_continues_after_failure() {
    let items = vec![
        mount_all_item("First", "First"),
        mount_all_item("Second", "Second"),
        mount_all_item("Third", "Third"),
    ];
    let order = std::cell::RefCell::new(Vec::new());
    let events = std::cell::RefCell::new(Vec::new());
    let stop = AtomicBool::new(false);

    let result = run_mount_all_coordinator(
        items,
        &stop,
        |_| true,
        |_| Ok(()),
        |archive_path| {
            order.borrow_mut().push(archive_path.to_path_buf());
            if archive_path.ends_with("Second.zip") {
                Err("second failed".to_string())
            } else {
                Ok(BatchMountAttempt::Mounted(PathBuf::from("/mount/actual")))
            }
        },
        |event| events.borrow_mut().push(event),
    );

    assert_eq!(
        order.into_inner(),
        vec![
            PathBuf::from("/roms/First.zip"),
            PathBuf::from("/roms/Second.zip"),
            PathBuf::from("/roms/Third.zip"),
        ]
    );
    assert_eq!(result.attempted(), 3);
    assert_eq!(result.successful, 2);
    assert_eq!(result.failed(), 1);
    assert_eq!(result.skipped(), 0);
    assert_eq!(result.unattempted, 0);
    let events = events.into_inner();
    assert!(matches!(
        events[0],
        MountAllEvent::ArchiveStarted { index: 1, .. }
    ));
    assert!(events.iter().any(|event| matches!(
        event,
        MountAllEvent::ArchiveFailed { item, .. }
            if item.archive_path == Path::new("/roms/Second.zip")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        MountAllEvent::ArchiveCompleted(item)
            if item.archive_path == Path::new("/roms/Third.zip")
    )));
}

#[test]
fn mount_all_counts_already_mounted_missing_and_duplicate_targets_as_skipped() {
    let items = vec![
        mount_all_item("Mounted", "Mounted"),
        mount_all_item("Missing", "Missing"),
        mount_all_item("FirstDuplicate", "Shared"),
        mount_all_item("SecondDuplicate", "Shared"),
    ];
    let stop = AtomicBool::new(false);
    let mount_calls = std::cell::RefCell::new(Vec::new());

    let result = run_mount_all_coordinator(
        items,
        &stop,
        |archive_path| !archive_path.ends_with("Missing.zip"),
        |archive_path| {
            if archive_path.ends_with("SecondDuplicate.zip") {
                Err("duplicate target after resolution".to_string())
            } else {
                Ok(())
            }
        },
        |archive_path| {
            mount_calls.borrow_mut().push(archive_path.to_path_buf());
            if archive_path.ends_with("Mounted.zip") {
                Ok(BatchMountAttempt::AlreadyMounted(PathBuf::from(
                    "/mount/already",
                )))
            } else {
                Ok(BatchMountAttempt::Mounted(PathBuf::from("/mount/actual")))
            }
        },
        |_| {},
    );

    assert_eq!(result.total, 4);
    assert_eq!(result.attempted(), 1);
    assert_eq!(result.successful, 1);
    assert_eq!(result.failed(), 0);
    assert_eq!(result.skipped(), 3);
    assert!(
        !mount_calls
            .borrow()
            .iter()
            .any(|path| path.ends_with("SecondDuplicate.zip"))
    );
    assert!(
        result
            .skipped
            .iter()
            .any(|entry| entry.reason.contains("already mounted"))
    );
    assert!(
        result
            .skipped
            .iter()
            .any(|entry| entry.reason.contains("disappeared"))
    );
    assert!(
        result
            .skipped
            .iter()
            .any(|entry| entry.reason.contains("duplicate target"))
    );
}

#[test]
fn mount_all_stop_after_current_prevents_later_mounts() {
    let items = vec![
        mount_all_item("First", "First"),
        mount_all_item("Second", "Second"),
        mount_all_item("Third", "Third"),
    ];
    let stop = AtomicBool::new(false);
    let attempted = std::cell::Cell::new(0);

    let result = run_mount_all_coordinator(
        items,
        &stop,
        |_| true,
        |_| Ok(()),
        |_| {
            attempted.set(attempted.get() + 1);
            stop.store(true, Ordering::Release);
            Ok(BatchMountAttempt::Mounted(PathBuf::from("/mount/actual")))
        },
        |_| {},
    );

    assert_eq!(attempted.get(), 1);
    assert_eq!(result.successful, 1);
    assert_eq!(result.unattempted, 2);
    assert!(result.stopped);
}

#[test]
fn mount_all_setup_failure_is_terminal_and_truthful() {
    let result = MountAllResult::setup_failed(12, "mount root is unavailable");

    assert_eq!(result.completion_message(), "Mount All could not start.");
    assert_ne!(
        result.completion_message(),
        "Mount All completed successfully."
    );
    assert_eq!(result.attempted(), 0);
    assert_eq!(result.successful, 0);
    assert_eq!(result.failed(), 0);
    assert_eq!(result.skipped(), 0);
    assert!(result.skipped.is_empty());
    assert_eq!(result.unattempted, 12);
    assert_eq!(
        result.setup_failure.as_deref(),
        Some("mount root is unavailable")
    );
}

#[test]
fn mount_all_setup_failure_records_failed_activity_and_feedback() {
    let mut app = app_for_operation_tests();
    let (sender, receiver) = mpsc::channel();
    app.mount_all = Some(RunningMountAll {
        receiver,
        stop: Arc::new(AtomicBool::new(false)),
        progress: MountAllProgress {
            total: 4,
            ..MountAllProgress::default()
        },
    });
    sender
        .send(MountAllEvent::Finished(MountAllResult::setup_failed(
            4,
            "configuration could not be loaded",
        )))
        .unwrap();

    app.poll_mount_all(&egui::Context::default());

    let feedback = app.feedback.as_ref().unwrap();
    assert!(!feedback.succeeded);
    assert!(feedback.message.contains("Mount All could not start"));
    assert!(
        feedback
            .message
            .contains("configuration could not be loaded")
    );
    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::MountAll
            && entry.outcome == ActivityOutcome::Failed
            && entry.message.contains("configuration could not be loaded")
    }));
    let result = app.mount_all_result.as_ref().unwrap();
    assert_eq!(result.unattempted, 4);
    assert!(result.skipped.is_empty());
}

#[test]
fn partial_mount_all_failure_is_not_a_total_failure() {
    let result = MountAllResult {
        total: 102,
        successful: 100,
        failures: vec![
            MountAllFailure {
                archive_path: PathBuf::from("/roms/One.zip"),
                message: "failed".to_string(),
            },
            MountAllFailure {
                archive_path: PathBuf::from("/roms/Two.zip"),
                message: "failed".to_string(),
            },
        ],
        skipped: Vec::new(),
        unattempted: 0,
        stopped: false,
        setup_failure: None,
    };

    assert_eq!(
        result.completion_message(),
        "Mount All completed with 2 failures."
    );
    assert_eq!(result.attempted(), 102);
}

#[test]
fn action_availability_follows_mount_state() {
    assert_eq!(available_action(MountState::Pending), ArchiveAction::Mount);
    assert_eq!(
        available_action(MountState::MountPathExists),
        ArchiveAction::Mount
    );
    assert_eq!(
        available_action(MountState::Mounted),
        ArchiveAction::Unmount
    );
}

#[test]
fn selected_record_lookup_uses_the_exact_archive_path() {
    let records = vec![
        record("/roms/Alpha.zip", MountState::Pending),
        record("/roms/Beta.7z", MountState::Mounted),
    ];

    assert_eq!(
        selected_record_index(&records, Some(Path::new("/roms/Beta.7z"))),
        Some(1)
    );
    assert_eq!(
        selected_record(&records, Some(Path::new("/roms/Beta.7z")))
            .unwrap()
            .mount_state,
        MountState::Mounted
    );
    assert!(selected_record(&records, Some(Path::new("/roms/Missing.rar"))).is_none());
    assert!(selected_record(&records, None).is_none());
}

#[test]
fn mount_all_is_rejected_while_an_individual_operation_is_active() {
    let mut app = app_for_operation_tests();
    let (_sender, receiver) = mpsc::channel();
    app.operation = Some(RunningOperation {
        action: ArchiveAction::Mount,
        archive_path: PathBuf::from("/roms/Active.zip"),
        receiver,
        progress_receiver: mpsc::channel().1,
    });

    assert!(!app.start_mount_all(
        egui::Context::default(),
        vec![mount_all_item("Pending", "Pending")],
    ));
    assert!(app.mount_all.is_none());
    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::MountAll && entry.outcome == ActivityOutcome::Rejected
    }));
}

#[test]
fn individual_actions_are_unavailable_during_mount_all() {
    let mut app = app_for_operation_tests();
    let (_sender, receiver) = mpsc::channel();
    app.mount_all = Some(RunningMountAll {
        receiver,
        stop: Arc::new(AtomicBool::new(false)),
        progress: MountAllProgress {
            total: 1,
            ..MountAllProgress::default()
        },
    });
    let mounted = record("/roms/Game.zip", MountState::Mounted);
    let pending = record("/roms/Pending.zip", MountState::Pending);

    assert!(app.is_busy());
    assert!(!individual_actions_available(app.is_busy()));
    assert!(!lazy_unmount_available(
        &mounted,
        &HashSet::from([PathBuf::from("/roms/Game.zip")]),
        app.is_busy(),
    ));
    assert!(!remount_available(
        &pending,
        &HashSet::from([PathBuf::from("/roms/Pending.zip")]),
        app.is_busy(),
    ));
}

#[test]
fn mount_all_stop_request_is_recorded_and_signalled() {
    let mut app = app_for_operation_tests();
    let (_sender, receiver) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    app.mount_all = Some(RunningMountAll {
        receiver,
        stop: Arc::clone(&stop),
        progress: MountAllProgress {
            total: 3,
            ..MountAllProgress::default()
        },
    });

    app.request_mount_all_stop();

    assert!(stop.load(Ordering::Acquire));
    assert!(app.mount_all.as_ref().unwrap().progress.stop_requested);
    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::MountAll
            && entry.outcome == ActivityOutcome::Cancelled
            && entry.message.contains("current archive")
    }));
}

#[test]
fn mount_all_activity_records_batch_and_archive_outcomes() {
    let mut app = app_for_operation_tests();
    let (sender, receiver) = mpsc::channel();
    let first = mount_all_item("First", "First");
    let second = mount_all_item("Second", "Second");
    let third = mount_all_item("Third", "Third");
    app.history.record(HistoryEntry::new(
        ActivityAction::MountAll,
        None,
        ActivityOutcome::Started,
        "Mount All started.",
    ));
    app.mount_all = Some(RunningMountAll {
        receiver,
        stop: Arc::new(AtomicBool::new(false)),
        progress: MountAllProgress {
            total: 3,
            ..MountAllProgress::default()
        },
    });
    sender
        .send(MountAllEvent::ArchiveStarted {
            index: 1,
            total: 3,
            item: first.clone(),
        })
        .unwrap();
    sender.send(MountAllEvent::ArchiveCompleted(first)).unwrap();
    sender
        .send(MountAllEvent::ArchiveFailed {
            item: second,
            message: "mount failed".to_string(),
        })
        .unwrap();
    sender
        .send(MountAllEvent::ArchiveSkipped {
            item: third,
            reason: "archive disappeared".to_string(),
        })
        .unwrap();
    sender
        .send(MountAllEvent::Finished(MountAllResult {
            total: 3,
            successful: 1,
            failures: vec![MountAllFailure {
                archive_path: PathBuf::from("/roms/Second.zip"),
                message: "mount failed".to_string(),
            }],
            skipped: vec![MountAllSkipped {
                archive_path: PathBuf::from("/roms/Third.zip"),
                reason: "archive disappeared".to_string(),
            }],
            unattempted: 0,
            stopped: false,
            setup_failure: None,
        }))
        .unwrap();

    app.poll_mount_all(&egui::Context::default());

    assert!(app.mount_all.is_none());
    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::MountAll && entry.outcome == ActivityOutcome::Completed
    }));
    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::Mount && entry.outcome == ActivityOutcome::Completed
    }));
    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::Mount && entry.outcome == ActivityOutcome::Failed
    }));
    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::Mount && entry.outcome == ActivityOutcome::Skipped
    }));
    assert_eq!(
        app.feedback.as_ref().unwrap().message,
        "Mount All completed with 1 failure."
    );
}

#[test]
fn start_operation_rejects_a_second_operation_without_replacing_the_receiver() {
    let mut app = app_for_operation_tests();
    let (sender, receiver) = mpsc::channel();
    app.operation = Some(RunningOperation {
        action: ArchiveAction::Mount,
        archive_path: PathBuf::from("/roms/Alpha.zip"),
        receiver,
        progress_receiver: mpsc::channel().1,
    });

    assert!(!app.start_operation(
        egui::Context::default(),
        ArchiveAction::Unmount,
        PathBuf::from("/roms/Beta.7z"),
        true,
    ));
    assert_eq!(app.operation.as_ref().unwrap().action, ArchiveAction::Mount);

    sender
        .send(Ok(OperationSuccess {
            message: "original result".to_string(),
            cleanup: None,
            warning: None,
        }))
        .unwrap();
    let result = app
        .operation
        .as_ref()
        .unwrap()
        .receiver
        .try_recv()
        .unwrap()
        .unwrap();
    assert_eq!(result.message, "original result");
    assert!(result.cleanup.is_none());
    let feedback = app.feedback.as_ref().unwrap();
    assert!(!feedback.succeeded);
    assert!(feedback.message.contains("already running"));
    let rejected = app.history.entries().next().unwrap();
    assert_eq!(rejected.outcome, ActivityOutcome::Rejected);
    assert_eq!(rejected.action, ActivityAction::Unmount);
    assert_eq!(
        rejected.archive_path.as_deref(),
        Some(Path::new("/roms/Beta.7z"))
    );
    assert!(rejected.message.contains("already running"));
}

#[test]
fn starting_an_operation_clears_pending_unmount_confirmation() {
    let mut app = app_for_operation_tests();
    app.confirm_unmount = Some(PathBuf::from("/roms/Alpha.zip"));

    assert!(app.start_operation_with_worker(
        egui::Context::default(),
        ArchiveAction::Mount,
        PathBuf::from("/roms/Beta.7z"),
        false,
        |_, _, _, _| {
            Ok(OperationSuccess {
                message: "mounted".to_string(),
                cleanup: None,
                warning: None,
            })
        },
    ));
    assert!(app.confirm_unmount.is_none());
    assert!(app.operation.is_some());
}

#[test]
fn unmount_confirmation_actions_are_unavailable_while_busy() {
    assert!(confirmation_actions_available(false));
    assert!(!confirmation_actions_available(true));
}

#[test]
fn history_keeps_newest_entries_first() {
    let mut history = OperationHistory::default();
    history.record(history_entry(ActivityOutcome::Started, "first"));
    history.record(history_entry(ActivityOutcome::Completed, "second"));

    let messages = history
        .entries()
        .map(|entry| entry.message.as_str())
        .collect::<Vec<_>>();
    assert_eq!(messages, vec!["second", "first"]);
}

#[test]
fn history_is_capped_at_fifty_entries() {
    let mut history = OperationHistory::default();
    for index in 0..60 {
        history.record(history_entry(ActivityOutcome::Started, index.to_string()));
    }

    assert_eq!(history.entries.len(), HISTORY_LIMIT);
    assert_eq!(history.entries.front().unwrap().message, "59");
    assert_eq!(history.entries.back().unwrap().message, "10");
}

#[test]
fn clearing_history_removes_every_entry() {
    let mut history = OperationHistory::default();
    history.record(history_entry(ActivityOutcome::Started, "one"));
    history.record(history_entry(ActivityOutcome::Completed, "two"));

    history.clear();

    assert!(history.entries.is_empty());
}

#[test]
fn history_preserves_success_and_failure_messages() {
    let mut history = OperationHistory::default();
    history.record(history_entry(
        ActivityOutcome::Completed,
        "mounted successfully",
    ));
    history.record(history_entry(
        ActivityOutcome::Failed,
        "ratarmount returned an error",
    ));

    let entries = history.entries().collect::<Vec<_>>();
    assert_eq!(entries[0].outcome, ActivityOutcome::Failed);
    assert_eq!(entries[0].message, "ratarmount returned an error");
    assert_eq!(entries[1].outcome, ActivityOutcome::Completed);
    assert_eq!(entries[1].message, "mounted successfully");
}

#[test]
fn cleanup_is_skipped_when_the_option_is_off() {
    let cleanup_called = std::cell::Cell::new(false);
    let cleanup_started = std::cell::Cell::new(false);
    let success = run_unmount_with_cleanup(
        false,
        || Ok(("unmounted".to_string(), PathBuf::from("/mount/Game"))),
        |_| {
            cleanup_called.set(true);
            Ok(Vec::new())
        },
        |_| cleanup_started.set(true),
    )
    .unwrap();

    assert!(!cleanup_started.get());
    assert!(!cleanup_called.get());
    assert!(success.cleanup.is_none());
}

#[test]
fn cleanup_runs_after_a_successful_unmount_when_enabled() {
    let cleanup_called = std::cell::Cell::new(false);
    let cleanup_started = std::cell::Cell::new(false);
    let success = run_unmount_with_cleanup(
        true,
        || Ok(("unmounted".to_string(), PathBuf::from("/mount/Game"))),
        |mount_path| {
            assert!(cleanup_started.get());
            cleanup_called.set(true);
            assert_eq!(mount_path, Path::new("/mount/Game"));
            Ok(vec![mount_path.to_path_buf()])
        },
        |_| cleanup_started.set(true),
    )
    .unwrap();

    assert!(cleanup_started.get());
    assert!(cleanup_called.get());
    assert!(matches!(
        success.cleanup,
        Some(CleanupOutcome::Completed { .. })
    ));
}

#[test]
fn cleanup_does_not_run_after_a_failed_unmount() {
    let cleanup_called = std::cell::Cell::new(false);
    let cleanup_started = std::cell::Cell::new(false);
    let result = run_unmount_with_cleanup(
        true,
        || {
            Err(OperationFailure {
                message: "unmount failed".to_string(),
                offer_lazy_unmount: true,
            })
        },
        |_| {
            cleanup_called.set(true);
            Ok(Vec::new())
        },
        |_| cleanup_started.set(true),
    );

    assert_eq!(result.unwrap_err().message, "unmount failed");
    assert!(!cleanup_started.get());
    assert!(!cleanup_called.get());
}

#[test]
fn cleanup_failure_preserves_successful_unmount_outcome() {
    let success = run_unmount_with_cleanup(
        true,
        || {
            Ok((
                "unmounted successfully".to_string(),
                PathBuf::from("/mount/Game"),
            ))
        },
        |_| Err("directory is busy".to_string()),
        |_| {},
    )
    .unwrap();

    assert_eq!(success.message, "unmounted successfully");
    let Some(CleanupOutcome::Failed { message, .. }) = success.cleanup else {
        panic!("expected a separate cleanup failure");
    };
    assert!(message.contains("directory is busy"));
}

#[test]
fn cleanup_started_progress_is_recorded_before_the_final_result() {
    let mut app = app_for_operation_tests();
    let archive_path = PathBuf::from("/roms/Game.zip");
    let mount_path = PathBuf::from("/mount/Game");
    let (result_sender, result_receiver) = mpsc::channel();
    let (progress_sender, progress_receiver) = mpsc::channel();
    app.operation = Some(RunningOperation {
        action: ArchiveAction::Unmount,
        archive_path: archive_path.clone(),
        receiver: result_receiver,
        progress_receiver,
    });

    progress_sender
        .send(OperationProgress::CleanupStarted(mount_path.clone()))
        .unwrap();
    app.poll_operation(&egui::Context::default());

    assert!(app.operation.is_some());
    let latest = app.history.entries().next().unwrap();
    assert_eq!(latest.action, ActivityAction::Cleanup);
    assert_eq!(latest.outcome, ActivityOutcome::Started);
    assert_eq!(latest.archive_path.as_deref(), Some(mount_path.as_path()));
    assert!(!app.history.entries().any(|entry| {
        entry.action == ActivityAction::Cleanup
            && matches!(
                entry.outcome,
                ActivityOutcome::Completed | ActivityOutcome::Failed
            )
    }));

    result_sender
        .send(Ok(OperationSuccess {
            message: "unmounted".to_string(),
            cleanup: Some(CleanupOutcome::Completed {
                mount_path: mount_path.clone(),
                message: "cleanup completed".to_string(),
            }),
            warning: None,
        }))
        .unwrap();
    app.poll_operation(&egui::Context::default());

    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::Cleanup
            && entry.outcome == ActivityOutcome::Completed
            && entry.archive_path.as_deref() == Some(mount_path.as_path())
    }));
}

#[test]
fn cleanup_progress_is_not_lost_when_the_final_result_is_already_ready() {
    let mut app = app_for_operation_tests();
    let archive_path = PathBuf::from("/roms/Game.zip");
    let mount_path = PathBuf::from("/mount/Game");
    let (result_sender, result_receiver) = mpsc::channel();
    let (progress_sender, progress_receiver) = mpsc::channel();
    app.operation = Some(RunningOperation {
        action: ArchiveAction::Unmount,
        archive_path,
        receiver: result_receiver,
        progress_receiver,
    });

    progress_sender
        .send(OperationProgress::CleanupStarted(mount_path.clone()))
        .unwrap();
    result_sender
        .send(Ok(OperationSuccess {
            message: "unmounted".to_string(),
            cleanup: Some(CleanupOutcome::Completed {
                mount_path: mount_path.clone(),
                message: "cleanup completed".to_string(),
            }),
            warning: None,
        }))
        .unwrap();

    app.poll_operation(&egui::Context::default());

    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::Cleanup
            && entry.outcome == ActivityOutcome::Started
            && entry.archive_path.as_deref() == Some(mount_path.as_path())
    }));
    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::Cleanup
            && entry.outcome == ActivityOutcome::Completed
            && entry.archive_path.as_deref() == Some(mount_path.as_path())
    }));
}

#[test]
fn activity_records_cleanup_success_and_failure_with_mount_paths() {
    let mount_path = PathBuf::from("/mount/Platform/Game");
    let mut history = OperationHistory::default();
    record_cleanup_started_activity(&mut history, &mount_path);
    record_cleanup_finished_activity(
        &mut history,
        &CleanupOutcome::Completed {
            mount_path: mount_path.clone(),
            message: "cleanup succeeded".to_string(),
        },
    );
    record_cleanup_started_activity(&mut history, &mount_path);
    record_cleanup_finished_activity(
        &mut history,
        &CleanupOutcome::Failed {
            mount_path: mount_path.clone(),
            message: "cleanup failed".to_string(),
        },
    );

    let entries = history.entries().collect::<Vec<_>>();
    assert_eq!(entries[0].action, ActivityAction::Cleanup);
    assert_eq!(entries[0].outcome, ActivityOutcome::Failed);
    assert_eq!(
        entries[0].archive_path.as_deref(),
        Some(mount_path.as_path())
    );
    assert_eq!(entries[0].message, "cleanup failed");
    assert_eq!(entries[2].outcome, ActivityOutcome::Completed);
    assert_eq!(entries[2].message, "cleanup succeeded");
    assert!(
        entries[1]
            .message
            .contains(&mount_path.display().to_string())
    );
    assert!(
        entries[3]
            .message
            .contains(&mount_path.display().to_string())
    );
}

#[test]
fn lazy_unmount_is_unavailable_before_normal_unmount_failure() {
    let mounted = record("/roms/Game.zip", MountState::Mounted);

    assert!(!lazy_unmount_available(&mounted, &HashSet::new(), false));
    assert!(!lazy_unmount_available(
        &mounted,
        &HashSet::from([PathBuf::from("/roms/Other.zip")]),
        false
    ));
    assert!(lazy_unmount_available(
        &mounted,
        &HashSet::from([PathBuf::from("/roms/Game.zip")]),
        false
    ));
}

#[test]
fn lazy_unmount_requires_matching_confirmation_and_is_blocked_while_busy() {
    let archive = Path::new("/roms/Game.zip");

    assert!(!lazy_confirmation_available(
        archive,
        &HashSet::new(),
        false
    ));
    assert!(!lazy_confirmation_available(
        archive,
        &HashSet::from([PathBuf::from("/roms/Other.zip")]),
        false
    ));
    let offered = HashSet::from([archive.to_path_buf()]);
    assert!(lazy_confirmation_available(archive, &offered, false));
    assert!(!lazy_confirmation_available(archive, &offered, true));
}

#[test]
fn remount_is_available_only_for_the_successfully_unmounted_archive() {
    let pending = record("/roms/Game.zip", MountState::Pending);
    let mounted = record("/roms/Game.zip", MountState::Mounted);
    let no_offers = HashSet::new();
    let other_offer = HashSet::from([PathBuf::from("/roms/Other.zip")]);
    let offer = HashSet::from([PathBuf::from("/roms/Game.zip")]);

    assert!(!remount_available(&pending, &no_offers, false));
    assert!(!remount_available(&pending, &other_offer, false));
    assert!(remount_available(&pending, &offer, false));
    assert!(!remount_available(&mounted, &offer, false));
    assert!(!remount_available(&pending, &offer, true));
    assert!(remount_is_offered(&pending, &offer));
}

#[test]
fn normal_unmount_failure_offers_lazy_recovery_and_records_activity() {
    let mut app = app_for_operation_tests();
    let archive_path = PathBuf::from("/roms/Game.zip");
    let (sender, receiver) = mpsc::channel();
    app.operation = Some(RunningOperation {
        action: ArchiveAction::Unmount,
        archive_path: archive_path.clone(),
        receiver,
        progress_receiver: mpsc::channel().1,
    });
    sender
        .send(Err(OperationFailure {
            message: "mount is busy".to_string(),
            offer_lazy_unmount: true,
        }))
        .unwrap();

    app.poll_operation(&egui::Context::default());

    assert!(app.lazy_unmount_offers.contains(&archive_path));
    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::Unmount
            && entry.outcome == ActivityOutcome::Failed
            && entry.message.contains("busy")
    }));
    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::LazyUnmount && entry.outcome == ActivityOutcome::Offered
    }));
    let feedback = app.feedback.as_ref().unwrap();
    assert_eq!(feedback.message, NORMAL_UNMOUNT_FAILURE_SUMMARY);
    assert!(
        feedback
            .more_information
            .as_deref()
            .unwrap()
            .contains("Try Normal Unmount again")
    );
}

#[test]
fn successful_lazy_unmount_with_cleanup_failure_still_offers_remount() {
    let mut app = app_for_operation_tests();
    let archive_path = PathBuf::from("/roms/Game.zip");
    let mount_path = PathBuf::from("/mount/Game");
    let (sender, receiver) = mpsc::channel();
    app.lazy_unmount_offers.insert(archive_path.clone());
    app.operation = Some(RunningOperation {
        action: ArchiveAction::LazyUnmount,
        archive_path: archive_path.clone(),
        receiver,
        progress_receiver: mpsc::channel().1,
    });
    sender
        .send(Ok(OperationSuccess {
            message: "lazy unmount completed".to_string(),
            cleanup: Some(CleanupOutcome::Failed {
                mount_path,
                message: "cleanup failed".to_string(),
            }),
            warning: Some("lazy warning".to_string()),
        }))
        .unwrap();

    app.poll_operation(&egui::Context::default());

    assert!(app.remount_offers.contains(&archive_path));
    assert!(!app.lazy_unmount_offers.contains(&archive_path));
    assert!(app.feedback.as_ref().unwrap().succeeded);
    assert!(
        !app.feedback
            .as_ref()
            .unwrap()
            .cleanup
            .as_ref()
            .unwrap()
            .succeeded
    );
    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::LazyUnmount && entry.outcome == ActivityOutcome::Completed
    }));
    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::Cleanup && entry.outcome == ActivityOutcome::Failed
    }));
    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::Remount && entry.outcome == ActivityOutcome::Offered
    }));
}

#[test]
fn successful_remount_clears_offer_and_records_completion() {
    let mut app = app_for_operation_tests();
    let archive_path = PathBuf::from("/roms/Game.zip");
    let other_archive = PathBuf::from("/roms/Other.zip");
    let (sender, receiver) = mpsc::channel();
    app.remount_offers.insert(archive_path.clone());
    app.remount_offers.insert(other_archive.clone());
    app.operation = Some(RunningOperation {
        action: ArchiveAction::Remount,
        archive_path: archive_path.clone(),
        receiver,
        progress_receiver: mpsc::channel().1,
    });
    sender
        .send(Ok(OperationSuccess {
            message: "remounted".to_string(),
            cleanup: None,
            warning: None,
        }))
        .unwrap();

    app.poll_operation(&egui::Context::default());

    assert!(!app.remount_offers.contains(&archive_path));
    assert!(app.remount_offers.contains(&other_archive));
    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::Remount && entry.outcome == ActivityOutcome::Completed
    }));
}

#[test]
fn successful_normal_unmount_offers_remount() {
    let mut app = app_for_operation_tests();
    let archive_path = PathBuf::from("/roms/Game.zip");
    let (sender, receiver) = mpsc::channel();
    app.operation = Some(RunningOperation {
        action: ArchiveAction::Unmount,
        archive_path: archive_path.clone(),
        receiver,
        progress_receiver: mpsc::channel().1,
    });
    sender
        .send(Ok(OperationSuccess {
            message: "unmounted".to_string(),
            cleanup: None,
            warning: None,
        }))
        .unwrap();

    app.poll_operation(&egui::Context::default());

    assert!(app.remount_offers.contains(&archive_path));
}

#[test]
fn failed_remount_preserves_offer_and_records_failure() {
    let mut app = app_for_operation_tests();
    let archive_path = PathBuf::from("/roms/Game.zip");
    let (sender, receiver) = mpsc::channel();
    app.remount_offers.insert(archive_path.clone());
    app.operation = Some(RunningOperation {
        action: ArchiveAction::Remount,
        archive_path: archive_path.clone(),
        receiver,
        progress_receiver: mpsc::channel().1,
    });
    sender
        .send(Err(OperationFailure {
            message: "mount path is still active".to_string(),
            offer_lazy_unmount: false,
        }))
        .unwrap();

    app.poll_operation(&egui::Context::default());

    assert!(app.remount_offers.contains(&archive_path));
    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::Remount
            && entry.outcome == ActivityOutcome::Failed
            && entry.message.contains("still active")
    }));
}

#[test]
fn mounting_another_archive_preserves_existing_remount_offer() {
    let mut app = app_for_operation_tests();
    let offered_archive = PathBuf::from("/roms/Game.zip");
    let mounted_archive = PathBuf::from("/roms/Other.zip");
    let (sender, receiver) = mpsc::channel();
    app.remount_offers.insert(offered_archive.clone());
    app.operation = Some(RunningOperation {
        action: ArchiveAction::Mount,
        archive_path: mounted_archive,
        receiver,
        progress_receiver: mpsc::channel().1,
    });
    sender
        .send(Ok(OperationSuccess {
            message: "mounted".to_string(),
            cleanup: None,
            warning: None,
        }))
        .unwrap();

    app.poll_operation(&egui::Context::default());

    assert!(app.remount_offers.contains(&offered_archive));
}

#[test]
fn recovery_wording_is_explicit_and_avoids_aggressive_terms() {
    let wording = format!(
        "{NORMAL_UNMOUNT_FAILURE_SUMMARY}\n{NORMAL_UNMOUNT_RECOVERY_GUIDANCE}\n{LAZY_UNMOUNT_WARNING}\n{LAZY_UNMOUNT_SUCCESS}\n{REMOUNT_GUIDANCE}"
    );

    assert!(wording.contains("not responding correctly"));
    assert!(wording.contains("still has files open"));
    assert!(wording.contains("Normal Unmount repeatedly fails"));
    for avoided in ["wedged", "force kill", "nuke"] {
        assert!(!wording.to_lowercase().contains(avoided));
    }
}

#[test]
fn lazy_unmount_advances_to_a_separate_final_confirmation() {
    let archive = PathBuf::from("/roms/Game.zip");
    let mut warning_confirmation = Some(archive.clone());
    let mut final_confirmation = None;
    let mut focus_final_cancel = false;

    advance_to_final_lazy_confirmation(
        &mut warning_confirmation,
        &mut final_confirmation,
        &mut focus_final_cancel,
        &archive,
    );

    assert!(warning_confirmation.is_none());
    assert_eq!(final_confirmation.as_deref(), Some(archive.as_path()));
    assert!(focus_final_cancel);
}

#[test]
fn recovery_activity_records_cancel_retry_and_confirmation() {
    let archive = Path::new("/roms/Game.zip");
    let mut history = OperationHistory::default();
    record_recovery_activity(
        &mut history,
        ActivityAction::LazyUnmount,
        archive,
        ActivityOutcome::Cancelled,
        "User cancelled lazy unmount.",
    );
    record_recovery_activity(
        &mut history,
        ActivityAction::Unmount,
        archive,
        ActivityOutcome::Retried,
        "Normal unmount retried.",
    );
    record_recovery_activity(
        &mut history,
        ActivityAction::LazyUnmount,
        archive,
        ActivityOutcome::Confirmed,
        "Lazy unmount confirmed.",
    );

    let entries = history.entries().collect::<Vec<_>>();
    assert_eq!(entries[0].outcome, ActivityOutcome::Confirmed);
    assert_eq!(entries[1].outcome, ActivityOutcome::Retried);
    assert_eq!(entries[2].outcome, ActivityOutcome::Cancelled);
    assert!(entries.iter().all(|entry| {
        entry.archive_path.as_deref() == Some(archive) && !entry.message.trim().is_empty()
    }));
}

#[test]
fn unmount_all_selects_only_mounted_archives() {
    let records = vec![
        record("/roms/Mounted.zip", MountState::Mounted),
        record("/roms/Pending.zip", MountState::Pending),
        record("/roms/Existing.zip", MountState::MountPathExists),
    ];

    let selected = pending_unmount_items(&records);

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].archive_path, PathBuf::from("/roms/Mounted.zip"));
}

#[test]
fn unmount_all_is_sequential_continues_and_keeps_cleanup_failure_separate() {
    let items = vec![
        unmount_all_item("One"),
        unmount_all_item("Two"),
        unmount_all_item("Three"),
    ];
    let stop = AtomicBool::new(false);
    let mut order = Vec::new();
    let mut events = Vec::new();

    let result = run_unmount_all_coordinator(
        items,
        &stop,
        |item| {
            order.push(item.display_name.clone());
            match item.display_name.as_str() {
                "One" => Ok(BatchUnmountAttempt::Unmounted),
                "Two" => Err(BatchUnmountError {
                    message: "mount is busy".to_string(),
                    offer_lazy_unmount: true,
                }),
                _ => Ok(BatchUnmountAttempt::NotMounted),
            }
        },
        |item, publish| {
            (item.display_name == "One").then(|| {
                publish(UnmountAllEvent::CleanupStarted(item.mount_path.clone()));
                Err("directory remained".to_string())
            })
        },
        |event| events.push(event),
    );

    assert_eq!(order, ["One", "Two", "Three"]);
    assert_eq!(result.attempted(), 2);
    assert_eq!(result.successful, 1);
    assert_eq!(result.failures.len(), 1);
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.cleanup_successes, 0);
    assert_eq!(result.cleanup_failures.len(), 1);
    assert!(result.completion_message().contains("1 failure"));
    let completed_index = events
        .iter()
        .position(|event| matches!(event, UnmountAllEvent::ArchiveCompleted(_)))
        .unwrap();
    let cleanup_index = events
        .iter()
        .position(|event| matches!(event, UnmountAllEvent::CleanupStarted(_)))
        .unwrap();
    assert!(completed_index < cleanup_index);
}

#[test]
fn unmount_all_stop_after_current_leaves_later_items_unattempted() {
    let items = vec![
        unmount_all_item("One"),
        unmount_all_item("Two"),
        unmount_all_item("Three"),
    ];
    let stop = AtomicBool::new(false);
    let result = run_unmount_all_coordinator(
        items,
        &stop,
        |_| {
            stop.store(true, Ordering::Release);
            Ok(BatchUnmountAttempt::Unmounted)
        },
        |_, _| None,
        |_| {},
    );

    assert!(result.stopped);
    assert_eq!(result.successful, 1);
    assert_eq!(result.unattempted, 2);
}

#[test]
fn unmount_all_setup_failure_is_terminal_and_truthful() {
    let result = UnmountAllResult::setup_failed(7, "mountinfo unavailable");

    assert_eq!(result.completion_message(), "Unmount All could not start.");
    assert_eq!(result.attempted(), 0);
    assert_eq!(result.successful, 0);
    assert!(result.failures.is_empty());
    assert!(result.skipped.is_empty());
    assert_eq!(result.unattempted, 7);

    let cleanup_only_failure = UnmountAllResult {
        total: 1,
        successful: 1,
        cleanup_failures: vec![UnmountAllCleanupFailure {
            mount_path: PathBuf::from("/mount/Game"),
            message: "directory remained".to_string(),
        }],
        ..Default::default()
    };
    assert_eq!(
        cleanup_only_failure.completion_message(),
        "Unmount All completed, but cleanup failed for 1 mount."
    );
}

#[test]
fn unmount_all_marks_the_app_busy_and_blocks_individual_actions() {
    let mut app = app_for_operation_tests();
    let (_sender, receiver) = mpsc::channel();
    app.unmount_all = Some(RunningUnmountAll {
        receiver,
        stop: Arc::new(AtomicBool::new(false)),
        progress: UnmountAllProgress::default(),
    });

    assert!(app.is_busy());
    assert!(!individual_actions_available(app.is_busy()));
}

#[test]
fn unmount_all_activity_records_batch_archive_cleanup_and_recovery_lifecycle() {
    let mut app = app_for_operation_tests();
    let item = unmount_all_item("Game");
    let failed = unmount_all_item("Busy");
    let (sender, receiver) = mpsc::channel();
    app.unmount_all = Some(RunningUnmountAll {
        receiver,
        stop: Arc::new(AtomicBool::new(false)),
        progress: UnmountAllProgress {
            total: 2,
            ..Default::default()
        },
    });
    sender
        .send(UnmountAllEvent::ArchiveStarted {
            index: 1,
            total: 2,
            item: item.clone(),
        })
        .unwrap();
    sender
        .send(UnmountAllEvent::ArchiveCompleted(item.clone()))
        .unwrap();
    sender
        .send(UnmountAllEvent::CleanupStarted(item.mount_path.clone()))
        .unwrap();
    sender
        .send(UnmountAllEvent::CleanupCompleted(item.mount_path.clone()))
        .unwrap();
    sender
        .send(UnmountAllEvent::ArchiveFailed {
            item: failed.clone(),
            message: "mount is busy".to_string(),
            offer_lazy_unmount: true,
        })
        .unwrap();
    sender
        .send(UnmountAllEvent::Finished(UnmountAllResult {
            total: 2,
            successful: 1,
            failures: vec![UnmountAllFailure {
                archive_path: failed.archive_path.clone(),
                message: "mount is busy".to_string(),
                offer_lazy_unmount: true,
            }],
            cleanup_successes: 1,
            ..Default::default()
        }))
        .unwrap();

    app.poll_unmount_all(&egui::Context::default());

    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::Unmount && entry.outcome == ActivityOutcome::Started
    }));
    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::Cleanup && entry.outcome == ActivityOutcome::Completed
    }));
    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::Unmount
            && entry.outcome == ActivityOutcome::Failed
            && entry.message.contains("busy")
    }));
    assert!(app.history.entries().any(|entry| {
        entry.action == ActivityAction::UnmountAll && entry.outcome == ActivityOutcome::Completed
    }));
    assert!(app.lazy_unmount_offers.contains(&failed.archive_path));
}

#[test]
fn successful_batch_unmount_clears_only_its_previous_lazy_offer() {
    let mut app = app_for_operation_tests();
    let item = unmount_all_item("Game");
    let other = PathBuf::from("/roms/Other.zip");
    app.lazy_unmount_offers = HashSet::from([item.archive_path.clone(), other.clone()]);
    let (sender, receiver) = mpsc::channel();
    app.unmount_all = Some(RunningUnmountAll {
        receiver,
        stop: Arc::new(AtomicBool::new(false)),
        progress: UnmountAllProgress::default(),
    });
    sender
        .send(UnmountAllEvent::ArchiveCompleted(item.clone()))
        .unwrap();

    app.poll_unmount_all(&egui::Context::default());

    assert!(!app.lazy_unmount_offers.contains(&item.archive_path));
    assert!(app.lazy_unmount_offers.contains(&other));
    let mounted_again = record("/roms/Game.zip", MountState::Mounted);
    assert!(!lazy_unmount_available(
        &mounted_again,
        &app.lazy_unmount_offers,
        false,
    ));
}

#[test]
fn no_longer_mounted_batch_skip_clears_only_its_previous_lazy_offer() {
    let mut app = app_for_operation_tests();
    let item = unmount_all_item("Game");
    let other = PathBuf::from("/roms/Other.zip");
    app.lazy_unmount_offers = HashSet::from([item.archive_path.clone(), other.clone()]);
    let (sender, receiver) = mpsc::channel();
    app.unmount_all = Some(RunningUnmountAll {
        receiver,
        stop: Arc::new(AtomicBool::new(false)),
        progress: UnmountAllProgress::default(),
    });
    sender
        .send(UnmountAllEvent::ArchiveSkipped {
            item: item.clone(),
            reason: "archive is no longer mounted".to_string(),
        })
        .unwrap();

    app.poll_unmount_all(&egui::Context::default());

    assert!(!app.lazy_unmount_offers.contains(&item.archive_path));
    assert!(app.lazy_unmount_offers.contains(&other));
}

#[test]
fn failed_normal_batch_unmount_retains_its_exact_lazy_offer() {
    let mut app = app_for_operation_tests();
    let item = unmount_all_item("Busy");
    let (sender, receiver) = mpsc::channel();
    app.unmount_all = Some(RunningUnmountAll {
        receiver,
        stop: Arc::new(AtomicBool::new(false)),
        progress: UnmountAllProgress::default(),
    });
    sender
        .send(UnmountAllEvent::ArchiveFailed {
            item: item.clone(),
            message: "mount is busy".to_string(),
            offer_lazy_unmount: true,
        })
        .unwrap();

    app.poll_unmount_all(&egui::Context::default());

    assert!(app.lazy_unmount_offers.contains(&item.archive_path));
}

#[test]
fn missing_config_load_opens_setup_instead_of_leaving_a_fatal_view() {
    let mut app = app_for_operation_tests();
    let (_diagnostics_sender, diagnostics_receiver) = mpsc::channel();
    app.diagnostics = DiagnosticsState::Loading {
        generation: RefreshGeneration::INITIAL,
        receiver: diagnostics_receiver,
    };
    let (sender, receiver) = mpsc::channel();
    app.state = LoadState::Loading {
        generation: RefreshGeneration::INITIAL,
        receiver,
        previous: None,
    };
    sender
        .send((
            RefreshGeneration::INITIAL,
            Err("missing /config/archivefs.toml".to_string()),
        ))
        .unwrap();

    app.poll_load(&egui::Context::default());

    assert_eq!(app.tools_overlay, ToolsOverlay::Diagnostics);
    assert!(matches!(app.state, LoadState::Error(_)));
    assert!(matches!(app.diagnostics, DiagnosticsState::Loading { .. }));
    assert!(!diagnostics_state_can_continue(&app.diagnostics));
    assert!(app.refresh_error.is_some());
}

#[test]
fn failed_refresh_retains_snapshot_and_invalidates_stale_diagnostics() {
    let mut app = app_for_operation_tests();
    let (_diagnostics_sender, diagnostics_receiver) = mpsc::channel();
    app.diagnostics = DiagnosticsState::Loading {
        generation: RefreshGeneration::INITIAL,
        receiver: diagnostics_receiver,
    };
    let (sender, receiver) = mpsc::channel();
    app.state = LoadState::Loading {
        generation: RefreshGeneration::INITIAL,
        receiver,
        previous: Some(Box::new(empty_loaded_data("/old-mount"))),
    };
    sender
        .send((
            RefreshGeneration::INITIAL,
            Err("config became invalid".to_string()),
        ))
        .unwrap();

    app.poll_load(&egui::Context::default());

    assert!(matches!(
        &app.state,
        LoadState::Ready(data) if data.mount_root == Path::new("/old-mount")
    ));
    assert!(app.snapshot_stale);
    assert!(matches!(app.diagnostics, DiagnosticsState::Loading { .. }));
    assert!(!diagnostics_state_can_continue(&app.diagnostics));
    assert_eq!(app.refresh_error.as_deref(), Some("config became invalid"));
}

#[test]
fn retry_success_replaces_the_old_snapshot_and_clears_error() {
    let mut app = app_for_operation_tests();
    let (sender, receiver) = mpsc::channel();
    app.state = LoadState::Loading {
        generation: RefreshGeneration::INITIAL,
        receiver,
        previous: Some(Box::new(empty_loaded_data("/old-mount"))),
    };
    app.refresh_error = Some("old failure".to_string());
    app.snapshot_stale = true;
    sender
        .send((
            RefreshGeneration::INITIAL,
            Ok(empty_loaded_data("/new-mount")),
        ))
        .unwrap();

    app.poll_load(&egui::Context::default());

    assert!(matches!(
        &app.state,
        LoadState::Ready(data) if data.mount_root == Path::new("/new-mount")
    ));
    assert!(!app.snapshot_stale);
    assert!(app.refresh_error.is_none());
}

#[test]
fn fresh_invalid_diagnostics_keep_setup_open() {
    let mut app = app_for_operation_tests();
    app.tools_overlay = ToolsOverlay::Diagnostics;
    let (sender, receiver) = mpsc::channel();
    app.diagnostics = DiagnosticsState::Loading {
        generation: RefreshGeneration::INITIAL,
        receiver,
    };
    sender
        .send((RefreshGeneration::INITIAL, setup_report(false, false)))
        .unwrap();

    app.poll_diagnostics();

    assert_eq!(app.tools_overlay, ToolsOverlay::Diagnostics);
    assert!(!diagnostics_state_can_continue(&app.diagnostics));
}

#[test]
fn successful_refresh_invalidates_stale_action_readiness() {
    let mut app = app_for_operation_tests();
    assert!(latest_generation_actions_safe(
        app.refresh_generation,
        app.snapshot_generation,
        app.snapshot_stale,
        snapshot_identity(&app.state),
        &app.diagnostics,
    ));

    app.refresh(&egui::Context::default());
    let current = app.refresh_generation;
    app.state = LoadState::Ready(Box::new(empty_loaded_data("/new-mount")));
    app.snapshot_generation = Some(current);

    assert!(matches!(
        app.diagnostics,
        DiagnosticsState::Loading {
            generation,
            ..
        } if generation == current
    ));
    assert!(!latest_generation_actions_safe(
        current,
        app.snapshot_generation,
        app.snapshot_stale,
        snapshot_identity(&app.state),
        &app.diagnostics,
    ));
}

#[test]
fn refresh_recomputes_action_readiness_as_true_once_diagnostics_complete() {
    // The other half of `successful_refresh_invalidates_stale_action_
    // readiness`: readiness must not just correctly go stale mid-
    // refresh, it must also correctly come back once the new
    // diagnostics generation actually completes as Ready - this is
    // "a refresh after startup recomputes action readiness correctly".
    let mut app = app_for_operation_tests();
    app.refresh(&egui::Context::default());
    let current = app.refresh_generation;
    app.state = LoadState::Ready(Box::new(empty_loaded_data("/new-mount")));
    app.snapshot_generation = Some(current);
    assert!(!latest_generation_actions_safe(
        current,
        app.snapshot_generation,
        app.snapshot_stale,
        snapshot_identity(&app.state),
        &app.diagnostics,
    ));

    app.diagnostics = DiagnosticsState::Ready {
        generation: current,
        report: setup_report(true, true),
    };

    assert!(
        latest_generation_actions_safe(
            current,
            app.snapshot_generation,
            app.snapshot_stale,
            snapshot_identity(&app.state),
            &app.diagnostics,
        ),
        "action readiness must recompute to true once the new generation's \
             diagnostics complete as Ready"
    );
    assert_eq!(
        archive_action_block_reason(
            app.is_busy(),
            current,
            app.snapshot_generation,
            app.snapshot_stale,
            snapshot_identity(&app.state),
            &app.diagnostics,
        ),
        None,
        "block_reason must agree: no reason once readiness is restored"
    );
}

#[test]
fn opening_and_closing_the_archive_inspector_does_not_stale_action_readiness() {
    // `start_archive_inspection` only ever touches `archive_inspector`/
    // `archive_inspector_generation`/`tools_overlay` - never
    // `refresh_generation`, `snapshot_generation`, `snapshot_stale`, or
    // `diagnostics`, the only fields `latest_generation_actions_safe`
    // reads. This proves that structural claim end to end rather than
    // just by inspection.
    let mut app = app_for_operation_tests();
    let before = latest_generation_actions_safe(
        app.refresh_generation,
        app.snapshot_generation,
        app.snapshot_stale,
        snapshot_identity(&app.state),
        &app.diagnostics,
    );
    assert!(before, "the fixture must start in the actions-safe state");

    app.start_archive_inspection(egui::Context::default(), PathBuf::from("/roms/a.zip"));
    assert_eq!(app.tools_overlay, ToolsOverlay::ArchiveInspector);
    assert!(
        latest_generation_actions_safe(
            app.refresh_generation,
            app.snapshot_generation,
            app.snapshot_stale,
            snapshot_identity(&app.state),
            &app.diagnostics,
        ),
        "opening Inspector must not stale action readiness"
    );

    app.tools_overlay = ToolsOverlay::None;
    assert!(
        latest_generation_actions_safe(
            app.refresh_generation,
            app.snapshot_generation,
            app.snapshot_stale,
            snapshot_identity(&app.state),
            &app.diagnostics,
        ),
        "closing Inspector must not stale action readiness"
    );
}

#[test]
fn newer_unsafe_config_cannot_inherit_old_action_readiness() {
    let current = RefreshGeneration(2);
    let old_ready = DiagnosticsState::Ready {
        generation: RefreshGeneration(1),
        report: setup_report(true, true),
    };
    assert!(!latest_generation_actions_safe(
        current,
        Some(current),
        false,
        Some(&default_config_identity()),
        &old_ready,
    ));

    let current_unsafe = DiagnosticsState::Ready {
        generation: current,
        report: setup_report(true, false),
    };
    assert!(!latest_generation_actions_safe(
        current,
        Some(current),
        false,
        Some(&default_config_identity()),
        &current_unsafe,
    ));
}

#[test]
fn late_diagnostics_from_an_older_generation_are_ignored() {
    let mut app = app_for_operation_tests();
    let current = RefreshGeneration(2);
    app.refresh_generation = current;
    let (sender, receiver) = mpsc::channel();
    app.diagnostics = DiagnosticsState::Loading {
        generation: current,
        receiver,
    };
    sender
        .send((RefreshGeneration(1), setup_report(true, true)))
        .unwrap();

    app.poll_diagnostics();

    assert!(matches!(
        app.diagnostics,
        DiagnosticsState::Loading {
            generation,
            ..
        } if generation == current
    ));
    assert!(!latest_generation_actions_safe(
        current,
        Some(current),
        false,
        snapshot_identity(&app.state),
        &app.diagnostics,
    ));
}

#[test]
fn actions_require_current_valid_snapshot_and_diagnostics() {
    let current = RefreshGeneration(4);
    let ready = DiagnosticsState::Ready {
        generation: current,
        report: setup_report(true, true),
    };
    assert!(latest_generation_actions_safe(
        current,
        Some(current),
        false,
        Some(&default_config_identity()),
        &ready,
    ));
    assert!(!latest_generation_actions_safe(
        current,
        Some(RefreshGeneration(3)),
        false,
        Some(&default_config_identity()),
        &ready,
    ));
}

#[test]
fn disconnected_diagnostics_stop_loading_and_allow_retry() {
    let mut app = app_for_operation_tests();
    let snapshot_root = PathBuf::from("/last-good");
    app.state = LoadState::Ready(Box::new(empty_loaded_data("/last-good")));
    let (sender, receiver) = mpsc::channel::<DiagnosticsMessage>();
    drop(sender);
    app.diagnostics = DiagnosticsState::Loading {
        generation: app.refresh_generation,
        receiver,
    };

    app.poll_diagnostics();

    assert!(matches!(
        &app.diagnostics,
        DiagnosticsState::Error { message, .. }
            if message.contains("Run diagnostics again")
    ));
    assert!(matches!(
        &app.state,
        LoadState::Ready(data) if data.mount_root == snapshot_root
    ));
    assert!(!diagnostics_state_can_continue(&app.diagnostics));
    assert_eq!(app.tools_overlay, ToolsOverlay::Diagnostics);
    assert!(!latest_generation_actions_safe(
        app.refresh_generation,
        app.snapshot_generation,
        app.snapshot_stale,
        snapshot_identity(&app.state),
        &app.diagnostics,
    ));
}

#[test]
fn ready_diagnostics_allow_continue_and_can_be_reopened() {
    let report = setup_report(true, false);
    assert!(diagnostics_can_continue(&report));

    let mut app = app_for_operation_tests();
    assert_eq!(app.tools_overlay, ToolsOverlay::None);
    app.tools_overlay = ToolsOverlay::Diagnostics;
    assert_eq!(app.tools_overlay, ToolsOverlay::Diagnostics);
}

#[test]
fn starter_config_requires_a_resolved_confirmed_missing_path() {
    let mut report = setup_report(false, false);
    report.config_missing = false;
    report.config_path_error = Some("HOME is unavailable".to_string());
    assert!(!starter_config_available(&report));

    report.config_path_error = None;
    report.config_missing = true;
    assert!(starter_config_available(&report));
}

#[test]
fn missing_config_reads_as_first_run_only_when_never_previously_confirmed() {
    // A genuine fresh install: nothing has been seen yet this session.
    assert!(missing_config_is_first_run(false));
    // The config was found present and readable earlier this session,
    // and is now gone - this must never read as an ordinary first run.
    assert!(!missing_config_is_first_run(true));
}

#[test]
fn unresolved_config_path_disables_starter_config_and_path_actions() {
    let mut report = setup_report(false, false);
    report.config_path = None;
    report.config_path_error = Some("HOME and USERPROFILE are unavailable".to_string());
    report.config_missing = true;

    assert!(report.config_path.is_none());
    assert!(!starter_config_available(&report));
}

#[test]
fn resolved_config_path_allows_path_actions() {
    let report = setup_report(true, true);
    assert!(report.config_path.is_some());
}

#[test]
fn mismatched_config_identity_blocks_actions_despite_matching_generation() {
    let current = RefreshGeneration(7);
    let ready = DiagnosticsState::Ready {
        generation: current,
        report: setup_report(true, true),
    };
    let different_identity = ConfigIdentity {
        config_path: Some(PathBuf::from("/config/archivefs.toml")),
        content_digest: Some([2; 32]),
    };

    assert!(!latest_generation_actions_safe(
        current,
        Some(current),
        false,
        Some(&different_identity),
        &ready,
    ));
    assert!(latest_generation_actions_safe(
        current,
        Some(current),
        false,
        Some(&default_config_identity()),
        &ready,
    ));
}

#[test]
fn config_changed_between_worker_starts_cannot_produce_trusted_combined_state() {
    let mut app = app_for_operation_tests();
    let current = app.refresh_generation;
    app.state = LoadState::Ready(Box::new(empty_loaded_data("/mount")));
    let changed_identity = ConfigIdentity {
        config_path: Some(PathBuf::from("/config/archivefs.toml")),
        content_digest: Some([9; 32]),
    };
    app.diagnostics = DiagnosticsState::Ready {
        generation: current,
        report: SetupDiagnostics {
            config_identity: changed_identity,
            ..setup_report(true, true)
        },
    };

    assert!(!latest_generation_actions_safe(
        app.refresh_generation,
        app.snapshot_generation,
        app.snapshot_stale,
        snapshot_identity(&app.state),
        &app.diagnostics,
    ));
}

#[test]
fn setup_failure_preserves_the_last_valid_snapshot() {
    let mut app = app_for_operation_tests();
    app.state = LoadState::Ready(Box::new(empty_loaded_data("/mount")));
    let (sender, receiver) = mpsc::channel();
    app.setup_action = Some(RunningSetupAction {
        action: SetupAction::OpenConfigFolder,
        receiver,
    });
    sender
        .send(Err("could not open folder".to_string()))
        .unwrap();

    app.poll_setup_action(&egui::Context::default());

    assert!(matches!(app.state, LoadState::Ready(_)));
    assert!(!app.feedback.as_ref().unwrap().succeeded);
}

// -----------------------------------------------------------------
// Stage 4: persistent library database GUI integration - tests.
// -----------------------------------------------------------------

#[test]
fn startup_with_no_database_is_reported_as_not_created_not_as_an_error() {
    let dir = database_test_dir("no-database");
    let database_path = dir.join("library.sqlite3");

    let config_path = dir.join("config.toml");
    let result = load_database_snapshot_at(&database_path, &config_path, None);

    assert!(matches!(
        result,
        Err(DatabaseLoadError::NotCreated { database_path: reported }) if reported == database_path
    ));
    let _ = std::fs::remove_dir_all(&dir);
}
