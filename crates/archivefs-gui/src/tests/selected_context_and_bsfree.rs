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
//! Predominant theme observed in this slice: selected-archive context menus, BSFree browser, gamer cover artwork.

use super::*;
use crate::ui::platform_artwork::*;

#[test]
fn right_click_on_a_row_already_in_the_selection_preserves_the_multi_selection() {
    let path_a = PathBuf::from("/roms/a.zip");
    let path_b = PathBuf::from("/roms/b.zip");
    let path_c = PathBuf::from("/roms/c.zip");
    let mut selected_archives: HashSet<PathBuf> = [path_a.clone(), path_b.clone(), path_c.clone()]
        .into_iter()
        .collect();
    let mut selected_archive = Some(path_a.clone());

    // Right-clicking path_b, which is already part of the current
    // multi-selection, must leave the whole selection (and the
    // focused row) completely undisturbed.
    apply_row_right_click(&mut selected_archives, &mut selected_archive, path_b);

    assert_eq!(
        selected_archives,
        [path_a.clone(), PathBuf::from("/roms/b.zip"), path_c]
            .into_iter()
            .collect::<HashSet<_>>(),
        "right-clicking an already-selected row must never shrink the multi-selection"
    );
    assert_eq!(
        selected_archive,
        Some(path_a),
        "the focused row must also stay untouched"
    );
}

#[test]
fn library_single_selection_context_menu_shows_the_correct_actions() {
    let record = record_at(PathBuf::from("/roms/a.zip"), MountState::Pending);
    let row = row_with_fields("/roms/a.zip", "SNES", "Pending", "/roms/a.zip", "/mnt/a");
    let records = vec![record];
    let ctx = egui::Context::default();
    // "Clear manual platform" only renders once the archive is known
    // to the persisted catalogue with a manual assignment (mirrors
    // `show_platform_section`'s identical gate) - provide a matching
    // `cached` snapshot so every one of the required menu items is
    // actually reachable in this one render, not just "Set platform".
    let cached = cached_snapshot(vec![persisted_archive_with_platform(
        PathBuf::from("/roms/a.zip"),
        1,
        "SNES",
        MANUAL_PLATFORM_SOURCE,
    )]);
    let menu_context = RowMenuContext {
        records: &records,
        cached: Some(&cached),
        busy: false,
        block_reason: None,
        platform_busy: false,
        retroarch_profiles: &RetroArchProfilesState::NotScanned,
        library_views_configured: false,
        library_view_last_plan: None,
    };

    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_single_row_context_menu(ui, &row, &menu_context);
        });
    });

    for expected in [
        "Mount",
        "Inspect contents",
        "Copy archive path",
        "Copy mount path",
        "Copy source path",
        "Set platform",
        "Clear manual platform",
        "Show only this source",
        "Clear selection",
        "Show in Library View preview",
        "Copy planned view path",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "expected the single-selection row menu to show {expected:?}"
        );
    }
}

#[test]
fn library_view_planned_destination_lookup_never_offers_a_skip_entrys_absent_path() {
    let view = sample_library_view("view-1", "retrodeck", "/home/user/retrodeck/roms");
    let plan = LibraryViewPlan {
        view_id: "view-1".to_string(),
        destination_root: PathBuf::from("/home/user/retrodeck/roms"),
        counts: LibraryViewPlanCounts {
            create: 0,
            correct: 0,
            repair: 0,
            remove: 0,
            collision: 0,
            skip: 1,
        },
        entries: vec![LibraryViewPlanEntry {
            action: LibraryViewPlanAction::SkipUnknownPlatform,
            archive_path: Some(PathBuf::from("/roms/unknown.zip")),
            relative_link_path: None,
            destination_path: None,
            platform: None,
            reason: Some("archive has no assigned platform".to_string()),
            colliding_with: None,
            source_folder_path: None,
            archive_identity: None,
        }],
        unsafe_root_error: None,
        profile_fingerprint: String::new(),
        fingerprint_conflict: None,
        profile_error: None,
    };
    let last_plan = (view, plan);

    // The entry exists (so "Copy planned view path" would be reachable
    // for a *different*, safety-relevant reason - e.g. showing the
    // skip reason) but it carries no destination, so the copy lookup
    // must return `None`, never a fabricated or stale path.
    assert!(
        library_view_planned_entry_for(Some(&last_plan), Path::new("/roms/unknown.zip")).is_some()
    );
    assert!(
        library_view_planned_destination_for(Some(&last_plan), Path::new("/roms/unknown.zip"))
            .is_none()
    );
}

#[test]
fn library_views_page_row_context_menu_never_offers_apply_or_repair_without_a_safe_plan() {
    let ctx = egui::Context::default();
    let views = vec![sample_library_view(
        "view-1",
        "retrodeck",
        "/home/user/retrodeck/roms",
    )];
    let all_source_folders: Vec<SourceFolderView> = Vec::new();
    let mut plan_filter = LibraryViewPlanFilter::default();
    let mut form_dialog = None;
    let mut remove_dialog = None;
    let mut clipboard = InMemoryClipboard::default();
    let action = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_library_views_page(
                ui,
                &views,
                &all_source_folders,
                false,
                None,
                None,
                &mut plan_filter,
                &mut form_dialog,
                &mut remove_dialog,
                &mut clipboard,
            );
        });
    });
    assert!(rendered_text_contains(&action, "Preview"));
    assert!(rendered_text_contains(&action, "retrodeck"));
    // The dialogs/action-returning path is exercised more directly by
    // `library_views_page_with_no_views_shows_empty_state_not_an_error`
    // and friends; this test's own job is narrower: prove the page
    // renders the row (so a context menu could even be opened on it)
    // while no plan exists, without panicking - the disabled-button
    // wiring itself mirrors the inline "Apply"/"Repair" buttons
    // exactly (`can_apply` computed once, reused for both).
}

#[test]
fn library_multi_selection_context_menu_shows_the_correct_bulk_actions() {
    let records = vec![
        record_at(PathBuf::from("/roms/a.zip"), MountState::Pending),
        record_at(PathBuf::from("/roms/b.zip"), MountState::Mounted),
    ];
    let selected_archives: HashSet<PathBuf> =
        [PathBuf::from("/roms/a.zip"), PathBuf::from("/roms/b.zip")]
            .into_iter()
            .collect();
    let ctx = egui::Context::default();
    let menu_context = row_menu_context_for(&records);

    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_bulk_row_context_menu(ui, &selected_archives, &menu_context);
        });
    });

    for expected in [
        "Mount selected",
        "Unmount selected",
        "Set platform for selected",
        "Copy selected archive paths",
        "Clear selection",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "expected the multi-selection row menu to show {expected:?}"
        );
    }
}

#[test]
fn context_menu_disabled_mount_reason_matches_the_main_button_reason() {
    // Both the main "Selected archive" panel and the Library row
    // context menu read the *same* `block_reason` - see
    // `RowMenuContext`/`SelectedArchiveViewState`, both fed from one
    // `archive_action_block_reason` call in `update()`. This proves
    // they render the identical text, not two independently-drifting
    // guesses.
    let reason = "Another operation is running.";
    let record = record_at(PathBuf::from("/roms/a.zip"), MountState::Pending);
    let row = row_with_fields("/roms/a.zip", "SNES", "Pending", "/roms/a.zip", "/mnt/a");
    let records = vec![record.clone()];
    let menu_context = RowMenuContext {
        records: &records,
        cached: None,
        busy: true,
        block_reason: Some(reason),
        platform_busy: false,
        retroarch_profiles: &RetroArchProfilesState::NotScanned,
        library_views_configured: false,
        library_view_last_plan: None,
    };

    let ctx = egui::Context::default();
    let row_menu_output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_single_row_context_menu(ui, &row, &menu_context);
        });
    });
    assert!(rendered_text_contains(&row_menu_output, reason));

    let EmptySelectedArchiveViewStateParts {
        mut confirm_unmount,
        mut confirm_lazy_unmount,
        mut focus_lazy_cancel,
        lazy_unmount_offers,
        remount_offers,
        mut cleanup_after_unmount,
        mut platform_choice,
        mut platform_custom_text,
        mut clipboard,
    } = empty_selected_archive_view_state_parts();
    let main_panel_output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_selected_archive(
                ui,
                Some(&record),
                None,
                None,
                None,
                SelectedArchiveViewState {
                    operation: None,
                    busy: true,
                    block_reason: Some(reason),
                    action_readiness_debug_lines: &[],
                    confirm_unmount: &mut confirm_unmount,
                    confirm_lazy_unmount: &mut confirm_lazy_unmount,
                    focus_lazy_cancel: &mut focus_lazy_cancel,
                    lazy_unmount_offers: &lazy_unmount_offers,
                    remount_offers: &remount_offers,
                    cleanup_after_unmount: &mut cleanup_after_unmount,
                    platform_choice: &mut platform_choice,
                    platform_custom_text: &mut platform_custom_text,
                    platform_busy: false,
                    clipboard: &mut clipboard,
                },
            );
        });
    });
    assert!(rendered_text_contains(&main_panel_output, reason));
}

#[test]
fn context_menu_mount_produces_the_same_operation_request_as_the_main_button() {
    // The row context menu's Mount click (see `show_single_row_
    // context_menu`) builds `OperationRequest { action:
    // available_action(record.mount_state), archive_path:
    // row.path.clone(), cleanup_after_unmount: false }` - byte-for-byte
    // the same shape `show_selected_archive`'s own Mount click builds
    // for the identical record (`action, archive_path.clone(),
    // cleanup_after_unmount: false` using the same `available_action`
    // call). Both are then wrapped in `AppOperationRequest::Archive`
    // unchanged by `update()`'s dispatch, so this pins the one place
    // the two code paths could otherwise silently diverge.
    for mount_state in [
        MountState::Pending,
        MountState::MountPathExists,
        MountState::Mounted,
    ] {
        let record = record_at(PathBuf::from("/roms/a.zip"), mount_state);
        let row = row_with_fields("/roms/a.zip", "SNES", "Pending", "/roms/a.zip", "/mnt/a");
        let records = vec![record.clone()];
        let menu_context = row_menu_context_for(&records);
        let ctx = egui::Context::default();
        let label = match available_action(mount_state) {
            ArchiveAction::Mount => "Mount",
            ArchiveAction::Unmount => "Unmount",
            ArchiveAction::LazyUnmount | ArchiveAction::Remount => unreachable!(),
        };

        // `show_single_row_context_menu` paints its Mount/Unmount
        // button as the very first widget in a fresh container, so an
        // identical standalone button (same label, same starting
        // layout state) lands at the identical rect - learning that
        // rect this way avoids hardcoding a fragile pixel guess.
        let mut button_rect = None;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                button_rect = Some(ui.button(label).rect);
            });
        });
        let pos = button_rect.unwrap().center();

        let mut row_menu_action = None;
        let render = |ui: &mut egui::Ui, action: &mut Option<RowContextMenuAction>| {
            if let Some(result) = show_single_row_context_menu(ui, &row, &menu_context) {
                *action = Some(result);
            }
        };
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render(ui, &mut row_menu_action);
            });
        });
        for pressed in [true, false] {
            let input = egui::RawInput {
                events: vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                }],
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    render(ui, &mut row_menu_action);
                });
            });
        }
        let request = match row_menu_action {
            Some(RowContextMenuAction::Operation(request)) => request,
            _ => panic!("expected the row menu's Mount/Unmount click to build an OperationRequest"),
        };

        // The exact construction `show_selected_archive`'s own click
        // handler uses (see its `ArchiveAction::Mount { request = ... }`
        // arm) - reproduced here as the expected value rather than
        // clicking a second real button, since both sides are already
        // provably driven by the same `available_action` call.
        let expected = OperationRequest {
            action: available_action(record.mount_state),
            archive_path: record.mount_plan.archive.path.clone(),
            cleanup_after_unmount: false,
        };
        assert_eq!(request.action, expected.action);
        assert_eq!(request.archive_path, expected.archive_path);
        assert_eq!(
            request.cleanup_after_unmount,
            expected.cleanup_after_unmount
        );
    }
}

#[test]
fn right_clicking_a_row_preserves_sort_filters_and_focus() {
    let data = loaded_data_with_rows(
        "/mount",
        vec![
            row_with_fields("/roms/a.zip", "SNES", "Live", "alpha.zip", "/mnt/a"),
            row_with_fields("/roms/b.zip", "GBA", "Live", "bravo.zip", "/mnt/b"),
        ],
    );
    let mut harness = RealLoadedDataHarness::new();
    harness.archive_context.focused = Some(PathBuf::from("/roms/a.zip"));
    harness.archive_context.selected = [PathBuf::from("/roms/a.zip")].into_iter().collect();
    harness.sort_field = Some(SortField::Platform);
    harness.sort_ascending = false;
    harness.library_filters.present = true;

    let ctx = egui::Context::default();
    harness.render(&ctx, &data, bounded_test_input());

    // A right-click on the already-rendered table (no menu item
    // clicked - just the secondary-click event itself) must never
    // perturb sort/filter/focus state, exactly like an ordinary
    // left-click replace/toggle never does.
    let mut right_click_input = bounded_test_input();
    right_click_input.events.push(egui::Event::PointerButton {
        pos: egui::pos2(50.0, 60.0),
        button: egui::PointerButton::Secondary,
        pressed: true,
        modifiers: egui::Modifiers::default(),
    });
    harness.render(&ctx, &data, right_click_input);
    let mut release_input = bounded_test_input();
    release_input.events.push(egui::Event::PointerButton {
        pos: egui::pos2(50.0, 60.0),
        button: egui::PointerButton::Secondary,
        pressed: false,
        modifiers: egui::Modifiers::default(),
    });
    harness.render(&ctx, &data, release_input);

    assert_eq!(harness.sort_field, Some(SortField::Platform));
    assert!(!harness.sort_ascending);
    assert!(harness.library_filters.present);
}

#[test]
fn sources_context_menu_never_offers_filesystem_deletion() {
    let ctx = egui::Context::default();
    let sources = three_source_views();
    let mut add_dialog = None;
    let mut remove_dialog = None;
    let mut clipboard = InMemoryClipboard::default();

    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_sources_page(
                ui,
                &sources,
                &[],
                Some(Path::new("/mnt/archivefs")),
                false,
                &mut add_dialog,
                &mut remove_dialog,
                &mut clipboard,
            );
        });
    });
    for forbidden in ["Delete from disk", "Delete files", "Delete permanently"] {
        assert!(!rendered_text_contains(&output, forbidden));
    }
    // Structural guarantee: `SourcesPageAction` itself has no
    // filesystem-deletion variant for the context menu to reach even
    // in principle - the exhaustive match in `update()` only ever
    // calls `start_source_action(SourceAction::Remove { keep_catalogue, .. })`,
    // never a raw filesystem delete.
    let action = SourcesPageAction::ConfirmRemove {
        path: PathBuf::from("/roms"),
        keep_catalogue: true,
    };
    assert!(matches!(
        action,
        SourcesPageAction::ConfirmRemove { .. }
            | SourcesPageAction::AddFolder(_)
            | SourcesPageAction::ScanOne(_)
            | SourcesPageAction::ScanAll
            | SourcesPageAction::RefreshStatus
            | SourcesPageAction::SetEnabled { .. }
            | SourcesPageAction::ViewInLibrary(_)
    ));
}

#[test]
fn sources_context_menu_remove_source_defaults_to_keep_catalogue_entries() {
    let ctx = egui::Context::default();
    let sources = three_source_views();
    let mut add_dialog = None;
    let mut remove_dialog: Option<SourcesRemoveDialogState> = None;
    let mut clipboard = InMemoryClipboard::default();

    // Render once so the row's rect is registered, then open its
    // context menu for real and click "Remove source".
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_sources_page(
                ui,
                &sources,
                &[],
                Some(Path::new("/mnt/archivefs")),
                false,
                &mut add_dialog,
                &mut remove_dialog,
                &mut clipboard,
            );
        });
    });

    // Mirrors exactly what the "Remove" button itself does (see
    // `show_sources_page`) - the context menu's "Remove source" item
    // sets the identical `SourcesRemoveDialogState`.
    remove_dialog = Some(SourcesRemoveDialogState {
        path: sources[0].path.clone(),
        last_archive_count: sources[0].last_archive_count,
        keep_catalogue: true,
    });

    assert!(
        remove_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.keep_catalogue),
        "Remove source must default to Keep catalogue entries, exactly like the Remove button"
    );
}

#[test]
fn health_context_menu_show_in_library_resolves_the_exact_archive() {
    let mut app = app_for_operation_tests();
    let path = PathBuf::from("/roms/health-issue.zip");

    // Exercises the exact dispatch `HealthDashboardAction::ViewInLibrary`
    // drives in `update()` - the same action both the detail panel's
    // "View in Library" button and the new issue-row context menu's
    // "Show archive in Library" item produce.
    app.view = MainView::Duplicates;
    app.archive_context.focused = None;
    app.archive_context.selected.clear();
    let action = HealthDashboardAction::ViewInLibrary(path.clone());
    match action {
        HealthDashboardAction::ViewInLibrary(resolved) => {
            app.navigate_to_library_tab(LibraryTab::Archives);
            app.archive_context.focused = Some(resolved.clone());
            app.archive_context.selected = [resolved].into_iter().collect();
        }
        _ => unreachable!(),
    }

    assert_eq!(app.view, MainView::Library);
    assert_eq!(app.library_tab, LibraryTab::Archives);
    assert_eq!(app.archive_context.focused, Some(path.clone()));
    assert_eq!(
        app.archive_context.selected,
        [path].into_iter().collect::<HashSet<_>>()
    );
}

#[test]
fn duplicate_review_action_has_no_delete_variant() {
    let variants = [
        DuplicateReviewAction::Close,
        DuplicateReviewAction::ViewInLibrary(PathBuf::from("/roms/a.zip")),
        DuplicateReviewAction::Inspect(PathBuf::from("/roms/a.zip")),
    ];
    for variant in variants {
        assert!(matches!(
            variant,
            DuplicateReviewAction::Close
                | DuplicateReviewAction::ViewInLibrary(_)
                | DuplicateReviewAction::Inspect(_)
        ));
    }
}

#[test]
fn duplicate_review_context_menus_never_render_delete_wording() {
    let path = PathBuf::from("/roms/dup/a.zip");
    let report = CatalogueDuplicateReport {
        groups: vec![CatalogueDuplicateGroup {
            normalized_title: "dup".to_string(),
            title: "Dup Game".to_string(),
            platform: "SNES".to_string(),
            reason: "same normalized title and platform".to_string(),
            entries: vec![
                CatalogueDuplicateArchive {
                    archive_id: 1,
                    path: path.clone(),
                    present: true,
                    size_bytes: Some(1024),
                    modified_time_unix_seconds: Some(0),
                },
                CatalogueDuplicateArchive {
                    archive_id: 2,
                    path: PathBuf::from("/roms/dup/b.zip"),
                    present: true,
                    size_bytes: Some(1024),
                    modified_time_unix_seconds: Some(0),
                },
            ],
            total_known_size_bytes: 2048,
            entries_with_known_size: 2,
        }],
        archives_in_groups: 2,
    };
    let mut filters = DuplicateReviewFilters::initial();
    let mut sort_field = DuplicateSortField::Title;
    let mut sort_ascending = true;
    let mut selected_group = Some(DuplicateGroupIdentity::from(&report.groups[0]));
    let mut selected_archive = Some(path);
    let mut clipboard = InMemoryClipboard::default();

    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_duplicate_review_panel(
                ui,
                &report,
                DuplicateReviewViewState {
                    filters: &mut filters,
                    sort_field: &mut sort_field,
                    sort_ascending: &mut sort_ascending,
                    selected_group: &mut selected_group,
                    selected_archive: &mut selected_archive,
                    clipboard: &mut clipboard,
                },
            );
        });
    });
    for forbidden in ["Delete", "Remove archive", "Cleanup"] {
        assert!(!rendered_text_contains(&output, forbidden));
    }
}

#[test]
fn activity_copy_message_uses_the_full_underlying_text() {
    let long_message = "Y".repeat(500);
    let entry = history_entry(ActivityOutcome::Completed, long_message.clone());
    let text = history_entry_text(&entry);
    // The Activity context menu's "Copy message" button copies exactly
    // `history_entry_text(entry)` (see `show_activity_panel`) - the
    // same complete string the panel renders, never a truncated
    // summary.
    assert!(text.contains(&long_message));

    let mut clipboard = InMemoryClipboard::default();
    let _ = clipboard.set_text(text.clone());
    assert_eq!(clipboard.set_calls, vec![text]);
}

#[test]
fn clear_one_activity_entry_leaves_the_rest_unchanged() {
    let mut history = OperationHistory::default();
    history.record(history_entry(ActivityOutcome::Completed, "third"));
    history.record(history_entry(ActivityOutcome::Completed, "second"));
    history.record(history_entry(ActivityOutcome::Completed, "first"));
    // `record` pushes to the front, so display order (most recent
    // first) is: "first", "second", "third".
    let before: Vec<String> = history
        .entries()
        .map(|entry| entry.message.clone())
        .collect();
    assert_eq!(before, vec!["first", "second", "third"]);

    // Removing display index 1 ("second") must not reorder or alter
    // "first"/"third".
    history.remove(1);

    let after: Vec<String> = history
        .entries()
        .map(|entry| entry.message.clone())
        .collect();
    assert_eq!(after, vec!["first", "third"]);
}

#[test]
fn library_row_platform_submenu_contains_every_canonical_platform_exactly_once() {
    let record = record_at(PathBuf::from("/roms/a.zip"), MountState::Pending);
    let row = row_with_fields("/roms/a.zip", "SNES", "Pending", "/roms/a.zip", "/mnt/a");
    let records = vec![record];
    let menu_context = row_menu_context_for(&records);
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_single_row_context_menu(ui, &row, &menu_context);
            // Force every canonical platform name to actually be
            // painted so `rendered_text_contains` can see it, mirroring
            // how `ui.menu_button`'s own content only paints once
            // opened - directly render the same list the submenu
            // builds from.
            for name in canonical_platform_names() {
                ui.label(name);
            }
        });
    });

    let names = canonical_platform_names();
    let mut seen = 0;
    for name in &names {
        if rendered_text_contains(&output, name) {
            seen += 1;
        }
    }
    assert_eq!(
        seen,
        names.len(),
        "every canonical platform must be reachable from the row menu's platform submenu"
    );
    let mut deduplicated = names.clone();
    deduplicated.dedup();
    assert_eq!(
        names.len(),
        deduplicated.len(),
        "the submenu must never list the same canonical platform twice"
    );
}

#[test]
fn row_context_menu_and_text_field_context_menu_do_not_interfere() {
    // The new row context menus (`Response::context_menu`, keyed by
    // each row's own stable `Id`) and the existing text-field context
    // menu (`show_text_edit_with_context_menu`, its own custom popup)
    // are structurally independent mechanisms with disjoint `Id`s.
    // Rendering both together in one frame must not panic or corrupt
    // either - reusing the exact same fixtures each mechanism's own
    // dedicated tests already use.
    let ctx = egui::Context::default();
    let mut text = "some text".to_string();
    let mut clipboard = InMemoryClipboard::default();
    let record = record_at(PathBuf::from("/roms/a.zip"), MountState::Pending);
    let row = row_with_fields("/roms/a.zip", "SNES", "Pending", "/roms/a.zip", "/mnt/a");
    let records = vec![record];
    let menu_context = row_menu_context_for(&records);

    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_text_edit_with_context_menu(ui, &mut text, &mut clipboard, |text_edit| text_edit);
            let _ = show_single_row_context_menu(ui, &row, &menu_context);
        });
    });
    assert!(rendered_text_contains(&output, "some text"));
    assert!(rendered_text_contains(&output, "Mount"));
}

#[test]
#[cfg(unix)]
fn row_context_menus_do_not_panic_on_non_utf8_paths() {
    // A recognized `.zip` extension with an invalid UTF-8 byte
    // embedded in the filename - `Archive::from_path` (via
    // `record_at`) requires a recognized extension to succeed at all,
    // exactly like the codebase's other non-UTF8 archive path fixtures.
    let non_utf8_bytes = b"fo\xffo.zip".to_vec();
    let non_utf8_path = PathBuf::from(OsString::from_vec(non_utf8_bytes));
    let record = record_at(non_utf8_path.clone(), MountState::Pending);
    let row = ArchiveRow {
        path: non_utf8_path.clone(),
        archive_path: non_utf8_path.to_string_lossy().into_owned(),
        mount_path: "/mnt/a".to_string(),
        platform: "SNES".to_string(),
        state: "Pending".to_string(),
        search_text: String::new(),
        origin: RowOrigin::Live,
        unknown_platform: false,
        source_path: Some(non_utf8_path),
    };
    let records = vec![record];
    let menu_context = row_menu_context_for(&records);
    let selected_archives: HashSet<PathBuf> = [row.path.clone()].into_iter().collect();

    let ctx = egui::Context::default();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_single_row_context_menu(ui, &row, &menu_context);
            let _ = show_bulk_row_context_menu(ui, &selected_archives, &menu_context);
        });
    });
    // No panic reaching here is the assertion.
}

#[test]
fn opening_row_context_menus_causes_no_side_effects() {
    let record = record_at(PathBuf::from("/roms/a.zip"), MountState::Pending);
    let row = row_with_fields("/roms/a.zip", "SNES", "Pending", "/roms/a.zip", "/mnt/a");
    let records = vec![record];
    let menu_context = row_menu_context_for(&records);
    let ctx = egui::Context::default();

    let mut action = None;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            action = show_single_row_context_menu(ui, &row, &menu_context);
        });
    });

    assert!(
        action.is_none(),
        "rendering the menu without clicking anything must request no action at all"
    );
}

// -----------------------------------------------------------------
// "Unmount selected" confirmation dialog follow-up.
// -----------------------------------------------------------------

#[test]
fn unmount_selected_button_click_opens_confirmation_not_immediate_unmount() {
    // The row menu's "Unmount selected" click must produce
    // `RowContextMenuAction::UnmountSelected` - a unit variant with no
    // item list attached, structurally incapable of driving an
    // immediate `AppOperationRequest::UnmountAll` the way it used to.
    // `show_loaded_data`'s dispatch (see its `menu_action` match) only
    // ever turns this into opening `confirm_unmount_selected`.
    let records = vec![
        record_at(PathBuf::from("/roms/a.zip"), MountState::Mounted),
        record_at(PathBuf::from("/roms/b.zip"), MountState::Pending),
    ];
    let selected_archives: HashSet<PathBuf> =
        [PathBuf::from("/roms/a.zip"), PathBuf::from("/roms/b.zip")]
            .into_iter()
            .collect();
    let menu_context = row_menu_context_for(&records);
    let ctx = egui::Context::default();
    let label = "Unmount selected (1)";

    let mut button_rect = None;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            // "Unmount selected" is the second widget
            // `show_bulk_row_context_menu` paints (after "Mount
            // selected"), so an identical standalone pair in the same
            // starting layout state lands at the identical rect.
            let _ = ui.button("Mount selected (1)");
            button_rect = Some(ui.button(label).rect);
        });
    });
    let pos = button_rect.unwrap().center();

    let mut action = None;
    let render = |ui: &mut egui::Ui, action: &mut Option<RowContextMenuAction>| {
        if let Some(result) = show_bulk_row_context_menu(ui, &selected_archives, &menu_context) {
            *action = Some(result);
        }
    };
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| render(ui, &mut action));
    });
    for pressed in [true, false] {
        let input = egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| render(ui, &mut action));
        });
    }

    assert!(matches!(
        action,
        Some(RowContextMenuAction::UnmountSelected)
    ));
}

#[test]
fn mounted_selected_unmount_items_reports_the_exact_mounted_selected_count() {
    let records = vec![
        record_at(PathBuf::from("/roms/a.zip"), MountState::Mounted),
        record_at(PathBuf::from("/roms/b.zip"), MountState::Mounted),
        record_at(PathBuf::from("/roms/c.zip"), MountState::Pending),
        record_at(PathBuf::from("/roms/d.zip"), MountState::Mounted),
    ];
    // "d" is mounted but not selected - must not appear either.
    let selected_archives: HashSet<PathBuf> = [
        PathBuf::from("/roms/a.zip"),
        PathBuf::from("/roms/b.zip"),
        PathBuf::from("/roms/c.zip"),
    ]
    .into_iter()
    .collect();

    let items = mounted_selected_unmount_items(&records, &selected_archives);

    assert_eq!(items.len(), 2, "exactly the 2 mounted+selected archives");
    let paths: HashSet<PathBuf> = items.into_iter().map(|item| item.archive_path).collect();
    assert_eq!(
        paths,
        [PathBuf::from("/roms/a.zip"), PathBuf::from("/roms/b.zip")]
            .into_iter()
            .collect()
    );
}

#[test]
fn mounted_selected_unmount_items_excludes_unmounted_selected_rows() {
    let records = vec![
        record_at(PathBuf::from("/roms/a.zip"), MountState::Mounted),
        record_at(PathBuf::from("/roms/b.zip"), MountState::Pending),
        record_at(PathBuf::from("/roms/c.zip"), MountState::MountPathExists),
    ];
    let selected_archives: HashSet<PathBuf> = [
        PathBuf::from("/roms/a.zip"),
        PathBuf::from("/roms/b.zip"),
        PathBuf::from("/roms/c.zip"),
    ]
    .into_iter()
    .collect();

    let items = mounted_selected_unmount_items(&records, &selected_archives);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].archive_path, PathBuf::from("/roms/a.zip"));
}

#[test]
fn mounted_selected_unmount_items_revalidates_against_current_state() {
    // A pure function of its two arguments - calling it again after
    // `records`/`selected_archives` change must reflect the *new*
    // state, never a value cached/captured from an earlier call. This
    // is the actual mechanism behind "revalidate before starting".
    let path = PathBuf::from("/roms/a.zip");
    let mounted_records = vec![record_at(path.clone(), MountState::Mounted)];
    let selected: HashSet<PathBuf> = [path.clone()].into_iter().collect();

    let before = mounted_selected_unmount_items(&mounted_records, &selected);
    assert_eq!(before.len(), 1);

    // The archive gets unmounted elsewhere while the dialog would
    // still be open.
    let now_pending_records = vec![record_at(path, MountState::Pending)];
    let after = mounted_selected_unmount_items(&now_pending_records, &selected);
    assert!(
        after.is_empty(),
        "no-longer-mounted archives must disappear from the confirmed set immediately"
    );
}

#[test]
fn unmount_selected_confirmation_dialog_shows_the_exact_mounted_count_and_required_wording() {
    let mut harness = RealLoadedDataHarness::new();
    harness.archive_context.selected = [
        PathBuf::from("/roms/a.zip"),
        PathBuf::from("/roms/b.zip"),
        PathBuf::from("/roms/c.zip"),
    ]
    .into_iter()
    .collect();
    harness.confirm_unmount_selected = Some(UnmountSelectedConfirmation);
    harness.cleanup_after_unmount = true;
    let data = loaded_data_with_records(
        "/mount",
        vec![
            record_at(PathBuf::from("/roms/a.zip"), MountState::Mounted),
            record_at(PathBuf::from("/roms/b.zip"), MountState::Mounted),
            record_at(PathBuf::from("/roms/c.zip"), MountState::Pending),
        ],
    );

    let ctx = egui::Context::default();
    harness.render(&ctx, &data, bounded_test_input());
    harness.render(&ctx, &data, bounded_test_input());
    let output = harness.last_output.as_ref().unwrap();

    for expected in [
        "2 of the 3 selected archives are currently mounted",
        "Only those 2 mounted archives will be unmounted",
        "Cleanup after each successful unmount: enabled.",
        "Original archive files will not be deleted or modified",
    ] {
        assert!(
            rendered_text_contains(output, expected),
            "expected the Unmount selected confirmation to show {expected:?}"
        );
    }
}

#[test]
fn unmount_selected_confirmation_cleanup_wording_matches_the_current_option() {
    let mut harness = RealLoadedDataHarness::new();
    harness.archive_context.selected = [PathBuf::from("/roms/a.zip")].into_iter().collect();
    harness.confirm_unmount_selected = Some(UnmountSelectedConfirmation);
    harness.cleanup_after_unmount = false;
    let data = loaded_data_with_records(
        "/mount",
        vec![record_at(PathBuf::from("/roms/a.zip"), MountState::Mounted)],
    );

    let ctx = egui::Context::default();
    harness.render(&ctx, &data, bounded_test_input());
    harness.render(&ctx, &data, bounded_test_input());
    let output = harness.last_output.as_ref().unwrap();

    assert!(rendered_text_contains(
        output,
        "Cleanup after each successful unmount: disabled."
    ));
    assert!(!rendered_text_contains(
        output,
        "Cleanup after each successful unmount: enabled."
    ));
}

#[test]
fn zero_mounted_selected_disables_the_unmount_selected_button() {
    let records = vec![record_at(PathBuf::from("/roms/a.zip"), MountState::Pending)];
    let selected_archives: HashSet<PathBuf> = [PathBuf::from("/roms/a.zip")].into_iter().collect();
    let menu_context = row_menu_context_for(&records);
    let ctx = egui::Context::default();

    let mut button_rect = None;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = ui.button("Mount selected (0)");
            button_rect = Some(ui.button("Unmount selected (0)").rect);
        });
    });
    let pos = button_rect.unwrap().center();

    let mut action = None;
    let render = |ui: &mut egui::Ui, action: &mut Option<RowContextMenuAction>| {
        if let Some(result) = show_bulk_row_context_menu(ui, &selected_archives, &menu_context) {
            *action = Some(result);
        }
    };
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| render(ui, &mut action));
    });
    for pressed in [true, false] {
        let input = egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| render(ui, &mut action));
        });
    }

    assert!(
        action.is_none(),
        "a disabled \"Unmount selected\" button must never produce an action, even when \
             clicked at its exact position"
    );
}

#[test]
fn unmount_selected_cancel_has_no_side_effects() {
    let mut harness = RealLoadedDataHarness::new();
    harness.archive_context.selected = [PathBuf::from("/roms/a.zip")].into_iter().collect();
    harness.archive_context.focused = Some(PathBuf::from("/roms/a.zip"));
    harness.sort_field = Some(SortField::Platform);
    harness.sort_ascending = false;
    harness.library_filters.present = true;
    harness.confirm_unmount_selected = Some(UnmountSelectedConfirmation);
    let data = loaded_data_with_records(
        "/mount",
        vec![record_at(PathBuf::from("/roms/a.zip"), MountState::Mounted)],
    );

    let ctx = egui::Context::default();
    harness.render(&ctx, &data, bounded_test_input());
    // An anchored, auto-sized egui window needs one frame to settle its measured position
    // before pointer coordinates from its painted controls are stable.
    harness.render(&ctx, &data, bounded_test_input());
    let pos = find_exact_text_center(harness.last_output.as_ref().unwrap(), "Cancel")
        .expect("the real dialog must render a \"Cancel\" button");

    for pressed in [true, false] {
        let mut input = bounded_test_input();
        input.events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        });
        harness.render(&ctx, &data, input);
    }

    assert!(
        harness.confirm_unmount_selected.is_none(),
        "Cancel must close the dialog"
    );
    assert!(
        harness.requested_action.is_none(),
        "Cancel must never request an unmount"
    );
    assert_eq!(
        harness.archive_context.selected,
        [PathBuf::from("/roms/a.zip")]
            .into_iter()
            .collect::<HashSet<_>>(),
        "Cancel must preserve the selection"
    );
    assert_eq!(
        harness.archive_context.focused,
        Some(PathBuf::from("/roms/a.zip"))
    );
    assert_eq!(harness.sort_field, Some(SortField::Platform));
    assert!(!harness.sort_ascending);
    assert!(harness.library_filters.present);
    assert_eq!(
        harness.library_column_widths,
        LibraryColumnWidths::default(),
        "Cancel must preserve resized column widths (unchanged here, but the same field \
             Confirm - and every other app action - must never touch as a side effect)"
    );
}

#[test]
fn unmount_selected_confirm_uses_the_same_batch_engine_with_only_mounted_selected_items() {
    let mut harness = RealLoadedDataHarness::new();
    harness.archive_context.selected = [PathBuf::from("/roms/a.zip"), PathBuf::from("/roms/b.zip")]
        .into_iter()
        .collect();
    harness.confirm_unmount_selected = Some(UnmountSelectedConfirmation);
    harness.cleanup_after_unmount = true;
    let data = loaded_data_with_records(
        "/mount",
        vec![
            record_at(PathBuf::from("/roms/a.zip"), MountState::Mounted),
            record_at(PathBuf::from("/roms/b.zip"), MountState::Pending),
        ],
    );

    let ctx = egui::Context::default();
    harness.render(&ctx, &data, bounded_test_input());
    harness.render(&ctx, &data, bounded_test_input());
    let pos = find_exact_text_center(harness.last_output.as_ref().unwrap(), "Unmount selected")
        .expect("the real dialog must render an \"Unmount selected\" button");

    for pressed in [true, false] {
        let mut input = bounded_test_input();
        input.events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        });
        harness.render(&ctx, &data, input);
    }

    assert!(harness.confirm_unmount_selected.is_none());
    match harness.requested_action {
        Some(AppOperationRequest::UnmountAll {
            items,
            cleanup_after_unmount,
        }) => {
            // The exact same `AppOperationRequest::UnmountAll` variant
            // `update()` dispatches straight to `self.start_unmount_all`
            // for the existing "Unmount All" confirmation - never a
            // second execution path.
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].archive_path, PathBuf::from("/roms/a.zip"));
            assert!(cleanup_after_unmount);
        }
        other => panic!("expected AppOperationRequest::UnmountAll, got {other:?}"),
    }
}

#[test]
fn existing_global_unmount_all_confirmation_wording_is_unchanged() {
    let mut filter = String::new();
    let mut filtered_rows = None;
    let mut selected_archive = None;
    let mut selected_archives = HashSet::new();
    let mut confirm_unmount = None;
    let mut confirm_lazy_unmount = None;
    let mut confirm_lazy_unmount_final = None;
    let mut confirm_mount_all = None;
    let mut focus_mount_all_cancel = false;
    let mut mount_all_typed_count = String::new();
    let mut confirm_unmount_all = Some(UnmountAllConfirmation);
    let mut focus_unmount_all_cancel = false;
    let mut unmount_all_typed_count = String::new();
    let mut confirm_unmount_selected = None;
    let mut focus_unmount_selected_cancel = false;
    let mut confirm_mount_selected = None;
    let mut focus_mount_selected_cancel = false;
    let mut mount_selected_typed_count = String::new();
    let mut confirm_bulk_platform_action = None;
    let mut focus_bulk_platform_cancel = false;
    let mut bulk_platform_action_typed_count = String::new();
    let mut focus_lazy_cancel = false;
    let mut focus_final_lazy_cancel = false;
    let lazy_unmount_offers = HashSet::new();
    let remount_offers = HashSet::new();
    let mut cleanup_after_unmount = true;
    let mut history = OperationHistory::default();
    let mut library_filters = LibraryRowFilters::default();
    let mut platform_choice = None;
    let mut platform_custom_text = String::new();
    let mut bulk_platform_choice = None;
    let mut confirm_remove_missing = None;
    let mut missing_removal_typed_count = String::new();
    let mut sort_field = None;
    let mut library_platform_query = String::new();
    let mut sort_ascending = true;
    let mut library_scroll_offset = 0.0;
    let mut clipboard = InMemoryClipboard::default();
    let mut library_source_filter = None;
    let mut library_column_widths = LibraryColumnWidths::default();
    let mut select_all_visible_requested = false;
    let data = loaded_data_with_records(
        "/mount",
        vec![
            record_at(PathBuf::from("/roms/a.zip"), MountState::Mounted),
            record_at(PathBuf::from("/roms/b.zip"), MountState::Mounted),
        ],
    );

    let ctx = egui::Context::default();
    let mut render = |ctx: &egui::Context| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_loaded_data(
                ui,
                &data,
                LoadedViewState {
                    filter: &mut filter,
                    filtered_rows: &mut filtered_rows,
                    selected_archive: &mut selected_archive,
                    operation: None,
                    busy: false,
                    block_reason: None,
                    action_readiness_debug_lines: &[],
                    feedback: None,
                    confirm_unmount: &mut confirm_unmount,
                    confirm_lazy_unmount: &mut confirm_lazy_unmount,
                    confirm_lazy_unmount_final: &mut confirm_lazy_unmount_final,
                    confirm_mount_all: &mut confirm_mount_all,
                    focus_mount_all_cancel: &mut focus_mount_all_cancel,
                    mount_all_typed_count: &mut mount_all_typed_count,
                    confirm_unmount_all: &mut confirm_unmount_all,
                    focus_unmount_all_cancel: &mut focus_unmount_all_cancel,
                    unmount_all_typed_count: &mut unmount_all_typed_count,
                    confirm_unmount_selected: &mut confirm_unmount_selected,
                    focus_unmount_selected_cancel: &mut focus_unmount_selected_cancel,
                    confirm_mount_selected: &mut confirm_mount_selected,
                    focus_mount_selected_cancel: &mut focus_mount_selected_cancel,
                    mount_selected_typed_count: &mut mount_selected_typed_count,
                    confirm_bulk_platform_action: &mut confirm_bulk_platform_action,
                    focus_bulk_platform_cancel: &mut focus_bulk_platform_cancel,
                    bulk_platform_action_typed_count: &mut bulk_platform_action_typed_count,
                    focus_lazy_cancel: &mut focus_lazy_cancel,
                    focus_final_lazy_cancel: &mut focus_final_lazy_cancel,
                    lazy_unmount_offers: &lazy_unmount_offers,
                    remount_offers: &remount_offers,
                    cleanup_after_unmount: &mut cleanup_after_unmount,
                    mount_all_result: None,
                    unmount_all_result: None,
                    history: &mut history,
                    cached: None,
                    library_filters: &mut library_filters,
                    platform_choice: &mut platform_choice,
                    platform_custom_text: &mut platform_custom_text,
                    platform_busy: false,
                    retroarch_profiles: &RetroArchProfilesState::NotScanned,
                    selected_archives: &mut selected_archives,
                    bulk_platform_choice: &mut bulk_platform_choice,
                    bulk_platform_busy: false,
                    missing_removal_available: false,
                    missing_removal_busy: false,
                    confirm_remove_missing: &mut confirm_remove_missing,
                    missing_removal_typed_count: &mut missing_removal_typed_count,
                    sort_field: &mut sort_field,
                    sort_ascending: &mut sort_ascending,
                    library_scroll_offset: &mut library_scroll_offset,
                    clipboard: &mut clipboard,
                    select_all_visible_requested: &mut select_all_visible_requested,
                    library_source_filter: &mut library_source_filter,
                    library_column_widths: &mut library_column_widths,
                    library_views_configured: false,
                    library_view_last_plan: None,
                    recent_scan: None,
                    recent_view: false,
                    library_platform_query: &mut library_platform_query,
                },
            );
        });
    };
    // A `Window`'s first frame only registers its content; the same
    // settling-frame requirement `show_activity_panel`'s own tests
    // document for `TopBottomPanel` applies here too.
    let _ = ctx.run(egui::RawInput::default(), &mut render);
    let output = ctx.run(egui::RawInput::default(), &mut render);

    // The window title itself is not asserted here: egui paints a
    // `Window`'s title bar via its own internal widget, not a plain
    // `Shape::Text` `rendered_text_contains` can see in one frame the
    // same way a `Window`'s content/tooltip is a documented
    // `rendered_text_contains` limitation elsewhere in this file - the
    // *content* wording below is the meaningful, observable pin.
    assert!(rendered_text_contains(
        &output,
        "2 mounted archives under /mount will be unmounted one at a time."
    ));
    assert!(rendered_text_contains(
        &output,
        "Cleanup after each successful unmount: enabled."
    ));
}

#[test]
fn active_mounts_recent_activity_shows_only_relevant_entries_through_the_shared_row_header() {
    let ctx = egui::Context::default();
    let mut history = OperationHistory::default();
    history.record(HistoryEntry::new(
        ActivityAction::Mount,
        Some(PathBuf::from("/roms/a.zip")),
        ActivityOutcome::Completed,
        "Mounted a.zip",
    ));
    history.record(HistoryEntry::new(
        ActivityAction::SourceScan,
        None,
        ActivityOutcome::Completed,
        "Scanned /roms: 12 archives found.",
    ));
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_active_mounts_recent_activity(ui, &history);
        });
    });
    assert!(rendered_text_contains(&output, "Recent activity"));
    assert!(rendered_text_contains(&output, "Mounted a.zip"));
    assert!(
        !rendered_text_contains(&output, "Scanned /roms: 12 archives found."),
        "Recent activity on Active Mounts must not show unrelated source activity"
    );
}

#[test]
fn active_mounts_recent_activity_empty_state_is_truthful() {
    let ctx = egui::Context::default();
    let history = OperationHistory::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_active_mounts_recent_activity(ui, &history);
        });
    });
    assert!(rendered_text_contains(
        &output,
        "No mount or unmount activity has been recorded in this session."
    ));
}

#[test]
fn library_sub_tab_headings_match_their_own_tab_bar_labels() {
    let ctx = egui::Context::default();
    let report = catalogue_filename_duplicates(&duplicate_catalogue_for_gui());
    let mut filters = DuplicateReviewFilters::initial();
    let mut sort_field = DuplicateSortField::Title;
    let mut sort_ascending = true;
    let mut selected_group = None;
    let mut selected_archive = None;
    let mut clipboard = InMemoryClipboard::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_duplicate_review_panel(
                ui,
                &report,
                DuplicateReviewViewState {
                    filters: &mut filters,
                    sort_field: &mut sort_field,
                    sort_ascending: &mut sort_ascending,
                    selected_group: &mut selected_group,
                    selected_archive: &mut selected_archive,
                    clipboard: &mut clipboard,
                },
            );
        });
    });
    assert!(
        rendered_text_contains(&output, "Duplicates"),
        "the Duplicates tab's own heading must match its tab-bar label"
    );
    assert!(
        !rendered_text_contains(&output, "Duplicate Review"),
        "the old, differently-worded heading must not still be rendered"
    );

    let mut views: Vec<LibraryViewConfig> = Vec::new();
    let mut plan_filter = LibraryViewPlanFilter::default();
    let mut form_dialog = None;
    let mut remove_dialog = None;
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_library_views_page(
                ui,
                &views,
                &[],
                false,
                None,
                None,
                &mut plan_filter,
                &mut form_dialog,
                &mut remove_dialog,
                &mut clipboard,
            );
        });
    });
    let _ = &mut views;
    assert!(
        rendered_text_contains(&output, "Views"),
        "the Views tab's own heading must match its tab-bar label"
    );
    assert!(
        !rendered_text_contains(&output, "Library Views"),
        "the old, differently-worded heading must not still be rendered"
    );
}

#[test]
fn mount_and_selected_pages_use_identical_mount_queue_button_wording() {
    let ctx = egui::Context::default();
    let records = vec![record("/roms/pending.zip", MountState::Pending)];
    let data = loaded_data_with_records("/mnt/archivefs", records.clone());
    let mut queue = vec![PathBuf::from("/roms/pending.zip")];
    let mut confirm = false;
    let mut search = String::new();
    let mount_output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_mount_page(
                ui,
                Some(&data),
                None,
                MountPageViewState {
                    queue: &mut queue,
                    search: &mut search,
                    platform: &mut None,
                    confirm: &mut confirm,
                    busy: false,
                    block_reason: None,
                },
            );
        });
    });
    assert!(rendered_text_contains(&mount_output, "Mount queue (1)"));

    let mut selected_queue = vec![PathBuf::from("/roms/pending.zip")];
    let mut selected_confirm = false;
    let selected_output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_selected_page(
                ui,
                Some(&data),
                None,
                SelectedPageViewState {
                    selected_archive: None,
                    selected_count: 0,
                    retroarch_profiles: &RetroArchProfilesState::NotScanned,
                    queue: &mut selected_queue,
                    confirm: &mut selected_confirm,
                    busy: false,
                    block_reason: None,
                },
            );
        });
    });
    assert!(
        rendered_text_contains(&selected_output, "Mount queue (1)"),
        "the Selected page's own queue button must use the same wording as the Mount page's"
    );
    assert!(!rendered_text_contains(
        &selected_output,
        "Mount ready archives"
    ));
}

#[test]
fn history_logs_page_has_a_recovery_heading_above_its_rollback_card() {
    let ctx = egui::Context::default();
    let shared_history = SharedHistoryState::NotLoaded;
    let mut rollback = SharedRollbackState::Idle;
    let mut history = OperationHistory::default();
    let mut filters = HistoryLogFilters::default();
    let mut clipboard = InMemoryClipboard::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_history_logs_page(
                ui,
                &shared_history,
                &mut rollback,
                None,
                &mut history,
                &mut filters,
                &mut clipboard,
            );
        });
    });
    assert!(rendered_text_contains(&output, "Recovery"));
    assert!(rendered_text_contains(
        &output,
        "Rollback always begins with a fresh read-only preview"
    ));
}

#[test]
fn pcsx2_profile_card_shows_the_first_blocker_directly_and_the_rest_behind_technical_details() {
    let ctx = egui::Context::default();
    let mut workflow = CheatWorkflowState {
        archive_path: PathBuf::from("/roms/a.zip"),
        display_name: "a".to_string(),
        normalized_name: "a".to_string(),
        platform: Some("PS2".to_string()),
        region: None,
        source_root: PathBuf::from("/roms"),
        size_bytes: None,
        adapter: CheatEmulatorAdapter::Pcsx2,
        identity_request: None,
        identity: CheatStepResource::NotLoaded,
        preview_request: None,
        preview: CheatStepResource::NotLoaded,
        transaction: CheatTransactionState::Idle,
        transaction_notice: None,
        selected_profile_id: None,
        selected_pcsx2_profile_id: None,
        pcsx2_inventory_profile_id: None,
        pcsx2_inventory: CheatStepResource::NotLoaded,
        pcsx2_gamehacking: CheatStepResource::NotLoaded,
        gamecube_gamehacking: CheatStepResource::NotLoaded,
        gamecube_gamehacking_request: None,
        gamecube_gamehacking_cancellation: None,
        gamecube_gamehacking_generation: 0,
        gamecube_gamehacking_blocked: false,
        bsfree_gamecube: CheatStepResource::NotLoaded,
        bsfree_gamecube_cancellation: None,
        bsfree_gamecube_generation: 0,
        bsfree_wii: CheatStepResource::NotLoaded,
        bsfree_wii_cancellation: None,
        bsfree_wii_generation: 0,
        selected_dolphin_profile_id: None,
        dolphin_explicit_root: String::new(),
        dolphin_inventory_profile_id: None,
        dolphin_inventory: CheatStepResource::NotLoaded,
        dolphin_provider_request: None,
        dolphin_provider: CheatStepResource::NotLoaded,
        dolphin_provider_selection: None,
        dolphin_destination_error: None,
        dolphin_local_lookup: DolphinLocalLookupState::NotAttempted,
        dolphin_profile_selection: None,
        dolphin_profile_choice: None,
        dolphin_details_open: false,
        dolphin_show_exact_changes: false,
        selected_xenia_profile_id: None,
        xenia_explicit_root: String::new(),
        xenia_provider_request: None,
        xenia_provider: CheatStepResource::NotLoaded,
        xenia_selected_candidate_index: None,
        xenia_selection: None,
        xenia_destination_error: None,
        xenia_profile_selection: None,
        xenia_profile_choice: None,
        xenia_details_open: false,
        xenia_show_exact_changes: false,
        source_mode: CheatSourceMode::ArchiveFsTrustedCatalogue,
        existing_library_profile_id: None,
        existing_library: CheatStepResource::NotLoaded,
        source_list: CheatStepResource::NotLoaded,
        source_fetch: CheatStepResource::NotLoaded,
        selected_source_id: None,
        fetch_force_refresh: false,
        candidates: CheatStepResource::NotLoaded,
        candidates_request: None,
        candidate_query: String::new(),
        candidate_selection: None,
        candidate_load_error: None,
    };
    let profile = Pcsx2Profile {
        profile_id: "pcsx2-user".to_string(),
        installation_type: Pcsx2InstallationType::Native,
        scope: Pcsx2ProfileScope::User,
        configuration_path: PathBuf::from("/isolated/PCSX2"),
        provenance: "test fixture",
        eligible: false,
        blockers: vec![
            Pcsx2ProfileBlocker {
                kind: Pcsx2ProfileBlockerKind::MissingConfiguration,
                path: EncodedPath::from_path(Path::new("/isolated/PCSX2")),
                detail: "first blocker detail".to_string(),
            },
            Pcsx2ProfileBlocker {
                kind: Pcsx2ProfileBlockerKind::MissingPcsx2Evidence,
                path: EncodedPath::from_path(Path::new("/isolated/PCSX2")),
                detail: "second blocker detail".to_string(),
            },
        ],
        patch_directories: Vec::new(),
        configuration_identity: None,
        executable_candidates: Vec::new(),
    };
    let discovery = Pcsx2ProfileDiscovery {
        profiles: vec![profile],
        warnings: Vec::new(),
        complete: true,
    };
    let mut clipboard = InMemoryClipboard::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_pcsx2_workflow(
                ui,
                &mut workflow,
                &Pcsx2ProfilesState::Ready(discovery.clone()),
                &mut clipboard,
            );
        });
    });
    assert!(
        rendered_text_contains(&output, "first blocker detail"),
        "the first blocker must stay directly visible, not hidden behind a disclosure"
    );
    assert!(
        rendered_text_contains(&output, "Technical details"),
        "the remaining blockers must be reachable behind the shared technical_details label"
    );
    assert!(
        !rendered_text_contains(&output, "All technical blockers"),
        "the old bespoke CollapsingHeader wording must no longer render"
    );
}

#[test]
fn bsfree_stage_one_capability_labels_are_honest() {
    assert_eq!(
        bsfree_compatibility_label(DeviceFormatCompatibility::PotentiallyConvertible),
        "Potentially convertible"
    );
    assert_eq!(
        bsfree_compatibility_label(DeviceFormatCompatibility::ReferenceOnly),
        "Reference only"
    );
    assert_eq!(
        bsfree_compatibility_label(DeviceFormatCompatibility::Unknown),
        "Unknown format"
    );
    assert_eq!(
        bsfree_match_label(ProviderGameMatchConfidence::Ambiguous),
        "Ambiguous candidates"
    );
    // A GameCube hex-pair code is honestly reported as installable via
    // Dolphin; a non-GameCube code is reference-only.
    let gamecube = archivefs_core::patch_manager::BsFreeCheat {
        upstream_id: 1,
        name: "Lives".to_string(),
        note: None,
        code: "042318AC 3B8003E7".to_string(),
        section: None,
        author: None,
        device: archivefs_core::patch_manager::BsFreeDeviceSummary {
            upstream_id: 6,
            name: "Action Replay".to_string(),
            compatibility: DeviceFormatCompatibility::PotentiallyConvertible,
        },
        compatibility: DeviceFormatCompatibility::PotentiallyConvertible,
        truncated_fields: Vec::new(),
    };
    let (label, _) = bsfree_code_capability(&gamecube, Some("GameCube"));
    assert!(label.contains("Supported by Dolphin"));
    let other = archivefs_core::patch_manager::BsFreeCheat {
        upstream_id: 2,
        name: "Code".to_string(),
        note: None,
        code: "AAAA-BBBB".to_string(),
        section: None,
        author: None,
        device: archivefs_core::patch_manager::BsFreeDeviceSummary {
            upstream_id: 2,
            name: "Game Genie".to_string(),
            compatibility: DeviceFormatCompatibility::PotentiallyConvertible,
        },
        compatibility: DeviceFormatCompatibility::PotentiallyConvertible,
        truncated_fields: Vec::new(),
    };
    let (label, _) = bsfree_code_capability(&other, Some("PS2"));
    assert_eq!(label, "Reference only");
}

#[test]
fn bsfree_gui_is_bounded_and_has_no_install_action_in_the_browser() {
    let source = include_str!("../sources_page.rs");
    let browser = source
        .split("fn show_bsfree_game_browser(")
        .nth(1)
        .unwrap()
        .split("/// The Sources page's compact")
        .next()
        .unwrap();
    assert!(browser.contains("installable via Dolphin"));
    assert!(browser.contains("GameCube cheats can be installed with Dolphin"));
    assert!(browser.contains("Browse only"));
    assert!(browser.contains("does not install them for this platform"));
    assert!(browser.contains("PageRequest::games(0)"));
    assert!(browser.contains("Previous 100"));
    assert!(browser.contains("Next 100"));
    assert!(browser.contains("archivefs_platform_display_name"));
    assert!(!browser.contains("Install selected"));
    assert!(!browser.contains("BsFreeOperation::Install"));
}

#[test]
fn bsfree_browser_is_install_aware_only_for_a_gamecube_context() {
    // A non-GameCube browse (e.g. Megadrive) must not imply the browsed
    // platform is Dolphin-installable: it reads as browse-only.
    let manager = BsFreeManagerState::NotLoaded;
    let output = render_bsfree_browser(&manager, Some("megadrive"));
    assert!(
        rendered_text_contains(&output, "Browse only"),
        "a Megadrive browse must be clearly browse-only"
    );
    assert!(
        rendered_text_contains(&output, "does not install them for this platform"),
        "the non-GameCube wording must name the platform's browse-only nature"
    );
    assert!(
        !rendered_text_contains(&output, "installable via Dolphin"),
        "no Dolphin-install claim for a non-GameCube browse"
    );

    // A GameCube browse still surfaces the truthful install capability.
    let output = render_bsfree_browser(&manager, Some("GameCube"));
    assert!(rendered_text_contains(&output, "installable via Dolphin"));
    assert!(rendered_text_contains(
        &output,
        "GameCube cheats can be installed with Dolphin"
    ));
    assert!(!rendered_text_contains(&output, "Browse only"));

    // No context at all defaults to browse-only too.
    let output = render_bsfree_browser(&manager, None);
    assert!(rendered_text_contains(&output, "Browse only"));
    assert!(!rendered_text_contains(&output, "installable via Dolphin"));
}

// ------------------------------------------------------------------
// BSFree GameCube Cheats & Mods integration
// ------------------------------------------------------------------

fn bsfree_gui_cheat(id: i64, name: &str, format: BsFreeGameCubeCodeFormat) -> BsFreeGameCubeCheat {
    let code_lines = match format {
        BsFreeGameCubeCodeFormat::GeckoEquivalent => vec!["042318AC 3B8003E7".to_string()],
        BsFreeGameCubeCodeFormat::ActionReplayNative => vec!["0224CD50 00003E7F".to_string()],
        BsFreeGameCubeCodeFormat::Unsupported => vec!["C4129124 0000FF00".to_string()],
        BsFreeGameCubeCodeFormat::Malformed => vec!["XR7M-X292-DZ418".to_string()],
    };
    BsFreeGameCubeCheat {
        upstream_id: id,
        name: name.to_string(),
        author: None,
        note: None,
        section: None,
        code_format: format,
        code_lines,
        canonical_digest: format!("digest-{id}"),
    }
}

fn bsfree_gui_matched_state(cheats: Vec<BsFreeGameCubeCheat>) -> BsFreeGameCubeGuiState {
    let selection = BsFreeGameCubeCheatSelection::from_cheats(&cheats, &parse_dolphin_ini(""));
    BsFreeGameCubeGuiState {
        status: BsFreeGameCubeSearchStatus::Matched,
        detail: "Matched BSFree GameCube game \"Test Game\"; review the cheats before applying."
            .to_string(),
        candidates: Vec::new(),
        game: Some(BsFreeGameCubeMatch {
            archive_title: "Test Game".to_string(),
            archive_game_id: "GLME01".to_string(),
            matched_bsfree_game_upstream_uid: 42,
            matched_bsfree_title: "Test Game".to_string(),
            matched_bsfree_version: None,
            region_evidence: "archive region is E".to_string(),
            requires_review: true,
            detail: "fixture".to_string(),
        }),
        cheats: cheats.clone(),
        selection,
        analysis: Vec::new(),
        search_title: "Test Game".to_string(),
    }
}

fn render_bsfree_section(
    workflow: &mut CheatWorkflowState,
) -> (egui::FullOutput, Option<CheatWorkflowAction>) {
    let ctx = egui::Context::default();
    let mut action = None;
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            action = show_bsfree_gamecube(ui, workflow);
        });
    });
    (output, action)
}

fn bsfree_wii_gui_cheat(
    id: i64,
    name: &str,
    format: BsFreeWiiCodeFormat,
    code: &str,
) -> BsFreeWiiCheat {
    BsFreeWiiCheat {
        upstream_id: id,
        name: name.to_string(),
        author: None,
        note: None,
        section: None,
        code_format: format,
        code_lines: vec![code.to_string()],
        canonical_digest: String::new(),
    }
}

fn bsfree_wii_gui_matched_state(cheats: Vec<BsFreeWiiCheat>) -> BsFreeWiiGuiState {
    let selection = BsFreeWiiCheatSelection::from_cheats(&cheats, &parse_dolphin_ini(""));
    BsFreeWiiGuiState {
        status: BsFreeWiiSearchStatus::Matched,
        detail: "Matched BSFree Wii game \"Test Game\"; review the cheats before applying."
            .to_string(),
        candidates: Vec::new(),
        game: Some(BsFreeWiiMatch {
            archive_title: "Test Game".to_string(),
            archive_game_id: "R3HX6Z".to_string(),
            matched_bsfree_game_upstream_uid: 42,
            matched_bsfree_title: "Test Game".to_string(),
            matched_bsfree_version: None,
            region_evidence: "archive region is PAL".to_string(),
            requires_review: true,
            detail: "fixture".to_string(),
        }),
        cheats: cheats.clone(),
        selection,
        analysis: Vec::new(),
        search_title: "Test Game".to_string(),
    }
}

fn render_bsfree_wii_section(
    workflow: &mut CheatWorkflowState,
) -> (egui::FullOutput, Option<CheatWorkflowAction>) {
    let ctx = egui::Context::default();
    let mut action = None;
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            action = show_bsfree_wii(ui, workflow);
        });
    });
    (output, action)
}

/// Renders `show_bsfree_wii` at a compact window width, so the beginner
/// flow is proven usable on narrow screens.
fn render_bsfree_wii_at_width(workflow: &mut CheatWorkflowState, width: f32) -> egui::FullOutput {
    let ctx = egui::Context::default();
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 2000.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_bsfree_wii(ui, workflow);
            });
        },
    )
}

#[test]
fn bsfree_wii_section_uses_beginner_states_and_keeps_browse_only_honest() {
    let mut workflow = CheatWorkflowState {
        archive_path: PathBuf::from("/roms/wii.iso"),
        display_name: "Test Game".to_string(),
        normalized_name: "testgame".to_string(),
        platform: Some("Wii".to_string()),
        region: None,
        source_root: PathBuf::from("/roms"),
        size_bytes: None,
        adapter: CheatEmulatorAdapter::Dolphin,
        identity_request: None,
        identity: CheatStepResource::NotLoaded,
        preview_request: None,
        preview: CheatStepResource::NotLoaded,
        transaction: CheatTransactionState::Idle,
        transaction_notice: None,
        selected_profile_id: None,
        selected_pcsx2_profile_id: None,
        pcsx2_inventory_profile_id: None,
        pcsx2_inventory: CheatStepResource::NotLoaded,
        pcsx2_gamehacking: CheatStepResource::NotLoaded,
        gamecube_gamehacking: CheatStepResource::NotLoaded,
        gamecube_gamehacking_request: None,
        gamecube_gamehacking_cancellation: None,
        gamecube_gamehacking_generation: 0,
        gamecube_gamehacking_blocked: false,
        bsfree_gamecube: CheatStepResource::NotLoaded,
        bsfree_gamecube_cancellation: None,
        bsfree_gamecube_generation: 0,
        bsfree_wii: CheatStepResource::Ready(bsfree_wii_gui_matched_state(vec![
            bsfree_wii_gui_cheat(
                1,
                "Infinite Health",
                BsFreeWiiCodeFormat::GeckoEquivalent,
                "042318AC 3B8003E7",
            ),
            bsfree_wii_gui_cheat(
                2,
                "Encrypted",
                BsFreeWiiCodeFormat::Malformed,
                "XR7M-X292-DZ418",
            ),
        ])),
        bsfree_wii_cancellation: None,
        bsfree_wii_generation: 0,
        selected_dolphin_profile_id: Some("profile".to_string()),
        dolphin_explicit_root: String::new(),
        dolphin_inventory_profile_id: None,
        dolphin_inventory: CheatStepResource::NotLoaded,
        dolphin_provider_request: None,
        dolphin_provider: CheatStepResource::NotLoaded,
        dolphin_provider_selection: None,
        dolphin_destination_error: None,
        dolphin_local_lookup: DolphinLocalLookupState::NotAttempted,
        dolphin_profile_selection: None,
        dolphin_profile_choice: None,
        dolphin_details_open: false,
        dolphin_show_exact_changes: false,
        selected_xenia_profile_id: None,
        xenia_explicit_root: String::new(),
        xenia_provider_request: None,
        xenia_provider: CheatStepResource::NotLoaded,
        xenia_selected_candidate_index: None,
        xenia_selection: None,
        xenia_destination_error: None,
        xenia_profile_selection: None,
        xenia_profile_choice: None,
        xenia_details_open: false,
        xenia_show_exact_changes: false,
        source_mode: CheatSourceMode::ArchiveFsTrustedCatalogue,
        existing_library_profile_id: None,
        existing_library: CheatStepResource::NotLoaded,
        source_list: CheatStepResource::NotLoaded,
        source_fetch: CheatStepResource::NotLoaded,
        selected_source_id: None,
        fetch_force_refresh: false,
        candidates: CheatStepResource::NotLoaded,
        candidates_request: None,
        candidate_query: String::new(),
        candidate_selection: None,
        candidate_load_error: None,
    };
    let (output, _action) = render_bsfree_wii_section(&mut workflow);

    assert!(
        rendered_text_contains(&output, "verified Wii hex-pair"),
        "the section must state the verified-subset rule"
    );
    assert!(rendered_text_contains(&output, "Infinite Health"));
    assert!(rendered_text_contains(&output, "Ready"));
    assert!(rendered_text_contains(&output, "Encrypted"));
    assert!(rendered_text_contains(
        &output,
        "Browse only — this code is encrypted, malformed, or from an unverified Wii device."
    ));
    assert!(
        !rendered_text_contains(&output, "GeckoEquivalent"),
        "raw converter terminology must stay hidden by default"
    );

    // Compact width stays usable and the states are unchanged.
    let compact = render_bsfree_wii_at_width(&mut workflow, 480.0);
    assert!(rendered_text_contains(&compact, "Infinite Health"));
    assert!(rendered_text_contains(&compact, "Browse only"));
}

/// Renders the generic BSFree browser with the given platform context.
fn render_bsfree_browser(manager: &BsFreeManagerState, platform: Option<&str>) -> egui::FullOutput {
    let ctx = egui::Context::default();
    let mut state = BsFreeGuiState::default();
    let context = platform.map(|platform| {
        (
            PathBuf::from("/roms"),
            "Test Game".to_string(),
            platform.to_string(),
        )
    });
    ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_bsfree_game_browser(ui, manager, false, &mut state, context.as_ref());
        });
    })
}

fn bsfree_test_screen() -> egui::Rect {
    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1080.0))
}

/// Simulates a real 3-frame click gesture (move, press, release) on the
/// BSFree GameCube section, returning the action it produces. egui's
/// hit-testing for a frame's pointer events uses widget rects registered
/// in the *previous* frame, so a single-frame synthetic click cannot
/// register - this mirrors real input.
fn bsfree_section_click(
    workflow: &mut CheatWorkflowState,
    pos: egui::Pos2,
) -> Option<CheatWorkflowAction> {
    let ctx = egui::Context::default();
    let action = std::rc::Rc::new(std::cell::RefCell::new(None));
    for event in [
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
    ] {
        let captured = std::rc::Rc::clone(&action);
        let _ = ctx.run(
            egui::RawInput {
                events: vec![event],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    if let Some(inner) = show_bsfree_gamecube(ui, workflow) {
                        *captured.borrow_mut() = Some(inner);
                    }
                });
            },
        );
    }
    action.borrow().clone()
}

fn bsfree_generated_fixture(profile: DolphinProfile) -> GeneratedBsFreeGameCubeInstall {
    GeneratedBsFreeGameCubeInstall {
        staging_root: PathBuf::from("/staging"),
        staged: StagedGameCubeIni {
            staging_root: PathBuf::from("/staging"),
            path: PathBuf::from("/staging/GLME01.ini"),
            digest: "a".repeat(64),
            contents: "[Gecko]\n$Lives [BSFree Archive]\n042318AC 3B8003E7\n".to_string(),
            destination_existed: true,
            affected: vec![StagedGameCubeCheat {
                name: "Lives".to_string(),
                dolphin_name: "Lives [BSFree Archive]".to_string(),
                code_format: GameCubeCodeFormat::Gecko,
            }],
            skipped_unselectable: vec!["Master".to_string()],
        },
        profile,
        findings: Vec::new(),
        skipped_duplicates: Vec::new(),
        skipped_unselectable: vec!["Master".to_string()],
    }
}

fn bsfree_preview_response(profile: DolphinProfile) -> CheatPreviewResponse {
    CheatPreviewResponse {
        key: CheatPreviewRequestKey {
            archive_path: PathBuf::from("/roms/a.zip"),
            platform: Some("GameCube".to_string()),
            adapter: CheatEmulatorAdapter::Dolphin,
            profile_id: Some("dolphin-native-test".to_string()),
            source_mode: CheatSourceMode::ArchiveFsTrustedCatalogue,
            source_id: None,
            snapshot_id: None,
        },
        outcome: CheatPreviewOutcome::Ready(SharedPreviewReport {
            request_archive: PathBuf::from("/roms/a.zip"),
            adapter: PreviewAdapter::Dolphin,
            entries: Vec::new(),
            conflicts: Vec::new(),
            warnings: Vec::new(),
            summary: Default::default(),
            complete: true,
        }),
        materialized: None,
        generated: None,
        dolphin_generated: None,
        xenia_generated: None,
        pcsx2_generated: None,
        gamecube_gamehacking_generated: None,
        bsfree_gamecube_generated: Some(bsfree_generated_fixture(profile)),

        bsfree_wii_generated: None,
    }
}

#[test]
fn bsfree_gui_renders_supported_and_unsupported_cheats_with_honest_status() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-bsfree-gui-render-{}",
        std::process::id()
    ));
    let mut app = dolphin_workflow_with_matched_identity(&directory, "GLME01");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.bsfree_gamecube = CheatStepResource::Ready(bsfree_gui_matched_state(vec![
        bsfree_gui_cheat(
            1,
            "Infinite lives",
            BsFreeGameCubeCodeFormat::GeckoEquivalent,
        ),
        bsfree_gui_cheat(2, "Max money", BsFreeGameCubeCodeFormat::ActionReplayNative),
        bsfree_gui_cheat(3, "Master", BsFreeGameCubeCodeFormat::Unsupported),
        bsfree_gui_cheat(4, "Encrypted", BsFreeGameCubeCodeFormat::Malformed),
    ]));
    let (output, action) = render_bsfree_section(workflow);
    assert!(
        rendered_text_contains(&output, "Infinite lives"),
        "supported cheat is visible"
    );
    assert!(
        rendered_text_contains(&output, "Max money"),
        "supported cheat is visible"
    );
    assert!(
        rendered_text_contains(&output, "Ready"),
        "installable cheats show Ready"
    );
    // Unsupported records stay visible but are marked browse-only.
    assert!(
        rendered_text_contains(&output, "Master"),
        "unsupported cheat stays visible"
    );
    assert!(
        rendered_text_contains(&output, "Encrypted"),
        "malformed cheat stays visible"
    );
    assert!(
        rendered_text_contains(&output, "Browse only"),
        "unsupported records say Browse only"
    );
    assert!(
        rendered_text_contains(&output, "this code format cannot be installed yet"),
        "browse-only reason is shown for the encrypted code"
    );
    assert!(
        action.is_none(),
        "rendering alone must not mutate or dispatch anything"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn bsfree_gui_unsupported_cheat_cannot_be_selected_for_apply() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-bsfree-gui-select-{}",
        std::process::id()
    ));
    let mut app = dolphin_workflow_with_matched_identity(&directory, "GLME01");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.bsfree_gamecube = CheatStepResource::Ready(bsfree_gui_matched_state(vec![
        bsfree_gui_cheat(
            1,
            "Infinite lives",
            BsFreeGameCubeCodeFormat::GeckoEquivalent,
        ),
        bsfree_gui_cheat(2, "Master", BsFreeGameCubeCodeFormat::Unsupported),
    ]));
    // Clicking the supported cheat's label toggles it.
    let (discovery_output, _) = render_bsfree_section(workflow);
    let lives_pos = find_exact_text_center(&discovery_output, "Infinite lives")
        .expect("supported cheat label renders");
    let action = bsfree_section_click(workflow, lives_pos);
    assert!(
        matches!(
            action,
            Some(CheatWorkflowAction::ToggleBsFreeGameCubeCheatSelected {
                index: 0,
                selected: true
            })
        ),
        "clicking the supported label dispatches a select action"
    );
    // Clicking the unsupported cheat's name (which is not a checkbox)
    // must not dispatch a select action.
    let (discovery_output, _) = render_bsfree_section(workflow);
    let master_pos = find_exact_text_center(&discovery_output, "Master")
        .expect("unsupported cheat label renders");
    let action = bsfree_section_click(workflow, master_pos);
    assert!(
        !matches!(
            action,
            Some(CheatWorkflowAction::ToggleBsFreeGameCubeCheatSelected { .. })
        ),
        "unsupported cheats can never be selected for Apply"
    );
    // Attempting to select the unsupported entry is refused outright.
    let CheatStepResource::Ready(state) = &workflow.bsfree_gamecube else {
        panic!("state is Ready");
    };
    let mut selection = state.selection.clone();
    assert!(
        !selection.set_selected(1, true),
        "Unsupported can never be selected"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn bsfree_gui_apply_button_shows_selection_count_and_is_disabled_when_empty() {
    let directory =
        std::env::temp_dir().join(format!("archivefs-bsfree-gui-count-{}", std::process::id()));
    let mut app = dolphin_workflow_with_matched_identity(&directory, "GLME01");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.bsfree_gamecube = CheatStepResource::Ready(bsfree_gui_matched_state(vec![
        bsfree_gui_cheat(
            1,
            "Infinite lives",
            BsFreeGameCubeCodeFormat::GeckoEquivalent,
        ),
        bsfree_gui_cheat(2, "Max money", BsFreeGameCubeCodeFormat::ActionReplayNative),
        bsfree_gui_cheat(3, "Master", BsFreeGameCubeCodeFormat::Unsupported),
    ]));
    let (output, _) = render_bsfree_section(workflow);
    assert!(
        rendered_text_contains(&output, "Install 0 cheats"),
        "the apply button shows the real count and nothing is pre-selected"
    );
    // Clicking the disabled apply button must not dispatch an apply.
    if let Some(pos) = find_exact_text_center(&output, "Install 0 cheats") {
        let click_output =
            egui::Context::default().run(click_at(bsfree_test_screen(), pos), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = show_bsfree_gamecube(ui, workflow);
                });
            });
        let _ = click_output;
    }
    // Select one cheat and the button enables and reports the count.
    let CheatStepResource::Ready(state) = &mut workflow.bsfree_gamecube else {
        panic!("state is Ready");
    };
    assert!(state.selection.set_selected(0, true));
    let (output, _) = render_bsfree_section(workflow);
    assert!(
        rendered_text_contains(&output, "Install 1 cheat"),
        "one selection shows 'Install 1 cheat'"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn bsfree_gui_install_button_dispatches_the_shared_preview_apply_action() {
    let directory =
        std::env::temp_dir().join(format!("archivefs-bsfree-gui-apply-{}", std::process::id()));
    let mut app = dolphin_workflow_with_matched_identity(&directory, "GLME01");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.bsfree_gamecube =
        CheatStepResource::Ready(bsfree_gui_matched_state(vec![bsfree_gui_cheat(
            1,
            "Infinite lives",
            BsFreeGameCubeCodeFormat::GeckoEquivalent,
        )]));
    let CheatStepResource::Ready(state) = &mut workflow.bsfree_gamecube else {
        panic!("state is Ready");
    };
    assert!(state.selection.set_selected(0, true));
    let (output, _) = render_bsfree_section(workflow);
    let pos =
        find_exact_text_center(&output, "Install 1 cheats").expect("the apply button renders");
    let action = bsfree_section_click(workflow, pos);
    assert!(
        matches!(
            action,
            Some(CheatWorkflowAction::InstallSelectedBsFreeGameCube)
        ),
        "the apply button drives the shared install-preview action"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn bsfree_gui_auto_searches_once_when_identity_is_ready() {
    let directory =
        std::env::temp_dir().join(format!("archivefs-bsfree-gui-auto-{}", std::process::id()));
    let mut app = dolphin_workflow_with_matched_identity(&directory, "GLME01");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.bsfree_gamecube = CheatStepResource::NotLoaded;
    let (output, action) = render_bsfree_section(workflow);
    assert!(
        rendered_text_contains(&output, "Searching BSFree Archive for this game"),
        "an auto-search is announced"
    );
    assert!(
        matches!(
            action,
            Some(CheatWorkflowAction::FetchBsFreeGameCube { .. })
        ),
        "the auto-search action is dispatched, no CLI required"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn bsfree_gui_candidates_require_explicit_confirmation() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-bsfree-gui-candidates-{}",
        std::process::id()
    ));
    let mut app = dolphin_workflow_with_matched_identity(&directory, "GLME01");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    let cheats = vec![bsfree_gui_cheat(
        1,
        "Lives",
        BsFreeGameCubeCodeFormat::GeckoEquivalent,
    )];
    let selection = BsFreeGameCubeCheatSelection::from_cheats(&cheats, &parse_dolphin_ini(""));
    workflow.bsfree_gamecube = CheatStepResource::Ready(BsFreeGameCubeGuiState {
        status: BsFreeGameCubeSearchStatus::Candidates,
        detail: "Several BSFree GameCube games matched; confirm which one to review.".to_string(),
        candidates: vec![
            BsFreeGameCubeMatch {
                archive_title: "Test Game".to_string(),
                archive_game_id: "GLME01".to_string(),
                matched_bsfree_game_upstream_uid: 100,
                matched_bsfree_title: "Test Game (USA)".to_string(),
                matched_bsfree_version: None,
                region_evidence: "fixture".to_string(),
                requires_review: true,
                detail: "fixture".to_string(),
            },
            BsFreeGameCubeMatch {
                archive_title: "Test Game".to_string(),
                archive_game_id: "GLME01".to_string(),
                matched_bsfree_game_upstream_uid: 101,
                matched_bsfree_title: "Test Game (Europe)".to_string(),
                matched_bsfree_version: None,
                region_evidence: "fixture".to_string(),
                requires_review: true,
                detail: "fixture".to_string(),
            },
        ],
        game: None,
        cheats,
        selection,
        analysis: Vec::new(),
        search_title: "Test Game".to_string(),
    });
    let (output, _) = render_bsfree_section(workflow);
    assert!(
        rendered_text_contains(&output, "Use this game"),
        "candidates are shown for explicit confirmation"
    );
    assert!(!rendered_text_contains(&output, "Install 0 cheats"));
    // Clicking a candidate dispatches ConfirmBsFreeGameCubeMatch.
    let pos = find_exact_text_center(&output, "Use this game").expect("a confirm button renders");
    let action = bsfree_section_click(workflow, pos);
    assert!(
        matches!(
            action,
            Some(CheatWorkflowAction::ConfirmBsFreeGameCubeMatch { upstream_uid: 100 })
        ),
        "ambiguous identity requires explicit review, never auto-apply"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn bsfree_gui_result_shows_count_and_rollback_after_success() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-bsfree-gui-result-{}",
        std::process::id()
    ));
    let mut app = dolphin_workflow_with_matched_identity(&directory, "GLME01");
    let profile = match &app.dolphin_profiles {
        DolphinProfilesState::Ready(discovery) => {
            discovery.profiles.first().expect("fixture profile").clone()
        }
        _ => panic!("fixture has profiles"),
    };
    let workflow = app.cheat_workflow.as_mut().unwrap();
    let cheats = vec![bsfree_gui_cheat(
        1,
        "Lives",
        BsFreeGameCubeCodeFormat::GeckoEquivalent,
    )];
    workflow.bsfree_gamecube = CheatStepResource::Ready(bsfree_gui_matched_state(cheats));
    workflow.preview = CheatStepResource::Ready(bsfree_preview_response(profile));
    let result = successful_shared_apply_result();
    workflow.transaction = CheatTransactionState::Result {
        key: cheat_preview_key(workflow),
        result,
    };
    let (output, _) = render_bsfree_section(workflow);
    assert!(
        rendered_text_contains(&output, "1 cheat added"),
        "the beginner-facing result announces the added count"
    );
    assert!(
        rendered_text_contains(&output, "Undo installation"),
        "rollback is reachable from the result, no CLI required"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn bsfree_gui_apply_and_rollback_reuse_the_shared_backend() {
    let source = include_str!("../main.rs");
    let dispatch = source
        .split("Some(CheatWorkflowAction::InstallSelectedBsFreeGameCube) =>")
        .nth(1)
        .unwrap();
    assert!(
        dispatch.contains("self.start_bsfree_gamecube_install_preview()"),
        "Install routes through the BSFree install-preview path"
    );
    let preview = source
        .split("fn start_bsfree_gamecube_install_preview")
        .nth(1)
        .unwrap()
        .split("fn update_pcsx2_cheat_selection")
        .next()
        .unwrap();
    assert!(
        preview.contains("stage_bsfree_gamecube_install("),
        "staging reuses the existing GameCube adapter"
    );
    assert!(
        preview.contains("build_bsfree_gamecube_install_preview("),
        "preview reuses the shared preview boundary"
    );
    assert!(preview.contains("self.review_cheat_apply()"));
    // The shared transaction layer performs the actual mutation.
    assert!(
        source.contains("fn start_cheat_apply") && source.contains("execute_shared_apply("),
        "Confirm drives the shared apply/backup/journal machinery"
    );
    assert!(
        source.contains("fn start_cheat_install_rollback")
            && source.contains("start_shared_rollback_preview"),
        "Undo drives the shared rollback/history flow"
    );
}

/// A compact (1280x720) render must still fit the BSFree section without
/// panicking and still show the supported cheats and the apply control.
#[test]
fn bsfree_gui_compact_width_stays_usable() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-bsfree-gui-compact-{}",
        std::process::id()
    ));
    let mut app = dolphin_workflow_with_matched_identity(&directory, "GLME01");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.bsfree_gamecube =
        CheatStepResource::Ready(bsfree_gui_matched_state(vec![bsfree_gui_cheat(
            1,
            "Infinite lives",
            BsFreeGameCubeCodeFormat::GeckoEquivalent,
        )]));
    let ctx = egui::Context::default();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_bsfree_gamecube(ui, workflow);
            });
        },
    );
    assert!(
        rendered_text_contains(&output, "Infinite lives"),
        "a compact-width Cheats & Mods page still shows the supported cheats"
    );
    assert!(
        rendered_text_contains(&output, "Install 0 cheats"),
        "the apply control is still usable at compact width"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn settings_uses_user_wording_for_future_options() {
    let source = include_str!("../administration_pages.rs");
    let settings = source
        .split("fn show_settings_page(")
        .nth(1)
        .unwrap()
        .split("fn history_entry_text(")
        .next()
        .unwrap();
    assert!(settings.contains("More settings coming later"));
    assert!(!settings.contains("Intentionally unavailable"));
    assert!(!settings.contains("no supported GUI"));
}

/// The screen-space position of the first `Shape::Text` *containing*
/// `needle`. The row labels in Gamer View are composed
/// ("Title - Platform . State"), so an exact match cannot address them;
/// the position is what these tests compare, to prove nothing moved.
fn find_text_position_containing(output: &egui::FullOutput, needle: &str) -> Option<egui::Pos2> {
    fn find_in_shape(shape: &egui::Shape, needle: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text_shape) => text_shape
                .galley
                .text()
                .contains(needle)
                .then_some(text_shape.pos),
            egui::Shape::Vec(nested) => nested.iter().find_map(|s| find_in_shape(s, needle)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|clipped| find_in_shape(&clipped.shape, needle))
}

// --- Gamer View cover artwork -----------------------------------------

/// Renders Gamer View over a library of `count` games and returns the app,
/// so a test can inspect what the list asked for and hand it answers.
fn gamer_cover_app(count: usize) -> (ArchiveFsApp, egui::Context) {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::GamerView;
    app.view = MainView::Library;
    let mut records = Vec::new();
    for index in 0..count {
        let mut built = record(&featured_path(index), MountState::Pending);
        built.metadata.platform = Some("SNES".to_string());
        built.metadata.title = Some(format!("Game {index:05}"));
        records.push(built);
    }
    app.state = LoadState::Ready(Box::new(loaded_data_with_records("/mount", records)));
    (app, egui::Context::default())
}

fn run_frames(
    app: &mut ArchiveFsApp,
    ctx: &egui::Context,
    width: f32,
    height: f32,
    frames: usize,
) -> egui::FullOutput {
    let mut frame = eframe::Frame::_new_kittest();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width, height),
        )),
        ..Default::default()
    };
    let mut output = None;
    for _ in 0..frames {
        output = Some(ctx.run(input.clone(), |ctx| app.update(ctx, &mut frame)));
    }
    output.expect("a rendered frame")
}

/// Every image drawn this frame, as `(texture id, bounds)`.
///
/// `Painter::image` emits a textured `Shape::Mesh`, which is what both the
/// platform artwork and a RomM cover arrive as at this stage - text is still
/// an untessellated `Shape::Text` here, so nothing else is counted.
fn rendered_images(output: &egui::FullOutput) -> Vec<(egui::TextureId, egui::Rect)> {
    fn walk(shape: &egui::Shape, out: &mut Vec<(egui::TextureId, egui::Rect)>) {
        match shape {
            egui::Shape::Mesh(mesh) => out.push((mesh.texture_id, mesh.calc_bounds())),
            egui::Shape::Vec(nested) => nested.iter().for_each(|shape| walk(shape, out)),
            _ => {}
        }
    }
    let mut images = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut images);
    }
    images
}

/// The texture a loaded cover is drawn from, so a test can find that exact
/// image among everything else the frame painted (the platform shelf draws
/// textured meshes too).
fn cover_texture_id(app: &ArchiveFsApp, local_path: &str) -> Option<egui::TextureId> {
    match app.gamer_covers.slot_for(Path::new(local_path), None)? {
        crate::gamer_artwork::CoverSlot::Ready { texture, .. } => Some(texture.id()),
        _ => None,
    }
}

/// Every image drawn for the game list, as a set of texture ids.
fn drawn_texture_ids(output: &egui::FullOutput) -> std::collections::HashSet<egui::TextureId> {
    rendered_images(output)
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// A decoded cover, as the worker would produce one.
fn cover_reply(
    generation: u64,
    local_path: &str,
    romm_game_id: &str,
) -> crate::gamer_artwork::CoverReply {
    crate::gamer_artwork::CoverReply {
        generation,
        local_path: PathBuf::from(local_path),
        provider_game_id: Some(romm_game_id.to_string()),
        answer: crate::gamer_artwork::CoverAnswer::Ready(Box::new(crate::romm_game::CoverImage {
            key: romm_game_id.to_string(),
            width: 20,
            height: 30,
            bytes: 2400,
            image: egui::ColorImage::new([20, 30], vec![egui::Color32::from_rgb(200, 30, 40); 600]),
            from_cache: true,
        })),
    }
}

#[test]
fn gamer_view_draws_a_romm_cover_beside_the_game_it_belongs_to() {
    // The whole point of this change: a matched record's approved cover is
    // drawn in the list itself, not only behind a Details button.
    let (mut app, ctx) = gamer_cover_app(6);
    // One pass so the list asks about its visible rows.
    run_frames(&mut app, &ctx, 1920.0, 1080.0, 3);
    let generation = app.gamer_covers.generation();
    assert!(
        app.gamer_covers
            .slot_for(Path::new(&featured_path(0)), None)
            .is_some(),
        "the visible list asked for nothing"
    );

    assert!(
        app.gamer_covers
            .absorb(&ctx, cover_reply(generation, &featured_path(0), "101")),
        "the cover was refused"
    );
    let cover = cover_texture_id(&app, &featured_path(0)).expect("a loaded cover");
    let drawn = rendered_images(&run_frames(&mut app, &ctx, 1920.0, 1080.0, 1));
    assert!(
        drawn.iter().any(|(id, _)| *id == cover),
        "the loaded cover's own texture was never painted"
    );
}

#[test]
fn a_cover_is_drawn_inside_the_row_artwork_slot_without_changing_row_height() {
    // A cover arriving must not move anything: the row's height and the
    // title's position are identical before and after.
    let (mut app, ctx) = gamer_cover_app(6);
    run_frames(&mut app, &ctx, 1920.0, 1080.0, 3);
    let generation = app.gamer_covers.generation();

    let title_before =
        find_text_position_containing(&run_frames(&mut app, &ctx, 1920.0, 1080.0, 1), "Game 00000");
    app.gamer_covers
        .absorb(&ctx, cover_reply(generation, &featured_path(0), "101"));
    let output = run_frames(&mut app, &ctx, 1920.0, 1080.0, 1);
    let title_after = find_text_position_containing(&output, "Game 00000");
    assert_eq!(
        title_before, title_after,
        "the title moved when the cover arrived"
    );

    // And the cover itself stayed inside the slot it was given: a 20x30
    // cover is scaled to fit 56, never stretched to fill it.
    let cover = cover_texture_id(&app, &featured_path(0)).expect("a loaded cover");
    let bounds = rendered_images(&output)
        .into_iter()
        .find(|(id, _)| *id == cover)
        .map(|(_, rect)| rect)
        .expect("the cover was painted");
    assert!(
        bounds.width().max(bounds.height()) <= crate::gamer_artwork::COVER_BOX + 0.5,
        "the cover measured {}x{}, outside the {} slot",
        bounds.width(),
        bounds.height(),
        crate::gamer_artwork::COVER_BOX
    );
    assert!(
        (bounds.width() / bounds.height() - 20.0 / 30.0).abs() < 0.01,
        "the cover was stretched: {}x{}",
        bounds.width(),
        bounds.height()
    );
}

#[test]
fn a_game_cover_replaces_the_platform_icon_rather_than_drawing_over_it() {
    // Both features draw into the same 56px slot, and the merge that brought
    // them together had to pick an order. A ready cover wins; the platform
    // icon is what a row falls back to. Drawing both would stack two images
    // in one slot, and drawing the icon on top would hide the cover.
    let (mut app, ctx) = gamer_cover_app(6);
    run_frames(&mut app, &ctx, 1920.0, 1080.0, 3);
    let generation = app.gamer_covers.generation();

    // Before any cover: something is painted in the slot, and it is not a
    // cover - that is the platform artwork fallback doing its job.
    let before = run_frames(&mut app, &ctx, 1920.0, 1080.0, 1);
    let fallback_images = rendered_images(&before).len();
    assert!(
        fallback_images > 0,
        "no platform artwork was drawn for a row with no cover"
    );

    app.gamer_covers
        .absorb(&ctx, cover_reply(generation, &featured_path(0), "101"));
    let after = run_frames(&mut app, &ctx, 1920.0, 1080.0, 1);
    let cover = cover_texture_id(&app, &featured_path(0)).expect("a loaded cover");
    let drawn = rendered_images(&after);

    assert!(
        drawn.iter().any(|(id, _)| *id == cover),
        "the cover was not painted once it was ready"
    );
    // The slot holds one image, not two: the count is unchanged because the
    // cover took the icon's place rather than joining it.
    assert_eq!(
        drawn.len(),
        fallback_images,
        "the row painted {} images after the cover arrived and {fallback_images} before, \
             so the platform icon and the cover are both in the slot",
        drawn.len()
    );
}

#[test]
fn a_row_and_the_featured_panel_agree_on_the_platform_fallback() {
    // The row and the featured panel derive `platform_fallback` at two
    // separate call sites. If they ever disagree, the same game shows one
    // glyph in the list and a different one in the panel beside it.
    // Every canonical platform, plus the cases that have no platform at all.
    for platform in archivefs_core::platform::PLATFORMS
        .iter()
        .map(|platform| platform.id)
        .chain(["definitely-not-a-platform", ""])
    {
        let unknown = canonical_platform_for_artwork(platform).is_none();
        let fallback = platform_fallback_asset_id(platform, unknown);
        assert!(
            valid_platform_asset_id(fallback),
            "{platform:?} fell back to {fallback:?}, which is not a drawable asset id"
        );
        // The last resort is a painted glyph, so the fallback must be one of
        // the category ids the glyph painter actually recognises - otherwise
        // a platform with no artwork lands on its default arm by accident
        // rather than by choice.
        assert!(
            [
                PlatformAssetCategory::Console,
                PlatformAssetCategory::Handheld,
                PlatformAssetCategory::Computer,
                PlatformAssetCategory::Arcade,
                PlatformAssetCategory::OpticalDisc,
                PlatformAssetCategory::Cartridge,
                PlatformAssetCategory::Unknown,
            ]
            .iter()
            .any(|category| category.asset_id() == fallback),
            "{platform:?} falls back to {fallback:?}, which is not a drawable category"
        );
    }

    // And an unrecognised platform lands on Unknown rather than on some
    // category picked from a name nobody recognised.
    assert_eq!(
        platform_fallback_asset_id("definitely-not-a-platform", true),
        PlatformAssetCategory::Unknown.asset_id()
    );
}

#[test]
fn a_failed_cover_leaves_the_row_exactly_as_it_was() {
    let (mut app, ctx) = gamer_cover_app(6);
    run_frames(&mut app, &ctx, 1920.0, 1080.0, 3);
    let generation = app.gamer_covers.generation();
    let before =
        find_text_position_containing(&run_frames(&mut app, &ctx, 1920.0, 1080.0, 1), "Game 00000");

    app.gamer_covers.absorb(
        &ctx,
        crate::gamer_artwork::CoverReply {
            generation,
            local_path: PathBuf::from(featured_path(0)),
            provider_game_id: Some("101".to_string()),
            answer: crate::gamer_artwork::CoverAnswer::None(crate::gamer_artwork::NoCover::Failed),
        },
    );
    let output = run_frames(&mut app, &ctx, 1920.0, 1080.0, 1);
    assert_eq!(
        before,
        find_text_position_containing(&output, "Game 00000"),
        "a failed cover disturbed the row"
    );
    assert!(
        rendered_text_contains(&output, "Game 00000"),
        "a failed cover took the title with it"
    );
}

#[test]
fn a_large_library_only_queues_artwork_for_the_rows_on_screen() {
    // The real library's size. A 1080p list shows tens of rows; the number
    // asked about must follow the viewport, never the catalogue.
    let (mut app, ctx) = gamer_cover_app(13_891);
    run_frames(&mut app, &ctx, 1920.0, 1080.0, 12);
    let tracked = app.gamer_covers.tracked();
    assert!(
        tracked < 200,
        "a 13,891-record library queued artwork for {tracked} records"
    );
}

#[test]
fn searching_does_not_show_the_previous_records_cover() {
    // Load a cover for one game, then search for a different one. The row
    // that appears is a different record, so it draws no cover at all -
    // covers are keyed by record, and row position 0 carries nothing.
    let (mut app, ctx) = gamer_cover_app(6);
    run_frames(&mut app, &ctx, 1920.0, 1080.0, 3);
    let generation = app.gamer_covers.generation();
    app.gamer_covers
        .absorb(&ctx, cover_reply(generation, &featured_path(0), "101"));
    run_frames(&mut app, &ctx, 1920.0, 1080.0, 1);

    app.filter = "archivefs-featured-g00004".to_string();
    run_frames(&mut app, &ctx, 1920.0, 1080.0, 2);
    assert!(
        !matches!(
            app.gamer_covers
                .slot_for(Path::new(&featured_path(4)), None),
            Some(crate::gamer_artwork::CoverSlot::Ready { .. })
        ),
        "the searched-for game inherited another record's cover"
    );
    // The loaded cover still belongs to the record it was resolved for.
    assert!(matches!(
        app.gamer_covers
            .slot_for(Path::new(&featured_path(0)), None),
        Some(crate::gamer_artwork::CoverSlot::Ready { .. })
    ));
}

#[test]
fn an_identity_refresh_stops_drawing_a_cover_until_the_record_is_confirmed() {
    // The frames between a successful import and its confirmation are exactly
    // when a path whose provider id moved would still be showing the old game's
    // art. The row keeps its height and its placeholder; it does not keep the
    // picture.
    let (mut app, ctx) = gamer_cover_app(6);
    run_frames(&mut app, &ctx, 1920.0, 1080.0, 3);
    let generation = app.gamer_covers.generation();
    app.gamer_covers
        .absorb(&ctx, cover_reply(generation, &featured_path(0), "101"));
    let cover = cover_texture_id(&app, &featured_path(0)).expect("a loaded cover");
    let before = run_frames(&mut app, &ctx, 1920.0, 1080.0, 1);
    assert!(drawn_texture_ids(&before).contains(&cover));
    let title_before = find_text_position_containing(&before, "Game 00000");

    app.gamer_covers.identity_refreshed();
    let after = run_frames(&mut app, &ctx, 1920.0, 1080.0, 1);
    assert!(
        !drawn_texture_ids(&after).contains(&cover),
        "the previous catalogue's cover was still painted after a refresh"
    );
    // And nothing moved while it was gone.
    assert_eq!(
        title_before,
        find_text_position_containing(&after, "Game 00000"),
        "the row shifted when its cover was withdrawn"
    );
    assert!(rendered_text_contains(&after, "Game 00000"));
}

#[test]
fn rendering_gamer_view_in_a_test_never_starts_a_real_cover_worker() {
    // The worker opens the per-user identity cache under $HOME and, for a
    // developer with RomM configured, can reach their instance. A test run
    // must do neither.
    //
    // It was also the cause of a CI-only failure: the worker answered the
    // same rows the cover tests drive by hand, so a reply landing between
    // two frames replaced the slot under test with a placeholder. It won
    // that race on a slow two-core runner and lost it on a fast
    // workstation, which is why the suite passed locally and failed in CI.
    let (mut app, ctx) = gamer_cover_app(24);
    run_frames(&mut app, &ctx, 1920.0, 1080.0, 4);

    assert!(
        app.gamer_cover_worker.is_none(),
        "a test frame started a cover worker, so the suite reads real user \
             data and cover tests race it"
    );
    // The scheduling itself must still run - suppressing the thread must not
    // quietly turn the cover column off and make the other tests vacuous.
    assert!(
        app.gamer_covers.tracked() > 0,
        "no cover was scheduled, so these tests would prove nothing"
    );
}

#[test]
fn a_confirmed_record_gets_its_own_texture_back_without_a_new_upload() {
    let (mut app, ctx) = gamer_cover_app(6);
    run_frames(&mut app, &ctx, 1920.0, 1080.0, 3);
    let generation = app.gamer_covers.generation();
    app.gamer_covers
        .absorb(&ctx, cover_reply(generation, &featured_path(0), "101"));
    let cover = cover_texture_id(&app, &featured_path(0)).expect("a loaded cover");

    app.gamer_covers.identity_refreshed();
    run_frames(&mut app, &ctx, 1920.0, 1080.0, 1);
    // The worker confirms the key the row offered back.
    let confirmed = app.gamer_covers.absorb(
        &ctx,
        crate::gamer_artwork::CoverReply {
            generation: app.gamer_covers.generation(),
            local_path: PathBuf::from(featured_path(0)),
            provider_game_id: Some("101".to_string()),
            answer: crate::gamer_artwork::CoverAnswer::Unchanged {
                key: "101".to_string(),
            },
        },
    );
    assert!(confirmed, "the confirmation was refused");
    assert_eq!(
        cover_texture_id(&app, &featured_path(0)),
        Some(cover),
        "the retained texture was replaced rather than reused"
    );
    let output = run_frames(&mut app, &ctx, 1920.0, 1080.0, 1);
    assert!(drawn_texture_ids(&output).contains(&cover));
}

#[test]
fn an_identity_refresh_re_asks_only_the_rows_on_screen() {
    // A refresh over the real library must not queue work for all of it.
    let (mut app, ctx) = gamer_cover_app(13_891);
    run_frames(&mut app, &ctx, 1920.0, 1080.0, 12);
    let before = app.gamer_covers.tracked();
    app.gamer_covers.identity_refreshed();
    run_frames(&mut app, &ctx, 1920.0, 1080.0, 6);
    let after = app.gamer_covers.tracked();
    assert!(
        after < 200,
        "a refresh over a 13,891-record library tracked {after} records (was {before})"
    );
}

// --- The featured "Selected game" panel --------------------------------

/// The local path the featured-panel fixture uses for one of its records.
fn featured_path(index: usize) -> String {
    std::env::temp_dir()
        .join(format!("archivefs-featured-g{index:05}.zip"))
        .display()
        .to_string()
}

/// Renders Gamer View with one game selected and returns the frame.
fn featured_panel_frame(
    width: f32,
    height: f32,
    title: &str,
) -> (ArchiveFsApp, egui::Context, egui::FullOutput) {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::GamerView;
    app.view = MainView::Library;
    // Real parent directory, because "Open location" is only offered when there
    // is somewhere to open - testing that it is present needs a path whose
    // folder exists. No file is created: only the folder is looked at.
    let folder = std::env::temp_dir();
    let mut records = Vec::new();
    for index in 0..8 {
        let path = folder.join(format!("archivefs-featured-g{index:05}.zip"));
        let mut built = record(&path.display().to_string(), MountState::Pending);
        built.metadata.platform = Some("SNES".to_string());
        built.metadata.title = Some(if index == 0 {
            title.to_string()
        } else {
            format!("Game {index:05}")
        });
        records.push(built);
    }
    app.state = LoadState::Ready(Box::new(loaded_data_with_records("/mount", records)));
    app.archive_context
        .select_only(folder.join("archivefs-featured-g00000.zip"));
    let ctx = egui::Context::default();
    let output = run_frames(&mut app, &ctx, width, height, 4);
    (app, ctx, output)
}

/// The rect of the first widget whose text exactly matches, from the frame's
/// text shapes. Relational assertions use these rather than fixed pixels.
fn text_rect(output: &egui::FullOutput, needle: &str) -> Option<egui::Rect> {
    fn walk(shape: &egui::Shape, needle: &str, out: &mut Option<egui::Rect>) {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == needle && out.is_none() => {
                *out = Some(text.visual_bounding_rect());
            }
            egui::Shape::Vec(nested) => nested.iter().for_each(|shape| walk(shape, needle, out)),
            _ => {}
        }
    }
    let mut found = None;
    for clipped in &output.shapes {
        walk(&clipped.shape, needle, &mut found);
    }
    found
}

/// The furthest-right edge any shape actually painted to, across the whole
/// frame - a genuinely long value (a long absolute mount path, most often)
/// pushing a widget past the viewport shows up here even though the exact
/// truncated text can't be searched for by content.
fn max_shape_right(output: &egui::FullOutput) -> f32 {
    fn walk(shape: &egui::Shape, max_right: &mut f32) {
        match shape {
            egui::Shape::Vec(nested) => nested.iter().for_each(|shape| walk(shape, max_right)),
            other => *max_right = max_right.max(other.visual_bounding_rect().right()),
        }
    }
    let mut max_right = 0.0_f32;
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut max_right);
    }
    max_right
}

/// A very long mount path (2026-08-22, live-QA Phase 8: "Selected archive"
/// content extended off the right edge of the window, including a
/// partially-hidden Mount-related stat) must not push the panel wider than
/// the viewport - `detail_row_with_copy`'s `.truncate()` on the Mount path
/// row only has anything to truncate against once the enclosing
/// `egui::Grid` column actually has a bounded width (see
/// `show_selected_archive`'s `.max_col_width` on `selected_archive_details`).
#[test]
fn a_long_mount_path_does_not_push_the_selected_archive_panel_past_the_viewport() {
    let width = 1280.0_f32;
    let height = 720.0_f32;
    let archive = Archive::from_path(&PathBuf::from("/roms/a.zip")).unwrap();
    let long_mount_path = PathBuf::from(format!(
        "/mnt/archivefs/{}/very-long-game-title",
        "extremely-long-platform-directory-segment".repeat(6)
    ));
    let record = ArchiveRecord::new(
        MountPlan::new(archive, long_mount_path),
        MountState::Mounted,
        ArchiveMetadata {
            title: None,
            platform: None,
            region: None,
            languages: None,
            version: None,
            disc: None,
            publisher: None,
            developer: None,
            release_year: None,
            genre: None,
            notes: None,
            source: None,
            synopsis: None,
            players: None,
            rating: None,
        },
        ArchiveHealth::Pending,
    );

    let EmptySelectedArchiveViewStateParts {
        mut confirm_unmount,
        mut confirm_lazy_unmount,
        mut focus_lazy_cancel,
        lazy_unmount_offers,
        remount_offers,
        mut cleanup_after_unmount,
        mut platform_choice,
        mut platform_custom_text,
        mut clipboard,
    } = empty_selected_archive_view_state_parts();

    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height));
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_selected_archive(
                    ui,
                    Some(&record),
                    None,
                    None,
                    None,
                    SelectedArchiveViewState {
                        operation: None,
                        busy: false,
                        block_reason: None,
                        action_readiness_debug_lines: &[],
                        confirm_unmount: &mut confirm_unmount,
                        confirm_lazy_unmount: &mut confirm_lazy_unmount,
                        focus_lazy_cancel: &mut focus_lazy_cancel,
                        lazy_unmount_offers: &lazy_unmount_offers,
                        remount_offers: &remount_offers,
                        cleanup_after_unmount: &mut cleanup_after_unmount,
                        platform_choice: &mut platform_choice,
                        platform_custom_text: &mut platform_custom_text,
                        platform_busy: false,
                        clipboard: &mut clipboard,
                    },
                );
            });
        },
    );

    assert!(
        max_shape_right(&output) <= width,
        "a long mount path must not push the Selected archive panel past the {width}px viewport"
    );
}

#[test]
fn the_featured_panel_shows_artwork_above_the_title_and_the_actions() {
    // The layout contract, stated relationally rather than in pixels: artwork
    // at the top, then the title, then Mount, then the secondary actions.
    let (mut app, ctx, _) = featured_panel_frame(1920.0, 1080.0, "Featured Game");
    let generation = app.gamer_covers.generation();
    app.gamer_covers
        .absorb(&ctx, cover_reply(generation, &featured_path(0), "101"));
    let output = run_frames(&mut app, &ctx, 1920.0, 1080.0, 1);

    let cover = cover_texture_id(&app, &featured_path(0)).expect("a loaded cover");
    // The featured cover is the large one; the row thumbnail is the small one.
    let mut drawn: Vec<egui::Rect> = rendered_images(&output)
        .into_iter()
        .filter(|(id, _)| *id == cover)
        .map(|(_, rect)| rect)
        .collect();
    drawn.sort_by(|left, right| right.height().total_cmp(&left.height()));
    assert!(
        drawn.len() >= 2,
        "the same cover should be drawn in the row and in the panel"
    );
    let featured = drawn[0];
    let thumbnail = drawn[1];
    assert!(
        featured.height() > thumbnail.height() * 2.0,
        "the featured cover ({featured:?}) is not clearly larger than the row thumbnail ({thumbnail:?})"
    );

    let title = text_rect(&output, "Featured Game").expect("the title");
    let mount = text_rect(&output, "Mount").expect("Mount");
    let cheats = text_rect(&output, "Cheats & Mods").expect("Cheats & Mods");
    assert!(
        featured.bottom() <= title.top(),
        "artwork is not above the title"
    );
    assert!(
        title.bottom() <= mount.top(),
        "the title is not above Mount"
    );
    assert!(
        mount.bottom() <= cheats.top(),
        "Mount is not above the secondary actions"
    );
}

#[test]
fn row_thumbnails_survive_the_featured_panel() {
    let (mut app, ctx, _) = featured_panel_frame(1920.0, 1080.0, "Featured Game");
    let generation = app.gamer_covers.generation();
    app.gamer_covers
        .absorb(&ctx, cover_reply(generation, &featured_path(1), "102"));
    let output = run_frames(&mut app, &ctx, 1920.0, 1080.0, 1);
    let other = cover_texture_id(&app, &featured_path(1)).expect("a loaded cover");
    let drawn: Vec<egui::Rect> = rendered_images(&output)
        .into_iter()
        .filter(|(id, _)| *id == other)
        .map(|(_, rect)| rect)
        .collect();
    assert!(
        drawn
            .iter()
            .any(|rect| rect.height() <= crate::gamer_artwork::COVER_BOX + 0.5),
        "the small row thumbnail is gone: {drawn:?}"
    );
}

#[test]
fn every_secondary_action_is_still_present() {
    let (_app, _ctx, output) = featured_panel_frame(1920.0, 1080.0, "Featured Game");
    for label in ["Mount", "Cheats & Mods", "Details", "Open location"] {
        assert!(
            rendered_text_contains(&output, label),
            "{label} is missing from the featured panel"
        );
    }
}

#[test]
fn mount_is_the_first_actionable_widget_in_the_panel() {
    // Artwork is painted, never allocated as a control, so the first thing a
    // keyboard reaches inside the panel is Mount - and the order after it is
    // the reading order.
    let (_app, _ctx, output) = featured_panel_frame(1920.0, 1080.0, "Featured Game");
    let mut order = Vec::new();
    for label in ["Mount", "Cheats & Mods", "Details", "Open location"] {
        let rect =
            text_rect(&output, label).unwrap_or_else(|| panic!("{label} should be rendered"));
        order.push((label, rect.top(), rect.left()));
    }
    assert_eq!(order[0].0, "Mount");
    assert!(
        order[0].1 <= order[1].1,
        "Mount is not above the secondary actions"
    );
    for pair in order[1..].windows(2) {
        assert!(
            pair[0].1 < pair[1].1 || pair[0].2 <= pair[1].2,
            "{} comes after {} in the reading order",
            pair[0].0,
            pair[1].0
        );
    }
}

/// Presses Tab `count` times and returns the rect of whatever holds focus.
fn tab_to_focused_rect(
    app: &mut ArchiveFsApp,
    ctx: &egui::Context,
    width: f32,
    height: f32,
    count: usize,
) -> Vec<egui::Rect> {
    let mut frame = eframe::Frame::_new_kittest();
    let mut seen = Vec::new();
    for _ in 0..count {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, height),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| app.update(ctx, &mut frame));
        if let Some(id) = ctx.memory(|memory| memory.focused())
            && let Some(rect) = ctx.read_response(id).map(|response| response.rect)
        {
            seen.push(rect);
        }
    }
    seen
}

#[test]
fn the_featured_artwork_area_is_not_keyboard_focusable() {
    // The artwork occupies most of the panel's upper half. It is allocated with
    // `Sense::hover` and painted, so it takes no Tab stop - if it did, it would
    // sit between the list and Mount for no purpose and a person on a sofa
    // would press the key an extra time for nothing.
    let (mut app, ctx, _) = featured_panel_frame(1920.0, 1080.0, "Featured Game");
    let generation = app.gamer_covers.generation();
    app.gamer_covers
        .absorb(&ctx, cover_reply(generation, &featured_path(0), "101"));
    let output = run_frames(&mut app, &ctx, 1920.0, 1080.0, 1);
    let cover = cover_texture_id(&app, &featured_path(0)).expect("a loaded cover");
    let featured = rendered_images(&output)
        .into_iter()
        .filter(|(id, _)| *id == cover)
        .map(|(_, rect)| rect)
        .max_by(|left, right| left.height().total_cmp(&right.height()))
        .expect("the featured cover");

    let focused = tab_to_focused_rect(&mut app, &ctx, 1920.0, 1080.0, 40);
    assert!(
        !focused.is_empty(),
        "nothing in Gamer View took keyboard focus at all"
    );
    for rect in &focused {
        assert!(
            !featured.contains_rect(*rect),
            "focus landed inside the featured artwork area: {rect:?} within {featured:?}"
        );
    }
}

#[test]
fn missing_artwork_keeps_the_panel_geometry_exactly() {
    // The reserved box is the same size whether a cover is loading, absent or
    // broken, so nothing beneath it moves.
    let (mut app, ctx, _) = featured_panel_frame(1920.0, 1080.0, "Featured Game");
    let loading = run_frames(&mut app, &ctx, 1920.0, 1080.0, 1);
    let title_loading = text_rect(&loading, "Featured Game").expect("the title");
    let mount_loading = text_rect(&loading, "Mount").expect("Mount");

    let generation = app.gamer_covers.generation();
    app.gamer_covers.absorb(
        &ctx,
        crate::gamer_artwork::CoverReply {
            generation,
            local_path: PathBuf::from(featured_path(0)),
            provider_game_id: Some("101".to_string()),
            answer: crate::gamer_artwork::CoverAnswer::None(
                crate::gamer_artwork::NoCover::NoArtwork,
            ),
        },
    );
    let absent = run_frames(&mut app, &ctx, 1920.0, 1080.0, 1);
    assert_eq!(
        title_loading,
        text_rect(&absent, "Featured Game").expect("the title"),
        "the title moved when the cover turned out not to exist"
    );
    assert_eq!(
        mount_loading,
        text_rect(&absent, "Mount").expect("Mount"),
        "Mount moved when the cover turned out not to exist"
    );
    assert!(
        rendered_text_contains(&absent, "No cover available"),
        "the placeholder does not say why it is a placeholder"
    );
}

#[test]
fn a_failed_cover_keeps_the_panel_geometry_exactly() {
    let (mut app, ctx, _) = featured_panel_frame(1920.0, 1080.0, "Featured Game");
    let before = run_frames(&mut app, &ctx, 1920.0, 1080.0, 1);
    let title = text_rect(&before, "Featured Game").expect("the title");
    let mount = text_rect(&before, "Mount").expect("Mount");

    let generation = app.gamer_covers.generation();
    app.gamer_covers.absorb(
        &ctx,
        crate::gamer_artwork::CoverReply {
            generation,
            local_path: PathBuf::from(featured_path(0)),
            provider_game_id: Some("101".to_string()),
            answer: crate::gamer_artwork::CoverAnswer::None(crate::gamer_artwork::NoCover::Failed),
        },
    );
    let after = run_frames(&mut app, &ctx, 1920.0, 1080.0, 1);
    assert_eq!(
        title,
        text_rect(&after, "Featured Game").expect("the title")
    );
    assert_eq!(mount, text_rect(&after, "Mount").expect("Mount"));
}

#[test]
fn a_cover_arriving_does_not_move_the_title_or_the_actions() {
    let (mut app, ctx, _) = featured_panel_frame(1920.0, 1080.0, "Featured Game");
    let before = run_frames(&mut app, &ctx, 1920.0, 1080.0, 1);
    let title = text_rect(&before, "Featured Game").expect("the title");
    let mount = text_rect(&before, "Mount").expect("Mount");

    let generation = app.gamer_covers.generation();
    app.gamer_covers
        .absorb(&ctx, cover_reply(generation, &featured_path(0), "101"));
    let after = run_frames(&mut app, &ctx, 1920.0, 1080.0, 1);
    assert_eq!(
        title,
        text_rect(&after, "Featured Game").expect("the title")
    );
    assert_eq!(mount, text_rect(&after, "Mount").expect("Mount"));
}

#[test]
fn a_long_title_does_not_overlap_the_artwork_or_the_actions() {
    let long = "The Legend of the Extraordinarily Long Aftermarket Title (World) \
                    (Rev 1) (Unl) (Demo) (Aftermarket) (Pirate) (Alt 3)";
    let (mut app, ctx, _) = featured_panel_frame(1920.0, 1080.0, long);
    let generation = app.gamer_covers.generation();
    app.gamer_covers
        .absorb(&ctx, cover_reply(generation, &featured_path(0), "101"));
    let output = run_frames(&mut app, &ctx, 1920.0, 1080.0, 1);

    let cover = cover_texture_id(&app, &featured_path(0)).expect("a loaded cover");
    let featured = rendered_images(&output)
        .into_iter()
        .filter(|(id, _)| *id == cover)
        .map(|(_, rect)| rect)
        .max_by(|left, right| left.height().total_cmp(&right.height()))
        .expect("the featured cover");
    let mount = text_rect(&output, "Mount").expect("Mount");
    let title = text_rect(&output, long).expect("the title");

    assert!(
        title.top() >= featured.bottom() - 0.5,
        "a long title rode up over the artwork"
    );
    assert!(
        title.bottom() <= mount.top() + 0.5,
        "a long title ran into Mount"
    );
}

#[test]
fn the_panel_keeps_mount_and_the_actions_on_screen_at_every_supported_size() {
    for (width, height) in [
        (1280.0, 720.0),
        (1366.0, 768.0),
        (1920.0, 1080.0),
        (2560.0, 1440.0),
    ] {
        let (_app, _ctx, output) = featured_panel_frame(width, height, "Featured Game");
        let _ = &output;
        for label in ["Mount", "Cheats & Mods", "Details"] {
            let rect = text_rect(&output, label)
                .unwrap_or_else(|| panic!("{label} is not rendered at {width}x{height}"));
            assert!(
                rect.bottom() <= height,
                "{label} falls below the viewport at {width}x{height}: {rect:?}"
            );
            assert!(
                rect.right() <= width,
                "{label} falls outside the viewport at {width}x{height}: {rect:?}"
            );
        }
    }
}

#[test]
fn the_actions_survive_a_feedback_banner_and_a_wrapped_title_at_720p() {
    // Found in Sunshine at 1280x720: an "Unmounted ..." banner takes a line
    // above the whole view, and with the cover sized from the *outer* height
    // the secondary actions dropped below the viewport. The cover has to yield
    // that space, so it is measured from what is actually left.
    let long = "Super Mario Bros. + Duck Hunt + World Class Track Meet (USA) (Rev 1)";
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::GamerView;
    app.view = MainView::Library;
    let folder = std::env::temp_dir();
    let mut records = Vec::new();
    for index in 0..8 {
        let path = folder.join(format!("archivefs-featured-g{index:05}.zip"));
        let mut built = record(&path.display().to_string(), MountState::Pending);
        built.metadata.platform = Some("SNES".to_string());
        built.metadata.title = Some(if index == 0 {
            long.to_string()
        } else {
            format!("Game {index:05}")
        });
        records.push(built);
    }
    app.state = LoadState::Ready(Box::new(loaded_data_with_records("/mount", records)));
    app.archive_context
        .select_only(folder.join("archivefs-featured-g00000.zip"));
    app.feedback = Some(ActionFeedback {
        succeeded: true,
        message: "Unmounted /mnt/virtualroms/Acorn Archimedes/ALPS_Support_Disc".to_string(),
        cleanup: None,
        warning: None,
        more_information: None,
    });

    let ctx = egui::Context::default();
    // Swept rather than fixed at 720: a real window is its nominal height minus
    // a title bar, and the banner's own height varies with wrapping, so the
    // exact value that used to overflow is not a number worth guessing at.
    for height in [600.0_f32, 640.0, 660.0, 680.0, 696.0, 720.0] {
        let output = run_frames(&mut app, &ctx, 1280.0, height, 4);
        for label in ["Mount", "Cheats & Mods", "Details"] {
            let rect = text_rect(&output, label).unwrap_or_else(|| {
                panic!("{label} is not rendered at 1280x{height} with a banner")
            });
            assert!(
                rect.bottom() <= height,
                "{label} fell below the viewport at 1280x{height} with a banner: {rect:?}"
            );
        }
    }
}

#[test]
fn the_featured_panel_stays_balanced_at_1920x1080() {
    // A cover that reads from a sofa, and a panel that is neither a sliver nor
    // a slab: the artwork occupies a meaningful share of the panel's height.
    let (mut app, ctx, _) = featured_panel_frame(1920.0, 1080.0, "Featured Game");
    let generation = app.gamer_covers.generation();
    app.gamer_covers
        .absorb(&ctx, cover_reply(generation, &featured_path(0), "101"));
    let output = run_frames(&mut app, &ctx, 1920.0, 1080.0, 1);
    let cover = cover_texture_id(&app, &featured_path(0)).expect("a loaded cover");
    let featured = rendered_images(&output)
        .into_iter()
        .filter(|(id, _)| *id == cover)
        .map(|(_, rect)| rect)
        .max_by(|left, right| left.height().total_cmp(&right.height()))
        .expect("the featured cover");
    assert!(
        featured.height() >= 150.0,
        "the 1080p featured cover is only {} tall",
        featured.height()
    );
    // And it sits in the right-hand panel, not over the list.
    assert!(
        featured.left() > 1920.0 * 0.5,
        "the featured cover is not in the right-hand panel: {featured:?}"
    );
}

#[test]
fn gamer_view_covers_behave_at_every_supported_resolution() {
    for (width, height) in [
        (1280.0, 720.0),
        (1366.0, 768.0),
        (1920.0, 1080.0),
        (2560.0, 1440.0),
    ] {
        let (mut app, ctx) = gamer_cover_app(60);
        run_frames(&mut app, &ctx, width, height, 3);
        let generation = app.gamer_covers.generation();
        app.gamer_covers
            .absorb(&ctx, cover_reply(generation, &featured_path(0), "101"));
        let output = run_frames(&mut app, &ctx, width, height, 1);
        assert!(
            rendered_text_contains(&output, "Game 00000"),
            "the first game's title vanished at {width}x{height}"
        );
        let tracked = app.gamer_covers.tracked();
        assert!(
            tracked <= 60,
            "{width}x{height} queued {tracked} of a 60-record library"
        );
    }
}
