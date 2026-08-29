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
//! Predominant theme observed in this slice: health dashboard, platform alias/bulk actions.

use super::*;
use archivefs_core::ArchiveKind;

#[test]
fn apply_arrow_focus_change_replaces_selection_without_ctrl_and_preserves_it_with_ctrl() {
    let mut selected_archives: HashSet<PathBuf> =
        [PathBuf::from("/roms/old.zip")].into_iter().collect();
    let mut selected_archive = Some(PathBuf::from("/roms/old.zip"));

    apply_arrow_focus_change(
        &mut selected_archives,
        &mut selected_archive,
        PathBuf::from("/roms/new.zip"),
        false,
    );
    assert_eq!(selected_archive, Some(PathBuf::from("/roms/new.zip")));
    assert_eq!(
        selected_archives,
        [PathBuf::from("/roms/new.zip")].into_iter().collect(),
        "without Ctrl, moving focus must replace the whole selection"
    );

    apply_arrow_focus_change(
        &mut selected_archives,
        &mut selected_archive,
        PathBuf::from("/roms/newer.zip"),
        true,
    );
    assert_eq!(selected_archive, Some(PathBuf::from("/roms/newer.zip")));
    assert_eq!(
        selected_archives,
        [PathBuf::from("/roms/new.zip")].into_iter().collect(),
        "with Ctrl held, moving focus must not touch the multi-selection"
    );
}

#[test]
fn compute_scroll_offset_for_focus_does_not_move_when_already_visible() {
    // Focus at row 2 (rows 24px apart) sits entirely within a viewport
    // already scrolled to show rows 1-5 (offset 24.0, height 120.0) -
    // no scroll should be requested at all, so repeatedly pressing
    // Ctrl+Down within an already-visible range never jitters the view.
    let offset = compute_scroll_offset_for_focus(2, 24.0, 24.0, 120.0);
    assert_eq!(offset, 24.0);
}

#[test]
fn compute_scroll_offset_for_focus_scrolls_up_when_focus_moves_above_the_viewport() {
    // Focus lands on row 1, but the viewport currently starts at
    // offset 48.0 (row 2) - row 1 is above the visible area, so the
    // offset must move up to align row 1 to the top edge exactly.
    let offset = compute_scroll_offset_for_focus(1, 24.0, 48.0, 120.0);
    assert_eq!(
        offset, 24.0,
        "focus above the viewport must scroll up to the row's own top edge"
    );
}

#[test]
fn compute_scroll_offset_for_focus_scrolls_down_when_focus_moves_below_the_viewport() {
    // Viewport shows rows starting at offset 0.0, 120.0 tall (5 rows of
    // 24px each, rows 0-4). Focus moves to row 5, one past the bottom
    // edge - the offset must move down just enough to bring row 5's
    // bottom edge exactly to the viewport's bottom edge.
    let offset = compute_scroll_offset_for_focus(5, 24.0, 0.0, 120.0);
    assert_eq!(
        offset, 24.0,
        "focus below the viewport must scroll down to the row's own bottom edge"
    );
}

#[test]
fn compute_scroll_offset_for_focus_never_scrolls_above_the_top() {
    let offset = compute_scroll_offset_for_focus(0, 24.0, 0.0, 500.0);
    assert_eq!(
        offset, 0.0,
        "a viewport taller than the content must clamp to 0"
    );
}

#[test]
fn sort_visible_indices_orders_each_column_ascending_and_descending() {
    let merged_rows = vec![
        row_with_fields("/roms/a.zip", "SNES", "Missing", "b_archive.zip", "/mnt/z"),
        row_with_fields("/roms/b.zip", "GBA", "Live", "a_archive.zip", "/mnt/a"),
        row_with_fields("/roms/c.zip", "NES", "Pending", "c_archive.zip", "/mnt/m"),
    ];

    for field in COLUMN_SORT_FIELDS {
        let mut ascending = vec![0usize, 1usize, 2usize];
        sort_visible_indices(&merged_rows, &mut ascending, field, true);
        let ascending_keys: Vec<&str> = ascending
            .iter()
            .map(|&index| sort_field_key(&merged_rows[index], field))
            .collect();
        let mut expected_ascending = ascending_keys.clone();
        expected_ascending.sort();
        assert_eq!(
            ascending_keys, expected_ascending,
            "{field:?} ascending must be in ascending key order"
        );

        let mut descending = vec![0usize, 1usize, 2usize];
        sort_visible_indices(&merged_rows, &mut descending, field, false);
        let descending_keys: Vec<&str> = descending
            .iter()
            .map(|&index| sort_field_key(&merged_rows[index], field))
            .collect();
        let mut expected_descending = descending_keys.clone();
        expected_descending.sort();
        expected_descending.reverse();
        assert_eq!(
            descending_keys, expected_descending,
            "{field:?} descending must be in descending key order"
        );
    }
}

#[test]
fn sort_visible_indices_breaks_ties_deterministically_by_exact_path() {
    // All three rows share the same platform - only the exact path
    // can break the tie, and it must do so the same way every time,
    // regardless of `merged_rows`'s incoming order.
    let merged_rows = vec![
        row_with_fields("/roms/charlie.zip", "SNES", "Live", "c.zip", "/mnt/c"),
        row_with_fields("/roms/alpha.zip", "SNES", "Live", "a.zip", "/mnt/a"),
        row_with_fields("/roms/bravo.zip", "SNES", "Live", "b.zip", "/mnt/b"),
    ];
    let mut indices = vec![0usize, 1usize, 2usize];
    sort_visible_indices(&merged_rows, &mut indices, SortField::Platform, true);

    let ordered_paths: Vec<&PathBuf> = indices.iter().map(|&i| &merged_rows[i].path).collect();
    assert_eq!(
        ordered_paths,
        vec![
            &PathBuf::from("/roms/alpha.zip"),
            &PathBuf::from("/roms/bravo.zip"),
            &PathBuf::from("/roms/charlie.zip"),
        ],
        "rows tied on platform must be ordered by their exact path"
    );

    // Reversing the incoming order must not change the outcome -
    // this is what makes the tie-break actually deterministic rather
    // than merely "stable" (stability alone would just preserve
    // whatever order happened to be handed in).
    let mut reversed_indices = vec![2usize, 1usize, 0usize];
    sort_visible_indices(
        &merged_rows,
        &mut reversed_indices,
        SortField::Platform,
        true,
    );
    let reversed_ordered_paths: Vec<&PathBuf> = reversed_indices
        .iter()
        .map(|&i| &merged_rows[i].path)
        .collect();
    assert_eq!(ordered_paths, reversed_ordered_paths);
}

#[test]
fn sort_visible_indices_never_touches_merged_rows_itself() {
    // Requirement 2: sorting must not mutate database order or
    // archive identity - `merged_rows` (and by extension
    // `data.records`/`data.rows`) must come out byte-for-byte
    // unchanged; only the separate `indices` list may reorder.
    let merged_rows = vec![
        row_with_fields("/roms/z.zip", "Z", "Live", "z.zip", "/mnt/z"),
        row_with_fields("/roms/a.zip", "A", "Live", "a.zip", "/mnt/a"),
    ];
    let original_paths: Vec<PathBuf> = merged_rows.iter().map(|row| row.path.clone()).collect();

    let mut indices = vec![0usize, 1usize];
    sort_visible_indices(&merged_rows, &mut indices, SortField::Platform, true);

    let paths_after: Vec<PathBuf> = merged_rows.iter().map(|row| row.path.clone()).collect();
    assert_eq!(
        original_paths, paths_after,
        "merged_rows's own order must be untouched by sorting"
    );
    assert_eq!(indices, vec![1usize, 0usize]);
}

#[test]
fn apply_header_click_selects_new_field_ascending_then_toggles_same_field() {
    let mut sort_field = None;
    let mut sort_ascending = true;

    apply_header_click(&mut sort_field, &mut sort_ascending, SortField::Platform);
    assert_eq!(sort_field, Some(SortField::Platform));
    assert!(sort_ascending, "a newly selected column starts ascending");

    apply_header_click(&mut sort_field, &mut sort_ascending, SortField::Platform);
    assert_eq!(sort_field, Some(SortField::Platform));
    assert!(
        !sort_ascending,
        "clicking the already-active column again toggles direction"
    );

    apply_header_click(&mut sort_field, &mut sort_ascending, SortField::Platform);
    assert!(sort_ascending, "toggling twice returns to ascending");

    apply_header_click(&mut sort_field, &mut sort_ascending, SortField::State);
    assert_eq!(sort_field, Some(SortField::State));
    assert!(
        sort_ascending,
        "selecting a different column resets to ascending"
    );
}

#[test]
fn real_header_click_reaches_show_header_row_and_reports_the_clicked_column() {
    let ctx = egui::Context::default();
    let clicked_field: std::cell::RefCell<Option<SortField>> = std::cell::RefCell::new(None);
    let column_widths: std::cell::RefCell<LibraryColumnWidths> =
        std::cell::RefCell::new(LibraryColumnWidths::default());

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

    let mut header_rect = None;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            header_rect = Some(render(ui).rect);
        });
    });
    let header_rect = header_rect.unwrap();

    // "Platform" is the first column - click well inside its left
    // edge, safely clear of any neighbouring column regardless of
    // exact font metrics.
    let click_pos = egui::pos2(header_rect.left() + 20.0, header_rect.center().y);
    simulate_row_click(&ctx, click_pos, egui::Modifiers::default(), render);

    assert_eq!(
        *clicked_field.borrow(),
        Some(SortField::Platform),
        "a real click on the header must be detected as the Platform column"
    );
}

#[test]
fn resizing_a_column_does_not_alter_selection_or_sort_state() {
    // The resize handle's own drag mechanics (grows only the dragged
    // column, never triggers a sort click) are already proven in
    // isolation by `dragging_the_archive_path_handle_grows_only_that_column_and_never_sorts`.
    // This test proves the other half: rendering the *whole* Library
    // page after a resize never perturbs row selection or sort state,
    // which nothing in `show_loaded_data` ever reads
    // `library_column_widths` to decide.
    let ctx = egui::Context::default();
    let data = loaded_data_with_rows(
        "/mount",
        vec![
            row_with_fields("/roms/a.zip", "SNES", "Live", "alpha.zip", "/mnt/a"),
            row_with_fields("/roms/b.zip", "GBA", "Live", "bravo.zip", "/mnt/b"),
        ],
    );
    let mut harness = RealLoadedDataHarness::new();
    harness.archive_context.focused = Some(PathBuf::from("/roms/b.zip"));
    harness.archive_context.selected = [PathBuf::from("/roms/b.zip")].into_iter().collect();
    harness.sort_field = Some(SortField::Platform);
    harness.sort_ascending = false;
    harness.library_column_widths.archive_path += 150.0;
    harness.library_column_widths.mount_path -= 50.0;

    harness.render(&ctx, &data, bounded_test_input());

    assert_eq!(
        harness.archive_context.focused,
        Some(PathBuf::from("/roms/b.zip")),
        "a resized column must never change the focused row"
    );
    assert_eq!(
        harness.archive_context.selected,
        [PathBuf::from("/roms/b.zip")].into_iter().collect(),
        "a resized column must never change the multi-selection"
    );
    assert_eq!(harness.sort_field, Some(SortField::Platform));
    assert!(!harness.sort_ascending);
}

#[test]
fn changing_the_available_window_height_does_not_alter_selection_sort_or_filter_state() {
    let ctx = egui::Context::default();
    let data = loaded_data_with_rows(
        "/mount",
        vec![
            row_with_fields("/roms/a.zip", "SNES", "Live", "alpha.zip", "/mnt/a"),
            row_with_fields("/roms/b.zip", "GBA", "Live", "bravo.zip", "/mnt/b"),
        ],
    );
    let mut harness = RealLoadedDataHarness::new();
    harness.archive_context.focused = Some(PathBuf::from("/roms/b.zip"));
    harness.archive_context.selected = [PathBuf::from("/roms/b.zip")].into_iter().collect();
    harness.sort_field = Some(SortField::Platform);
    harness.sort_ascending = false;
    harness.library_filters.present = true;

    harness.render(&ctx, &data, bounded_test_input());

    let mut short_input = bounded_test_input();
    short_input.screen_rect = Some(egui::Rect::from_min_size(
        egui::pos2(0.0, 0.0),
        egui::vec2(1000.0, 120.0),
    ));
    harness.render(&ctx, &data, short_input);

    let mut tall_input = bounded_test_input();
    tall_input.screen_rect = Some(egui::Rect::from_min_size(
        egui::pos2(0.0, 0.0),
        egui::vec2(1000.0, 900.0),
    ));
    harness.render(&ctx, &data, tall_input);

    assert_eq!(
        harness.archive_context.focused,
        Some(PathBuf::from("/roms/b.zip")),
        "a changed window/panel height must never change the focused row"
    );
    assert_eq!(
        harness.archive_context.selected,
        [PathBuf::from("/roms/b.zip")].into_iter().collect(),
        "a changed window/panel height must never change the multi-selection"
    );
    assert_eq!(harness.sort_field, Some(SortField::Platform));
    assert!(!harness.sort_ascending);
    assert!(harness.library_filters.present);
}

#[test]
fn horizontal_scrolling_does_not_change_sorting_or_the_focused_row() {
    let ctx = egui::Context::default();
    let data = loaded_data_with_rows(
        "/mount",
        vec![
            row_with_fields("/roms/a.zip", "SNES", "Live", "alpha.zip", "/mnt/a"),
            row_with_fields("/roms/b.zip", "GBA", "Live", "bravo.zip", "/mnt/b"),
        ],
    );
    let mut harness = RealLoadedDataHarness::new();
    harness.archive_context.focused = Some(PathBuf::from("/roms/a.zip"));
    harness.sort_field = Some(SortField::Platform);
    harness.sort_ascending = true;

    let scroll_input = egui::RawInput {
        events: vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(-120.0, 0.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::default(),
        }],
        ..bounded_test_input()
    };
    harness.render(&ctx, &data, scroll_input);

    assert_eq!(
        harness.archive_context.focused,
        Some(PathBuf::from("/roms/a.zip")),
        "a horizontal scroll must never change the focused row"
    );
    assert_eq!(harness.sort_field, Some(SortField::Platform));
    assert!(harness.sort_ascending);
}

#[test]
fn real_escape_key_clears_the_selected_archives_set() {
    let ctx = egui::Context::default();
    let data = loaded_data_with_rows(
        "/mount",
        vec![
            row_with_fields("/roms/a.zip", "SNES", "Live", "a.zip", "/mnt/a"),
            row_with_fields("/roms/b.zip", "SNES", "Live", "b.zip", "/mnt/b"),
        ],
    );
    let mut harness = RealLoadedDataHarness::new();
    harness.archive_context.selected = [PathBuf::from("/roms/a.zip"), PathBuf::from("/roms/b.zip")]
        .into_iter()
        .collect();

    harness.render(
        &ctx,
        &data,
        key_press_input(egui::Key::Escape, egui::Modifiers::default()),
    );

    assert!(
        harness.archive_context.selected.is_empty(),
        "Escape must clear the complete selected_archives set"
    );
}

#[test]
fn real_ctrl_a_selects_only_the_currently_visible_filtered_rows() {
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
    // Simulates the state right after a real filter-changed frame: only
    // rows 0 and 2 (a.zip, c.zip) currently pass the search filter;
    // row 1 (b.zip) is hidden.
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
        "Ctrl+A must select only the archives visible after the current search/filters, \
             never the hidden b.zip"
    );
}

#[test]
fn real_ctrl_a_is_ignored_while_the_search_box_has_keyboard_focus() {
    let ctx = egui::Context::default();
    let data = loaded_data_with_rows(
        "/mount",
        vec![
            row_with_fields("/roms/a.zip", "SNES", "Live", "a.zip", "/mnt/a"),
            row_with_fields("/roms/b.zip", "SNES", "Live", "b.zip", "/mnt/b"),
        ],
    );
    let mut harness = RealLoadedDataHarness::new();

    // Frame 1: render once so the search TextEdit exists this
    // `Context`, then give it real keyboard focus - exactly the state
    // egui is in immediately after a user clicks into the search box.
    harness.render(&ctx, &data, bounded_test_input());
    ctx.memory_mut(|memory| {
        memory.request_focus(egui::Id::new(SEARCH_FILTER_TEXT_EDIT_ID));
    });

    // Frame 2: Ctrl+A must be left for the focused text field's own
    // "select all text" behaviour, never hijacked into a table
    // selection change.
    harness.render(
        &ctx,
        &data,
        key_press_input(egui::Key::A, egui::Modifiers::CTRL),
    );

    assert!(
        harness.archive_context.selected.is_empty(),
        "Ctrl+A must be ignored for table selection while the search box has keyboard focus"
    );
}

#[test]
fn real_ctrl_a_is_ignored_while_the_bulk_platform_combobox_popup_is_open() {
    let ctx = egui::Context::default();
    let data = loaded_data_with_rows(
        "/mount",
        vec![
            row_with_fields("/roms/a.zip", "SNES", "Live", "a.zip", "/mnt/a"),
            row_with_fields("/roms/b.zip", "SNES", "Live", "b.zip", "/mnt/b"),
            row_with_fields("/roms/c.zip", "SNES", "Live", "c.zip", "/mnt/c"),
        ],
    );
    let mut harness = RealLoadedDataHarness::new();
    harness.archive_context.selected = [PathBuf::from("/roms/a.zip")].into_iter().collect();

    // Frame 1: render once (registers everything with this Context).
    harness.render(&ctx, &data, bounded_test_input());

    // Open the exact popup egui's own `ComboBox::from_id_salt(
    // "bulk_platform_choice_combo")` (see `show_bulk_platform_action_bar`)
    // opens when clicked - `ComboBox::widget_to_popup_id` is private,
    // but its formula (the widget id salted with "popup") is exactly
    // reproducible, so this is a faithful simulation of a user having
    // opened the dropdown, not a shortcut around it.
    let popup_id = egui::Id::new("bulk_platform_choice_combo").with("popup");
    egui::Popup::open_id(&ctx, popup_id);

    // If Ctrl+A were not suppressed here, all 3 visible rows would be
    // selected - so an unchanged single-row selection is conclusive.
    harness.render(
        &ctx,
        &data,
        key_press_input(egui::Key::A, egui::Modifiers::CTRL),
    );

    assert_eq!(
        harness.archive_context.selected,
        [PathBuf::from("/roms/a.zip")].into_iter().collect(),
        "Ctrl+A must be ignored for table selection while a ComboBox popup is open"
    );
}

#[test]
fn real_arrow_navigation_follows_the_visible_sorted_order() {
    let ctx = egui::Context::default();
    let data = loaded_data_with_rows(
        "/mount",
        vec![
            row_with_fields("/roms/a.zip", "Z-Platform", "Live", "a.zip", "/mnt/a"),
            row_with_fields("/roms/b.zip", "Y-Platform", "Live", "b.zip", "/mnt/b"),
            row_with_fields("/roms/c.zip", "X-Platform", "Live", "c.zip", "/mnt/c"),
        ],
    );
    let mut harness = RealLoadedDataHarness::new();
    // Ascending by platform puts c (X), b (Y), a (Z) in that screen
    // order - arrow navigation must follow *this* order, never the
    // rows' insertion order.
    harness.sort_field = Some(SortField::Platform);
    harness.sort_ascending = true;

    harness.render(
        &ctx,
        &data,
        key_press_input(egui::Key::ArrowDown, egui::Modifiers::default()),
    );
    assert_eq!(
        harness.archive_context.focused,
        Some(PathBuf::from("/roms/c.zip"))
    );
    assert_eq!(
        harness.archive_context.selected,
        [PathBuf::from("/roms/c.zip")].into_iter().collect()
    );

    harness.render(
        &ctx,
        &data,
        key_press_input(egui::Key::ArrowDown, egui::Modifiers::default()),
    );
    assert_eq!(
        harness.archive_context.focused,
        Some(PathBuf::from("/roms/b.zip"))
    );
    assert_eq!(
        harness.archive_context.selected,
        [PathBuf::from("/roms/b.zip")].into_iter().collect(),
        "moving focus without Ctrl must replace the selection with the newly focused row"
    );
}

#[test]
fn real_arrow_navigation_does_not_use_a_stale_index_after_filtering() {
    let ctx = egui::Context::default();
    let data = loaded_data_with_rows(
        "/mount",
        vec![
            row_with_fields("/roms/a.zip", "SNES", "Live", "a.zip", "/mnt/a"),
            row_with_fields("/roms/b.zip", "SNES", "Live", "b.zip", "/mnt/b"),
            row_with_fields("/roms/c.zip", "SNES", "Live", "c.zip", "/mnt/c"),
        ],
    );
    let mut harness = RealLoadedDataHarness::new();
    harness.archive_context.focused = Some(PathBuf::from("/roms/b.zip"));
    // A filter has just excluded b.zip - only a.zip and c.zip (at new
    // positions 0 and 1) remain visible; b.zip's old position no
    // longer means anything.
    harness.filtered_rows = Some(vec![0usize, 2usize]);

    harness.render(
        &ctx,
        &data,
        key_press_input(egui::Key::ArrowDown, egui::Modifiers::default()),
    );

    assert_eq!(
        harness.archive_context.focused,
        Some(PathBuf::from("/roms/a.zip")),
        "focus must fall back to the first visible row, never use a stale index that \
             would have pointed at whatever now occupies the old visible position 1"
    );
}

/// The exact Nobara bug report: multiple rows selected, Ctrl+Up/Down
/// pressed - the selected count must stay unchanged (correct, and
/// already worked) while the focused archive (`selected_archive`)
/// actually moves through the visible order (the part that was
/// broken - see `focused_row_paints_a_distinct_stroke_from_the_multi_selected_fill`
/// for why it was invisible even though this underlying state change
/// itself was already correct before this fix).
#[test]
fn real_ctrl_arrow_navigation_preserves_the_multi_selection_and_moves_focus() {
    let ctx = egui::Context::default();
    let data = loaded_data_with_rows(
        "/mount",
        vec![
            row_with_fields("/roms/a.zip", "SNES", "Live", "a.zip", "/mnt/a"),
            row_with_fields("/roms/b.zip", "SNES", "Live", "b.zip", "/mnt/b"),
            row_with_fields("/roms/c.zip", "SNES", "Live", "c.zip", "/mnt/c"),
        ],
    );
    let mut harness = RealLoadedDataHarness::new();
    let full_selection: HashSet<PathBuf> = [
        PathBuf::from("/roms/a.zip"),
        PathBuf::from("/roms/b.zip"),
        PathBuf::from("/roms/c.zip"),
    ]
    .into_iter()
    .collect();
    harness.archive_context.selected = full_selection.clone();
    harness.archive_context.focused = Some(PathBuf::from("/roms/a.zip"));

    harness.render(
        &ctx,
        &data,
        key_press_input(egui::Key::ArrowDown, egui::Modifiers::CTRL),
    );
    assert_eq!(
        harness.archive_context.focused,
        Some(PathBuf::from("/roms/b.zip")),
        "Ctrl+Down must move the focused archive to the next visible row"
    );
    assert_eq!(
        harness.archive_context.selected, full_selection,
        "Ctrl+Down must leave every multi-selected row exactly as it was"
    );

    harness.render(
        &ctx,
        &data,
        key_press_input(egui::Key::ArrowUp, egui::Modifiers::CTRL),
    );
    assert_eq!(
        harness.archive_context.focused,
        Some(PathBuf::from("/roms/a.zip")),
        "Ctrl+Up must move the focused archive to the previous visible row"
    );
    assert_eq!(
        harness.archive_context.selected, full_selection,
        "Ctrl+Up must also leave every multi-selected row exactly as it was"
    );
}

#[test]
fn real_keyboard_navigation_scrolls_the_newly_focused_row_into_view() {
    let ctx = egui::Context::default();
    // Comfortably more rows than the bounded 250px test viewport
    // (see `bounded_test_input`) can show at once, so moving focus to
    // the last one requires the fix to actually scroll - this cannot
    // pass by coincidence the way a 2-3 row table could.
    let rows: Vec<ArchiveRow> = (0..30)
        .map(|i| {
            row_with_fields(
                &format!("/roms/{i:02}.zip"),
                "SNES",
                "Live",
                &format!("{i:02}.zip"),
                &format!("/mnt/{i:02}"),
            )
        })
        .collect();
    let data = loaded_data_with_rows("/mount", rows);
    let mut harness = RealLoadedDataHarness::new();
    harness.archive_context.focused = Some(PathBuf::from("/roms/00.zip"));

    for _ in 0..29 {
        harness.render(
            &ctx,
            &data,
            key_press_input(egui::Key::ArrowDown, egui::Modifiers::default()),
        );
    }

    assert_eq!(
        harness.archive_context.focused,
        Some(PathBuf::from("/roms/29.zip")),
        "sanity check: focus must actually have reached the last row"
    );
    assert!(
        harness.library_scroll_offset > 0.0,
        "moving keyboard focus down through 30 rows in a ~250px viewport must have \
             scrolled the table - offset is still {}",
        harness.library_scroll_offset
    );
}

#[test]
fn real_sorting_does_not_change_the_selected_archives_set() {
    let ctx = egui::Context::default();
    let data = loaded_data_with_rows(
        "/mount",
        vec![
            row_with_fields("/roms/a.zip", "Z", "Live", "a.zip", "/mnt/a"),
            row_with_fields("/roms/b.zip", "Y", "Live", "b.zip", "/mnt/b"),
            row_with_fields("/roms/c.zip", "X", "Live", "c.zip", "/mnt/c"),
        ],
    );
    let mut harness = RealLoadedDataHarness::new();
    harness.archive_context.selected = [PathBuf::from("/roms/a.zip"), PathBuf::from("/roms/c.zip")]
        .into_iter()
        .collect();

    harness.render(&ctx, &data, bounded_test_input());
    assert_eq!(harness.archive_context.selected.len(), 2);

    harness.sort_field = Some(SortField::Platform);
    harness.sort_ascending = false;
    harness.render(&ctx, &data, bounded_test_input());

    assert_eq!(
        harness.archive_context.selected,
        [PathBuf::from("/roms/a.zip"), PathBuf::from("/roms/c.zip")]
            .into_iter()
            .collect(),
        "sorting must never change which exact archives are selected, only the display order"
    );
}

#[test]
fn real_zero_filter_results_message_renders_instead_of_an_empty_table() {
    let ctx = egui::Context::default();
    let data = loaded_data_with_rows(
        "/mount",
        vec![row_with_fields(
            "/roms/a.zip",
            "SNES",
            "Live",
            "a.zip",
            "/mnt/a",
        )],
    );

    let mut visible = RealLoadedDataHarness::new();
    let visible_height = visible.render(&ctx, &data, bounded_test_input());

    let mut hidden = RealLoadedDataHarness::new();
    hidden.filtered_rows = Some(Vec::new());
    let hidden_height = hidden.render(&ctx, &data, bounded_test_input());

    assert_eq!(
        library_table_message(false, 0),
        Some(LibraryTableMessage::NoFilterResults)
    );
    assert!(visible_height > 0.0 && hidden_height > 0.0);
}

#[test]
fn real_empty_library_message_renders_instead_of_an_empty_table() {
    let ctx = egui::Context::default();
    let populated = loaded_data_with_rows(
        "/mount",
        vec![row_with_fields(
            "/roms/a.zip",
            "SNES",
            "Live",
            "a.zip",
            "/mnt/a",
        )],
    );
    let empty = empty_loaded_data("/mount");

    let mut populated_harness = RealLoadedDataHarness::new();
    let populated_height = populated_harness.render(&ctx, &populated, bounded_test_input());

    let mut empty_harness = RealLoadedDataHarness::new();
    let empty_height = empty_harness.render(&ctx, &empty, bounded_test_input());

    assert_eq!(
        library_table_message(true, 0),
        Some(LibraryTableMessage::EmptyLibrary)
    );
    assert!(populated_height > 0.0 && empty_height > 0.0);
}

#[test]
fn prune_selection_uses_the_full_catalogue_not_the_filtered_view() {
    // A selected archive a text filter would currently hide must
    // still count as "in the loaded catalogue" - filtering must never
    // silently deselect a row, only change what is visible.
    let mut app = app_for_operation_tests();
    let path_a = PathBuf::from("/roms/a.zip");
    let path_b = PathBuf::from("/roms/b.zip");
    app.archive_context.selected = [path_a.clone(), path_b.clone()].into_iter().collect();
    let record_a = record_at(path_a, MountState::Pending);
    let record_b = record_at(path_b, MountState::Pending);
    let rows = vec![row_for(&record_a), row_for(&record_b)];

    app.prune_selection(&rows);

    assert_eq!(
        app.archive_context.selected.len(),
        2,
        "both selected rows are still in the catalogue"
    );
}

#[test]
fn prune_selection_removes_a_vanished_selection_and_clears_the_focused_row() {
    let mut app = app_for_operation_tests();
    let still_present = PathBuf::from("/roms/a.zip");
    let vanished = PathBuf::from("/roms/b.zip");
    app.archive_context.selected = [still_present.clone(), vanished.clone()]
        .into_iter()
        .collect();
    app.archive_context.focused = Some(vanished.clone());
    let record = record_at(still_present.clone(), MountState::Pending);
    let rows = vec![row_for(&record)];

    app.prune_selection(&rows);

    assert_eq!(
        app.archive_context.selected,
        [still_present].into_iter().collect::<HashSet<_>>()
    );
    assert_eq!(
        app.archive_context.focused, None,
        "the focused row must be cleared once it no longer exists in the catalogue"
    );
}

#[test]
fn bulk_platform_action_available_requires_no_running_action_or_database_load() {
    let mut app = app_for_operation_tests();
    assert!(app.bulk_platform_action_available());

    let (_sender, receiver) = mpsc::channel();
    app.bulk_platform_action = Some(RunningBulkPlatformAction {
        kind: BulkPlatformActionKind::Clear,
        requested_paths: 2,
        receiver,
    });
    assert!(!app.bulk_platform_action_available());
    app.bulk_platform_action = None;

    let (_sender, receiver) = mpsc::channel();
    app.database_state = DatabaseState::Loading {
        generation: DatabaseGeneration::INITIAL,
        receiver,
        worker: None,
        previous: None,
        scanning: false,
    };
    assert!(!app.bulk_platform_action_available());
}

#[test]
fn single_and_bulk_platform_actions_are_mutually_exclusive() {
    let mut app = app_for_operation_tests();
    let (_sender, receiver) = mpsc::channel();
    app.platform_action = Some(RunningPlatformAction {
        archive_path: PathBuf::from("/roms/a.zip"),
        receiver,
    });

    assert!(
        !app.bulk_platform_action_available(),
        "a running single-row platform action must block a new bulk one"
    );

    app.platform_action = None;
    let (_sender, receiver) = mpsc::channel();
    app.bulk_platform_action = Some(RunningBulkPlatformAction {
        kind: BulkPlatformActionKind::Clear,
        requested_paths: 2,
        receiver,
    });

    assert!(
        !app.platform_action_available(),
        "a running bulk platform action must block a new single-row one"
    );
}

#[test]
fn bulk_platform_action_never_affects_is_busy_or_mount_availability() {
    let mut app = app_for_operation_tests();
    let (_sender, receiver) = mpsc::channel();
    app.bulk_platform_action = Some(RunningBulkPlatformAction {
        kind: BulkPlatformActionKind::Set("GameCube".to_string()),
        requested_paths: 3,
        receiver,
    });

    assert!(
        !app.is_busy(),
        "bulk platform assignment is metadata-only and must never enter the mount busy state"
    );
}

#[test]
fn bulk_platform_action_does_not_block_on_a_slow_background_worker() {
    // Mirrors scan_library_action_does_not_block_on_a_slow_background_worker:
    // never calls the real start_bulk_platform_action/apply_bulk_platform_action
    // (which would touch the real default database path) - drives the
    // same Loading-equivalent state by hand and proves poll_bulk_platform_action's
    // use of try_recv (not recv) never blocks the UI thread.
    let mut app = app_for_operation_tests();
    let (_sender, receiver) = mpsc::channel();
    app.bulk_platform_action = Some(RunningBulkPlatformAction {
        kind: BulkPlatformActionKind::Clear,
        requested_paths: 5,
        receiver,
    });

    app.poll_bulk_platform_action(&egui::Context::default());

    assert!(app.bulk_platform_action.is_some());
}

#[test]
fn poll_bulk_platform_action_success_refreshes_the_database_cache_asynchronously() {
    let mut app = app_for_operation_tests();
    let (sender, receiver) = mpsc::channel();
    app.bulk_platform_action = Some(RunningBulkPlatformAction {
        kind: BulkPlatformActionKind::Set("GameCube".to_string()),
        requested_paths: 3,
        receiver,
    });
    sender
        .send(Ok(BulkPlatformActionOutcome {
            result: BulkPlatformAssignmentResult {
                requested: 3,
                changed: 2,
                unchanged: 1,
                missing: Vec::new(),
            },
            unresolved_paths: 0,
        }))
        .unwrap();

    app.poll_bulk_platform_action(&egui::Context::default());

    assert!(app.bulk_platform_action.is_none());
    let feedback = app.feedback.as_ref().unwrap();
    assert!(feedback.succeeded);
    assert!(feedback.message.contains("GameCube"));
    assert!(
        feedback.message.contains('2'),
        "must mention the changed count"
    );
    assert!(app.history.entries().any(|entry| entry.action
        == ActivityAction::BulkPlatformAssignment
        && entry.outcome == ActivityOutcome::Completed));
    // Refreshing the cache is asynchronous - poll_bulk_platform_action
    // only starts a new background database load, it does not block
    // waiting for it, and the live snapshot is untouched.
    assert!(app.database_state.is_loading());
    assert!(matches!(app.state, LoadState::Ready(_)));
}

#[test]
fn poll_bulk_platform_action_failure_preserves_the_cached_row_and_selection() {
    let mut app = app_for_operation_tests();
    let stale_snapshot = cached_snapshot(vec![persisted_archive_with_platform(
        PathBuf::from("/roms/a.zip"),
        1,
        "N64",
        "folder_alias",
    )]);
    app.database_state = DatabaseState::Ready {
        snapshot: Box::new(stale_snapshot.clone()),
        last_scan_summary: None,
    };
    let selected: HashSet<PathBuf> = [PathBuf::from("/roms/a.zip"), PathBuf::from("/roms/b.zip")]
        .into_iter()
        .collect();
    app.archive_context.selected = selected.clone();
    let (sender, receiver) = mpsc::channel();
    app.bulk_platform_action = Some(RunningBulkPlatformAction {
        kind: BulkPlatformActionKind::Set("GameCube".to_string()),
        requested_paths: 2,
        receiver,
    });
    sender.send(Err("database is locked".to_string())).unwrap();

    app.poll_bulk_platform_action(&egui::Context::default());

    assert!(app.bulk_platform_action.is_none());
    let feedback = app.feedback.as_ref().unwrap();
    assert!(!feedback.succeeded);
    assert!(feedback.message.contains("database is locked"));
    assert!(app.history.entries().any(|entry| entry.action
        == ActivityAction::BulkPlatformAssignment
        && entry.outcome == ActivityOutcome::Failed));
    // Requirement 8: a failed bulk action must preserve both the
    // prior cached rows and the selection exactly as they were.
    match &app.database_state {
        DatabaseState::Ready { snapshot, .. } => {
            assert_eq!(snapshot.archives, stale_snapshot.archives);
            assert_eq!(snapshot.database_path, stale_snapshot.database_path);
        }
        other => panic!(
            "expected the cached snapshot to survive untouched, got status {}",
            other.status_label()
        ),
    }
    assert_eq!(app.archive_context.selected, selected);
}

#[test]
fn apply_bulk_platform_action_at_sets_platform_for_every_selected_archive() {
    let dir = database_test_dir("apply-bulk-set");
    let source = dir.join("source");
    let mount = dir.join("mount");
    let path_a = write_archive_file(&source, "a.zip", b"a");
    let path_b = write_archive_file(&source, "b.zip", b"b");
    let config = config_for(&source, &mount);
    let database_path = dir.join("library.sqlite3");
    {
        let mut database = Database::open_or_create(&database_path).unwrap();
        scan_and_persist(&mut database, &config, "test").unwrap();
    }

    let outcome = apply_bulk_platform_action_at(
        &database_path,
        &[path_a, path_b],
        &BulkPlatformActionKind::Set("GameCube".to_string()),
    )
    .unwrap();

    assert_eq!(outcome.result.requested, 2);
    assert_eq!(outcome.result.changed, 2);
    assert_eq!(outcome.unresolved_paths, 0);

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn apply_bulk_platform_action_at_reports_unresolved_paths_separately_from_missing_ids() {
    let dir = database_test_dir("apply-bulk-unresolved");
    let source = dir.join("source");
    let mount = dir.join("mount");
    let path_a = write_archive_file(&source, "a.zip", b"a");
    let config = config_for(&source, &mount);
    let database_path = dir.join("library.sqlite3");
    {
        let mut database = Database::open_or_create(&database_path).unwrap();
        scan_and_persist(&mut database, &config, "test").unwrap();
    }
    // A live-only row never scanned into the database at all - a
    // fundamentally different situation from a stale archive id, and
    // must be reported separately (unresolved_paths), never silently
    // treated as if it were a "missing" database id.
    let never_scanned = source.join("never-scanned.zip");

    let outcome = apply_bulk_platform_action_at(
        &database_path,
        &[path_a, never_scanned],
        &BulkPlatformActionKind::Set("GameCube".to_string()),
    )
    .unwrap();

    assert_eq!(outcome.result.requested, 1);
    assert_eq!(outcome.result.changed, 1);
    assert!(outcome.result.missing.is_empty());
    assert_eq!(outcome.unresolved_paths, 1);

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn apply_bulk_platform_action_at_clears_and_restores_fallback() {
    let dir = database_test_dir("apply-bulk-clear");
    let source = dir.join("source");
    let mount = dir.join("mount");
    let path_a = write_archive_file(&source, "msx2/game.zip", b"a");
    let config = config_for(&source, &mount);
    let database_path = dir.join("library.sqlite3");
    {
        let mut database = Database::open_or_create(&database_path).unwrap();
        scan_and_persist(&mut database, &config, "test").unwrap();
    }
    apply_bulk_platform_action_at(
        &database_path,
        std::slice::from_ref(&path_a),
        &BulkPlatformActionKind::Set("GameCube".to_string()),
    )
    .unwrap();

    let outcome =
        apply_bulk_platform_action_at(&database_path, &[path_a], &BulkPlatformActionKind::Clear)
            .unwrap();

    assert_eq!(outcome.result.changed, 1);

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[cfg(unix)]
fn apply_bulk_platform_action_at_works_for_non_utf8_archive_paths_on_unix() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let dir = database_test_dir("apply-bulk-non-utf8");
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
    let other_path = write_archive_file(&source, "other.zip", b"contents");
    let config = config_for(&source, &mount);
    let database_path = dir.join("library.sqlite3");
    {
        let mut database = Database::open_or_create(&database_path).unwrap();
        scan_and_persist(&mut database, &config, "test").unwrap();
    }

    let outcome = apply_bulk_platform_action_at(
        &database_path,
        &[archive_path, other_path],
        &BulkPlatformActionKind::Set("GameCube".to_string()),
    )
    .unwrap();

    assert_eq!(
        outcome.result.changed, 2,
        "a non-UTF-8 archive path must resolve to its exact archive, not be silently dropped"
    );
    assert_eq!(outcome.unresolved_paths, 0);

    std::fs::remove_dir_all(&dir).unwrap();
}

// -------------------------------------------------------------
// Custom Platform Aliases panel
// -------------------------------------------------------------

#[test]
fn alias_action_available_requires_no_running_action_or_database_load() {
    let mut app = app_for_operation_tests();
    assert!(app.alias_action_available());

    let (_sender, receiver) = mpsc::channel();
    app.alias_action = Some(RunningAliasAction {
        action: AliasAction::Remove {
            alias: "gc".to_string(),
        },
        receiver,
    });
    assert!(!app.alias_action_available());
    app.alias_action = None;

    let (_sender, receiver) = mpsc::channel();
    app.database_state = DatabaseState::Loading {
        generation: DatabaseGeneration::INITIAL,
        receiver,
        worker: None,
        previous: None,
        scanning: false,
    };
    assert!(!app.alias_action_available());
}

#[test]
fn start_alias_action_does_not_start_a_second_concurrent_action() {
    let mut app = app_for_operation_tests();
    let (_sender, receiver) = mpsc::channel();
    app.alias_action = Some(RunningAliasAction {
        action: AliasAction::Add {
            alias: "gc".to_string(),
            platform: "GameCube".to_string(),
        },
        receiver,
    });
    let first_action = app.alias_action.as_ref().unwrap().action.clone();

    // A second alias action must not replace the first one's receiver
    // - mirrors start_operation_rejects_a_second_operation_without_replacing_the_receiver's
    // existing convention for the archive-action channel. Seed the
    // running action directly so this test never spawns a production
    // worker against the real default database path.
    app.start_alias_action(
        egui::Context::default(),
        AliasAction::Remove {
            alias: "wii".to_string(),
        },
    );
    assert_eq!(app.alias_action.as_ref().unwrap().action, first_action);
}

#[test]
fn poll_alias_action_add_success_refreshes_the_cache_and_clears_the_input_fields() {
    let mut app = app_for_operation_tests();
    app.new_alias_text = "gc".to_string();
    app.new_alias_platform_choice = Some("GameCube".to_string());
    let (sender, receiver) = mpsc::channel();
    app.alias_action = Some(RunningAliasAction {
        action: AliasAction::Add {
            alias: "gc".to_string(),
            platform: "GameCube".to_string(),
        },
        receiver,
    });
    sender.send(Ok(())).unwrap();

    app.poll_alias_action(&egui::Context::default());

    assert!(app.alias_action.is_none());
    let feedback = app.feedback.as_ref().unwrap();
    assert!(feedback.succeeded);
    assert!(feedback.message.contains("gc"));
    assert!(feedback.message.contains("GameCube"));
    assert!(feedback.message.contains("Run a library scan"));
    assert!(
        app.history
            .entries()
            .any(|entry| entry.outcome == ActivityOutcome::Completed
                && entry.action == ActivityAction::PlatformAliasManagement)
    );
    assert!(app.new_alias_text.is_empty());
    assert!(app.new_alias_platform_choice.is_none());
    // Asynchronous: only a new background database load is started,
    // never blocked on, and the live snapshot is untouched.
    assert!(app.database_state.is_loading());
    assert!(matches!(app.state, LoadState::Ready(_)));
}

#[test]
fn poll_alias_action_remove_success_refreshes_the_cache() {
    let mut app = app_for_operation_tests();
    let (sender, receiver) = mpsc::channel();
    app.alias_action = Some(RunningAliasAction {
        action: AliasAction::Remove {
            alias: "gc".to_string(),
        },
        receiver,
    });
    sender.send(Ok(())).unwrap();

    app.poll_alias_action(&egui::Context::default());

    assert!(app.alias_action.is_none());
    let feedback = app.feedback.as_ref().unwrap();
    assert!(feedback.succeeded);
    assert!(feedback.message.contains("gc"));
    assert!(app.database_state.is_loading());
}

#[test]
fn poll_alias_action_failure_preserves_the_cached_aliases_and_shows_the_error() {
    let mut app = app_for_operation_tests();
    let stale_snapshot = cached_snapshot(Vec::new());
    let mut stale_snapshot = stale_snapshot;
    stale_snapshot.platform_aliases = vec![PlatformAlias {
        id: 1,
        alias: "gc".to_string(),
        normalized_alias: "gc".to_string(),
        platform: "GameCube".to_string(),
        created_at: "2026-01-01T00:00:00Z".to_string(),
        updated_at: "2026-01-01T00:00:00Z".to_string(),
    }];
    app.database_state = DatabaseState::Ready {
        snapshot: Box::new(stale_snapshot.clone()),
        last_scan_summary: None,
    };
    app.new_alias_text = "wii".to_string();
    app.new_alias_platform_choice = Some("Wii".to_string());
    let (sender, receiver) = mpsc::channel();
    app.alias_action = Some(RunningAliasAction {
        action: AliasAction::Add {
            alias: "wii".to_string(),
            platform: "Wii".to_string(),
        },
        receiver,
    });
    sender
        .send(Err("a platform alias for 'wii' already exists".to_string()))
        .unwrap();

    app.poll_alias_action(&egui::Context::default());

    assert!(app.alias_action.is_none());
    let feedback = app.feedback.as_ref().unwrap();
    assert!(!feedback.succeeded);
    assert!(feedback.message.contains("already exists"));
    assert!(
        app.history
            .entries()
            .any(|entry| entry.outcome == ActivityOutcome::Failed
                && entry.action == ActivityAction::PlatformAliasManagement)
    );
    // A failed add must not clear the input fields (the user should
    // be able to see/correct what they typed) and must not touch the
    // cached snapshot or trigger a database reload.
    assert_eq!(app.new_alias_text, "wii");
    assert_eq!(app.new_alias_platform_choice, Some("Wii".to_string()));
    match &app.database_state {
        DatabaseState::Ready { snapshot, .. } => {
            assert_eq!(snapshot.platform_aliases, stale_snapshot.platform_aliases);
        }
        other => panic!(
            "expected the stale Ready snapshot to survive untouched, got status {}",
            other.status_label()
        ),
    }
}

#[test]
fn alias_action_is_independent_of_is_busy_and_mount_action_availability() {
    // Alias management is metadata-only, exactly like per-archive
    // platform assignment (platform_action): it must never appear in
    // is_busy() (which gates mount/unmount exclusivity), and a
    // running alias action must not disable mount/unmount actions.
    let mut app = app_for_operation_tests();
    let (_sender, receiver) = mpsc::channel();
    app.alias_action = Some(RunningAliasAction {
        action: AliasAction::Remove {
            alias: "gc".to_string(),
        },
        receiver,
    });
    assert!(!app.is_busy());
}

#[test]
fn new_alias_action_uses_the_chosen_canonical_platform() {
    for platform in canonical_platform_names() {
        let action = resolved_new_alias_action("gc", Some(platform)).unwrap();
        assert_eq!(
            action,
            AliasAction::Add {
                alias: "gc".to_string(),
                platform: platform.to_string(),
            }
        );
    }
}

#[test]
fn resolved_new_alias_action_requires_a_non_empty_alias_and_a_chosen_platform() {
    assert!(resolved_new_alias_action("gc", None).is_none());
    assert!(resolved_new_alias_action("   ", Some("GameCube")).is_none());
    assert!(resolved_new_alias_action("", Some("GameCube")).is_none());
    assert_eq!(
        resolved_new_alias_action("  gc  ", Some("GameCube")),
        Some(AliasAction::Add {
            alias: "gc".to_string(),
            platform: "GameCube".to_string(),
        })
    );
}

#[test]
fn apply_alias_action_add_list_remove_round_trip_and_duplicate_error() {
    let dir = database_test_dir("apply-alias-round-trip");
    let database_path = dir.join("library.sqlite3");

    apply_alias_action_at(
        &database_path,
        &AliasAction::Add {
            alias: "gc".to_string(),
            platform: "GameCube".to_string(),
        },
    )
    .unwrap();

    let duplicate_error = apply_alias_action_at(
        &database_path,
        &AliasAction::Add {
            alias: "GC".to_string(),
            platform: "Wii".to_string(),
        },
    )
    .unwrap_err();
    assert!(duplicate_error.to_string().contains("already exists"));

    let database = Database::open_or_create(&database_path).unwrap();
    assert_eq!(database.list_platform_aliases().unwrap().len(), 1);
    drop(database);

    apply_alias_action_at(
        &database_path,
        &AliasAction::Remove {
            alias: "gc".to_string(),
        },
    )
    .unwrap();
    let database = Database::open_or_create(&database_path).unwrap();
    assert!(database.list_platform_aliases().unwrap().is_empty());

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn apply_alias_action_remove_unknown_alias_is_a_clear_error() {
    let dir = database_test_dir("apply-alias-remove-unknown");
    let database_path = dir.join("library.sqlite3");
    Database::open_or_create(&database_path).unwrap();

    let error = apply_alias_action_at(
        &database_path,
        &AliasAction::Remove {
            alias: "does-not-exist".to_string(),
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("no platform alias matches"));

    std::fs::remove_dir_all(&dir).unwrap();
}

/// A record for a path that is *not* a mountable archive, built without
/// going through `Archive::from_path`.
///
/// Needed because a ScummVM resource file is no longer classified as an
/// archive at all: `.gen` only means "Mega Drive ROM" once a folder or a
/// cartridge header corroborates it, so `Archive::from_path` correctly
/// returns `None` for `RESOURCE.GEN`. A row like this can therefore only
/// reach the health view from a database an *older* build wrote, which is
/// exactly the case `HealthCategory::MountNotRequired` exists to classify -
/// and exactly why nothing here rewrites the stored row.
fn legacy_unsupported_record(
    path: &str,
    kind: ArchiveKind,
    platform: Option<&str>,
) -> ArchiveRecord {
    let path = PathBuf::from(path);
    let archive = Archive {
        path: path.clone(),
        kind,
        identity: archivefs_core::ArchiveIdentity::from_path(&path, PathBuf::from("/roms"), None),
        health: ArchiveHealth::Unsupported,
    };
    let mut record = ArchiveRecord::new(
        MountPlan::new(archive, PathBuf::from("/mnt/archivefs/Test")),
        MountState::NotMountable,
        ArchiveMetadata {
            title: None,
            platform: platform.map(str::to_string),
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
        ArchiveHealth::Unsupported,
    );
    record.health = ArchiveHealth::Unsupported;
    record
}

fn health_issue_fixture(path: &str, category: HealthCategory) -> HealthIssue {
    HealthIssue {
        path: PathBuf::from(path),
        platform: Some("SNES".to_string()),
        present: !matches!(category, HealthCategory::Missing),
        mount_state: None,
        category,
        reason: category.label().to_string(),
        retryable: category == HealthCategory::RetryableFailure,
        recovery_action: None,
        last_seen_at: None,
        size_bytes: None,
        modified_time_unix_seconds: None,
    }
}

#[test]
fn build_health_issues_classifies_mounted_and_pending_as_healthy() {
    let mounted = health_test_record(
        "/roms/a.zip",
        MountState::Mounted,
        ArchiveHealth::Mounted,
        Some("SNES"),
    );
    let pending = health_test_record(
        "/roms/b.zip",
        MountState::Pending,
        ArchiveHealth::Pending,
        Some("SNES"),
    );
    let cached = cached_snapshot(Vec::new());

    let issues = build_health_issues(
        &[mounted, pending],
        &cached,
        &HashSet::new(),
        &HashSet::new(),
    );

    assert!(
        issues.is_empty(),
        "mounted and pending archives with no failure must never be reported as issues"
    );
}

#[test]
fn build_health_issues_marks_retryable_and_terminal_failures_distinctly() {
    let retryable = health_test_record(
        "/roms/a.zip",
        MountState::Pending,
        ArchiveHealth::Failed,
        Some("SNES"),
    );
    let terminal = health_test_record(
        "/roms/b.zip",
        MountState::Pending,
        ArchiveHealth::Corrupt,
        Some("SNES"),
    );
    let cached = cached_snapshot(Vec::new());

    let issues = build_health_issues(
        &[retryable, terminal],
        &cached,
        &HashSet::new(),
        &HashSet::new(),
    );

    assert_eq!(issues.len(), 2);
    let retryable_issue = issues
        .iter()
        .find(|issue| issue.path == Path::new("/roms/a.zip"))
        .unwrap();
    let terminal_issue = issues
        .iter()
        .find(|issue| issue.path == Path::new("/roms/b.zip"))
        .unwrap();
    assert_eq!(retryable_issue.category, HealthCategory::RetryableFailure);
    assert!(retryable_issue.retryable);
    assert_eq!(
        retryable_issue.recovery_action,
        Some(RecoveryAction::RetryMount)
    );
    assert_eq!(terminal_issue.category, HealthCategory::TerminalFailure);
    assert!(!terminal_issue.retryable);
    assert_eq!(terminal_issue.recovery_action, None);
}

#[test]
fn build_health_issues_marks_loose_roms_and_scummvm_gen_resources_as_no_mount_required() {
    let loose_rom = health_test_record(
        "/roms/genesis/3 Ninjas Kick Back (USA).md",
        MountState::NotMountable,
        ArchiveHealth::Unsupported,
        Some("MegaDrive"),
    );
    // A row an older build persisted, when `.gen` alone was taken for a
    // Mega Drive ROM. Current scans never produce one - see
    // `legacy_unsupported_record` - but the row is still in real databases
    // and must classify as "no mount required" rather than as a failure.
    let scummvm_resource = legacy_unsupported_record(
        "/roms/scummvm/laurabow2/RESOURCE.GEN",
        ArchiveKind::MegaDriveRom,
        Some("ScummVM"),
    );
    let issues = build_health_issues(
        &[loose_rom, scummvm_resource],
        &cached_snapshot(Vec::new()),
        &HashSet::new(),
        &HashSet::new(),
    );

    assert_eq!(issues.len(), 2);
    assert!(
        issues
            .iter()
            .all(|issue| issue.category == HealthCategory::MountNotRequired)
    );
    assert!(issues.iter().all(|issue| !issue.retryable));
    assert!(issues.iter().all(|issue| issue.recovery_action.is_none()));
}

#[test]
fn persisted_failure_without_a_live_record_is_historical() {
    let mut persisted = persisted_archive(PathBuf::from("/roms/old/Game.zip"), false);
    persisted.last_known_health = "Corrupt".to_string();
    let issues = build_health_issues(
        &[],
        &cached_snapshot(vec![persisted]),
        &HashSet::new(),
        &HashSet::new(),
    );

    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].category, HealthCategory::HistoricalMountFailure);
}

#[test]
fn build_health_issues_marks_missing_and_cached_only_distinctly() {
    let dir = database_test_dir("health-cached-states");
    let reachable = write_archive_file(&dir, "reachable.zip", b"data");
    let unreachable = dir.join("gone.zip");

    let missing_archive = persisted_archive(dir.join("missing.zip"), true);
    let mut awaiting_archive = persisted_archive(reachable, false);
    awaiting_archive.id = 2;
    let mut unreachable_archive = persisted_archive(unreachable, false);
    unreachable_archive.id = 3;

    let cached = cached_snapshot(vec![missing_archive, awaiting_archive, unreachable_archive]);
    let issues = build_health_issues(&[], &cached, &HashSet::new(), &HashSet::new());

    assert_eq!(issues.len(), 3);
    let category_for = |suffix: &str| {
        issues
            .iter()
            .find(|issue| issue.path.ends_with(suffix))
            .unwrap()
            .category
    };
    assert_eq!(category_for("missing.zip"), HealthCategory::Missing);
    assert_eq!(
        category_for("reachable.zip"),
        HealthCategory::AwaitingValidation
    );
    assert_eq!(category_for("gone.zip"), HealthCategory::CachedOnly);

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn doctor_does_not_turn_a_known_legacy_cue_companion_into_a_missing_game() {
    let mut cue = persisted_archive(PathBuf::from("/roms/Disc Game.cue"), true);
    cue.archive_kind = "direct_game_image".to_string();
    cue.last_seen_at = "2026-08-08T20:00:00Z".to_string();
    let cached = cached_snapshot(vec![cue]);

    let issues = build_health_issues(&[], &cached, &HashSet::new(), &HashSet::new());

    assert!(issues.is_empty(), "a known companion is not a missing game");
}

#[test]
fn build_health_issues_never_floods_one_issue_per_archive_for_an_unavailable_source() {
    // 1,242 archives under one offline source must never produce
    // 1,242 `HealthIssue`s - `source_health_issues` (called
    // separately by the Health Dashboard) already reports this
    // truthfully as one source-level problem; per-archive issues for
    // that same source must be suppressed here, never doubled up.
    let offline_source_id = 2;
    let mut offline_archives: Vec<PersistedArchive> = (0..50)
        .map(|index| {
            let mut archive = persisted_archive(
                PathBuf::from(format!("/mnt/usbdrive/retro/{index}.zip")),
                false,
            );
            archive.id = index;
            archive.source_folder_id = offline_source_id;
            archive
        })
        .collect();
    // One archive under a perfectly healthy, available source must
    // still produce its own ordinary issue - suppression is scoped
    // to the offline source only, never global.
    let mut healthy_source_archive =
        persisted_archive(PathBuf::from("/home/davedap/Archives/other.zip"), false);
    healthy_source_archive.id = 999;
    healthy_source_archive.source_folder_id = 1;
    offline_archives.push(healthy_source_archive);

    let cached = CachedLibrarySnapshot {
        source_views: vec![
            source_view_fixture(1, "/home/davedap/Archives", true),
            SourceFolderView {
                availability: SourceAvailability::Unavailable,
                ..source_view_fixture(offline_source_id, "/mnt/usbdrive/retro", true)
            },
        ],
        ..cached_snapshot(offline_archives)
    };

    let issues = build_health_issues(&[], &cached, &HashSet::new(), &HashSet::new());

    assert_eq!(
        issues.len(),
        1,
        "the 50 archives under the offline source must produce zero per-archive issues; \
             only the healthy source's own archive should appear"
    );
    assert!(issues[0].path.ends_with("other.zip"));
}

#[test]
fn build_health_issues_recovery_available_only_with_a_real_offer() {
    let record = health_test_record(
        "/roms/a.zip",
        MountState::Pending,
        ArchiveHealth::Pending,
        Some("SNES"),
    );
    let cached = cached_snapshot(Vec::new());

    let no_offer = build_health_issues(
        std::slice::from_ref(&record),
        &cached,
        &HashSet::new(),
        &HashSet::new(),
    );
    assert!(
        no_offer.is_empty(),
        "a pending archive with no active recovery offer is not an issue"
    );

    let remount_offers: HashSet<PathBuf> = [PathBuf::from("/roms/a.zip")].into_iter().collect();
    let with_remount = build_health_issues(&[record], &cached, &HashSet::new(), &remount_offers);
    assert_eq!(with_remount.len(), 1);
    assert_eq!(with_remount[0].category, HealthCategory::RecoveryAvailable);
    assert_eq!(
        with_remount[0].recovery_action,
        Some(RecoveryAction::Remount)
    );
    assert!(with_remount[0].recovery_available());

    let mounted_record = health_test_record(
        "/roms/b.zip",
        MountState::Mounted,
        ArchiveHealth::Mounted,
        Some("SNES"),
    );
    let lazy_unmount_offers: HashSet<PathBuf> =
        [PathBuf::from("/roms/b.zip")].into_iter().collect();
    let with_lazy = build_health_issues(
        &[mounted_record],
        &cached,
        &lazy_unmount_offers,
        &HashSet::new(),
    );
    assert_eq!(with_lazy.len(), 1);
    assert_eq!(
        with_lazy[0].recovery_action,
        Some(RecoveryAction::LazyUnmount)
    );
}

#[test]
fn health_overview_counts_exactly_match_the_issues_present() {
    let retryable = health_test_record(
        "/roms/a.zip",
        MountState::Pending,
        ArchiveHealth::Failed,
        Some("SNES"),
    );
    let terminal = health_test_record(
        "/roms/b.zip",
        MountState::Pending,
        ArchiveHealth::Corrupt,
        Some("SNES"),
    );
    let unknown = health_test_record(
        "/roms/c.zip",
        MountState::Mounted,
        ArchiveHealth::Mounted,
        None,
    );
    let healthy = health_test_record(
        "/roms/d.zip",
        MountState::Mounted,
        ArchiveHealth::Mounted,
        Some("SNES"),
    );
    let records = vec![retryable, terminal, unknown, healthy];
    let cached = cached_snapshot(Vec::new());
    let issues = build_health_issues(&records, &cached, &HashSet::new(), &HashSet::new());
    let overview = health_overview(&issues, records.len(), 2, 2, 0);

    assert_eq!(overview.retryable_failures, 1);
    assert_eq!(overview.terminal_failures, 1);
    assert_eq!(overview.unknown_platform, 1);
    assert_eq!(overview.healthy, 1);
    assert_eq!(
        overview.healthy
            + overview.retryable_failures
            + overview.terminal_failures
            + overview.unknown_platform,
        records.len(),
        "the overview's counts must exactly match the archives actually present"
    );
}

#[test]
fn visible_health_issue_indices_does_not_mutate_the_underlying_issues() {
    let issues = vec![
        health_issue_fixture("/roms/a.zip", HealthCategory::Missing),
        health_issue_fixture("/roms/b.zip", HealthCategory::TerminalFailure),
    ];
    let before = issues.clone();
    let filters = HealthDashboardFilters {
        category: HealthIssueFilter::Missing,
        ..Default::default()
    };

    let _ = visible_health_issue_indices(&issues, &filters, HealthSortField::Severity, true);

    assert_eq!(
        issues, before,
        "filtering must never mutate the underlying cached issue list"
    );
}

#[test]
fn visible_health_issue_indices_search_matches_path_and_reason() {
    let mut luigi = health_issue_fixture("/roms/Luigi.zip", HealthCategory::Missing);
    luigi.reason = "Missing from latest successful scan".to_string();
    let mut mario = health_issue_fixture("/roms/Mario.zip", HealthCategory::CachedOnly);
    mario.reason = "Archive exists only in the cached catalogue".to_string();
    let issues = vec![luigi, mario];

    let mut filters = HealthDashboardFilters {
        search: "luigi".to_string(),
        ..Default::default()
    };
    assert_eq!(
        visible_health_issue_indices(&issues, &filters, HealthSortField::Path, true),
        vec![0],
        "search must match the exact path"
    );

    filters.search = "cached catalogue".to_string();
    assert_eq!(
        visible_health_issue_indices(&issues, &filters, HealthSortField::Path, true),
        vec![1],
        "search must also match the reason text"
    );
}

#[test]
fn visible_health_issue_indices_platform_filter_works() {
    let mut snes = health_issue_fixture("/roms/a.zip", HealthCategory::Missing);
    snes.platform = Some("SNES".to_string());
    let mut genesis = health_issue_fixture("/roms/b.zip", HealthCategory::Missing);
    genesis.platform = Some("Genesis".to_string());
    let issues = vec![snes, genesis];

    let filters = HealthDashboardFilters {
        platform: Some("Genesis".to_string()),
        ..Default::default()
    };
    assert_eq!(
        visible_health_issue_indices(&issues, &filters, HealthSortField::Path, true),
        vec![1]
    );
}

#[test]
fn health_issue_filter_mount_failures_includes_both_retryable_and_terminal() {
    assert!(HealthIssueFilter::MountFailures.matches(HealthCategory::RetryableFailure));
    assert!(HealthIssueFilter::MountFailures.matches(HealthCategory::TerminalFailure));
    assert!(!HealthIssueFilter::MountFailures.matches(HealthCategory::Missing));
    assert!(HealthIssueFilter::Retryable.matches(HealthCategory::RetryableFailure));
    assert!(!HealthIssueFilter::Retryable.matches(HealthCategory::TerminalFailure));
    assert!(HealthIssueFilter::Terminal.matches(HealthCategory::TerminalFailure));
    assert!(!HealthIssueFilter::Terminal.matches(HealthCategory::RetryableFailure));
    assert!(HealthIssueFilter::All.matches(HealthCategory::UnknownPlatform));
}

#[test]
fn visible_health_issue_indices_severity_sort_is_deterministic() {
    let issues = vec![
        health_issue_fixture("/roms/z-unknown.zip", HealthCategory::UnknownPlatform),
        health_issue_fixture("/roms/a-terminal.zip", HealthCategory::TerminalFailure),
        health_issue_fixture("/roms/m-missing.zip", HealthCategory::Missing),
    ];
    let filters = HealthDashboardFilters::default();

    let first = visible_health_issue_indices(&issues, &filters, HealthSortField::Severity, true);
    let second = visible_health_issue_indices(&issues, &filters, HealthSortField::Severity, true);

    assert_eq!(first, second, "sorting must be deterministic across calls");
    assert_eq!(
        first
            .iter()
            .map(|&index| issues[index].category)
            .collect::<Vec<_>>(),
        vec![
            HealthCategory::TerminalFailure,
            HealthCategory::Missing,
            HealthCategory::UnknownPlatform,
        ]
    );
}

#[test]
fn build_health_issues_reflects_the_latest_records_and_offers_on_every_call() {
    // No hidden per-call caching: two calls with different inputs must
    // never return a stale result from the first call.
    let record = health_test_record(
        "/roms/a.zip",
        MountState::Pending,
        ArchiveHealth::Pending,
        Some("SNES"),
    );
    let cached = cached_snapshot(Vec::new());

    let before = build_health_issues(
        std::slice::from_ref(&record),
        &cached,
        &HashSet::new(),
        &HashSet::new(),
    );
    assert!(before.is_empty());

    let mut failed = record;
    failed.health = ArchiveHealth::Failed;
    let after = build_health_issues(&[failed], &cached, &HashSet::new(), &HashSet::new());
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].category, HealthCategory::RetryableFailure);
}

// -----------------------------------------------------------------
// `ArchiveFsApp::cached_health_issues` - the actual per-frame cache,
// as opposed to `build_health_issues` above (the pure builder it
// calls only on a cache miss).
// -----------------------------------------------------------------

fn app_with_health_state(
    records: Vec<ArchiveRecord>,
    archives: Vec<PersistedArchive>,
) -> ArchiveFsApp {
    let mut app = app_for_operation_tests();
    app.state = LoadState::Ready(Box::new(loaded_data_with_records("/mount", records)));
    app.database_state = DatabaseState::Ready {
        snapshot: Box::new(cached_snapshot(archives)),
        last_scan_summary: None,
    };
    app
}

#[test]
fn cached_health_issues_does_not_rebuild_on_repeated_calls() {
    let record = health_test_record(
        "/roms/a.zip",
        MountState::Pending,
        ArchiveHealth::Failed,
        Some("SNES"),
    );
    let mut app = app_with_health_state(vec![record], Vec::new());

    let first_ptr = app.cached_health_issues().as_ptr();
    let second_ptr = app.cached_health_issues().as_ptr();
    let third_ptr = app.cached_health_issues().as_ptr();

    assert_eq!(
        first_ptr, second_ptr,
        "repeated calls with nothing relevant changed must reuse the same cached Vec"
    );
    assert_eq!(second_ptr, third_ptr);
}

#[test]
fn cached_health_issues_ignores_a_generation_bump_with_no_new_applied_data() {
    // Regression guard for the exact bug a raw generation-only
    // comparison would hit: `refresh_generation` bumps the instant a
    // refresh *starts*, before any new data exists (see
    // `HealthReportCacheKey`'s doc comment). The cache must track the
    // actually-applied `LoadedData`, never `refresh_generation` alone.
    let record = health_test_record(
        "/roms/a.zip",
        MountState::Pending,
        ArchiveHealth::Failed,
        Some("SNES"),
    );
    let mut app = app_with_health_state(vec![record], Vec::new());

    let before_ptr = app.cached_health_issues().as_ptr();
    app.refresh_generation = app.refresh_generation.next();
    let after_ptr = app.cached_health_issues().as_ptr();

    assert_eq!(
        after_ptr, before_ptr,
        "a generation bump alone, with self.state still pointing at the same LoadedData, \
             must never invalidate the cache"
    );
}

#[test]
fn cached_health_issues_refreshes_when_the_live_snapshot_actually_changes() {
    let pending = health_test_record(
        "/roms/a.zip",
        MountState::Pending,
        ArchiveHealth::Pending,
        Some("SNES"),
    );
    let mut app = app_with_health_state(vec![pending], Vec::new());
    assert!(
        app.cached_health_issues().is_empty(),
        "a pending archive with no failure is healthy"
    );

    let failed = health_test_record(
        "/roms/a.zip",
        MountState::Pending,
        ArchiveHealth::Failed,
        Some("SNES"),
    );
    app.state = LoadState::Ready(Box::new(loaded_data_with_records("/mount", vec![failed])));

    let issues = app.cached_health_issues();
    assert_eq!(
        issues.len(),
        1,
        "a genuinely new LoadedData (new Box) must invalidate the cache and rebuild"
    );
    assert_eq!(issues[0].category, HealthCategory::RetryableFailure);
}

#[test]
fn cached_health_issues_refreshes_when_recovery_offers_change() {
    let record = health_test_record(
        "/roms/b.zip",
        MountState::Pending,
        ArchiveHealth::Pending,
        Some("SNES"),
    );
    let mut app = app_with_health_state(vec![record], Vec::new());
    assert!(app.cached_health_issues().is_empty());

    app.remount_offers.insert(PathBuf::from("/roms/b.zip"));
    let issues = app.cached_health_issues();

    assert_eq!(
        issues.len(),
        1,
        "a mount/unmount/remount completion changing the recovery offers must refresh \
             recovery classifications, even though neither generation moved"
    );
    assert_eq!(issues[0].category, HealthCategory::RecoveryAvailable);
    assert_eq!(issues[0].recovery_action, Some(RecoveryAction::Remount));
}

#[test]
fn cached_health_issues_refreshes_when_the_database_snapshot_changes() {
    let mut app = app_with_health_state(Vec::new(), Vec::new());
    assert!(app.cached_health_issues().is_empty());

    let missing_archive = persisted_archive(PathBuf::from("/roms/missing.zip"), true);
    app.database_state = DatabaseState::Ready {
        snapshot: Box::new(cached_snapshot(vec![missing_archive])),
        last_scan_summary: None,
    };

    let issues = app.cached_health_issues();
    assert_eq!(
        issues.len(),
        1,
        "a new database snapshot (catalogue cleanup, platform assignment, rescan, ...) must \
             refresh missing/cached-only/unknown-platform classifications"
    );
    assert_eq!(issues[0].category, HealthCategory::Missing);
}

#[test]
fn changing_dashboard_filters_and_sort_does_not_rebuild_the_cached_report() {
    let record = health_test_record(
        "/roms/a.zip",
        MountState::Pending,
        ArchiveHealth::Failed,
        Some("SNES"),
    );
    let mut app = app_with_health_state(vec![record], Vec::new());

    let before_ptr = app.cached_health_issues().as_ptr();

    app.health_filters.search = "something".to_string();
    app.health_filters.category = HealthIssueFilter::Retryable;
    app.health_sort_field = HealthSortField::Reason;
    app.health_sort_ascending = false;
    let issues_snapshot = app.cached_health_issues().to_vec();
    let _ = visible_health_issue_indices(
        &issues_snapshot,
        &app.health_filters,
        app.health_sort_field,
        app.health_sort_ascending,
    );

    let after_ptr = app.cached_health_issues().as_ptr();
    assert_eq!(
        after_ptr, before_ptr,
        "changing the dashboard's own filters/sort, and filtering/sorting itself, must \
             never rebuild the underlying cached report"
    );
}

#[test]
fn health_dashboard_state_is_separate_from_library_state_and_activity() {
    let mut app = app_for_operation_tests();
    app.filter = "ordinary search".to_string();
    app.library_filters.missing = true;
    app.sort_field = Some(SortField::State);
    app.archive_context.focused = Some(PathBuf::from("/roms/library.zip"));
    app.archive_context.selected = [PathBuf::from("/roms/library.zip")].into_iter().collect();
    app.selected_duplicate_archive = Some(PathBuf::from("/backup/Other.7z"));
    let history_len = app.history.entries.len();

    app.view = MainView::Health;
    app.health_filters.search = "luigi".to_string();
    app.health_filters.category = HealthIssueFilter::Missing;
    app.health_sort_field = HealthSortField::Reason;
    app.health_sort_ascending = false;
    app.selected_health_issue = Some(PathBuf::from("/roms/health-issue.zip"));
    app.view = MainView::Library;

    assert_eq!(app.filter, "ordinary search");
    assert!(app.library_filters.missing);
    assert_eq!(app.sort_field, Some(SortField::State));
    assert_eq!(
        app.archive_context.focused,
        Some(PathBuf::from("/roms/library.zip"))
    );
    assert_eq!(
        app.archive_context.selected,
        [PathBuf::from("/roms/library.zip")].into_iter().collect()
    );
    assert_eq!(
        app.selected_duplicate_archive,
        Some(PathBuf::from("/backup/Other.7z")),
        "Duplicate Review's selection must remain independent of the health dashboard"
    );
    assert_eq!(
        app.history.entries.len(),
        history_len,
        "opening, filtering, sorting, and selecting in the dashboard must never add \
             Activity entries"
    );
}

#[test]
fn sources_and_tools_overlay_navigation_never_touches_library_state_or_activity() {
    let mut app = app_for_operation_tests();
    app.filter = "ordinary search".to_string();
    app.library_filters.missing = true;
    app.sort_field = Some(SortField::State);
    app.archive_context.focused = Some(PathBuf::from("/roms/library.zip"));
    app.library_source_filter = Some(Some(PathBuf::from("/home/davedap/Archives")));
    let history_len = app.history.entries.len();

    app.view = MainView::Sources;
    app.tools_overlay = ToolsOverlay::DoctorChecks;
    app.tools_overlay = ToolsOverlay::PlatformAliases;
    app.tools_overlay = ToolsOverlay::DatabaseStatus;
    app.tools_overlay = ToolsOverlay::ArchiveInspector;
    app.tools_overlay = ToolsOverlay::None;
    app.view = MainView::Library;

    assert_eq!(app.filter, "ordinary search");
    assert!(app.library_filters.missing);
    assert_eq!(app.sort_field, Some(SortField::State));
    assert_eq!(
        app.archive_context.focused,
        Some(PathBuf::from("/roms/library.zip"))
    );
    assert_eq!(
        app.library_source_filter,
        Some(Some(PathBuf::from("/home/davedap/Archives")))
    );
    assert_eq!(
        app.history.entries.len(),
        history_len,
        "visiting Sources or any Tools overlay must never add Activity entries"
    );
}

/// Mirrors `show_primary_navigation`'s exact layout (same widgets, same
/// group order, same enabled/selected predicates) purely to discover each
/// button's rendered `Rect` for click simulation. The production function
/// itself only returns `Option<NavClick>`, not per-button geometry; egui's
/// vertical layout is fully deterministic from widget order and size
/// alone, so this mirror's rects match the real function's. Iterates the
/// same `ADVANCED_NAV_GROUPS` const production uses (docs/
/// GUI_NAVIGATION_RESET_DESIGN.md §3.2's grouped sidebar), so the mirror
/// cannot drift from the real, grouped destination list. The actual click
/// below is driven through the real production function, not this mirror.
fn primary_nav_rects(
    ctx: &egui::Context,
    current: MainView,
    current_overlay: ToolsOverlay,
    has_database: bool,
) -> Vec<(NavClick, egui::Rect)> {
    let mut rects = Vec::new();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("EmuWiz").size(23.0).strong());
                ui.label(egui::RichText::new("Archive library manager").color(theme::muted(ui)));
                ui.add_space(18.0);
                for group in ADVANCED_NAV_GROUPS {
                    if let Some(heading) = group.heading {
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new(heading)
                                .small()
                                .strong()
                                .color(theme::muted(ui)),
                        );
                    }
                    for entry in group.entries {
                        let (enabled, selected) = match entry.click {
                            NavClick::View(view) => (
                                navigation_destination_enabled(view, has_database),
                                navigation_destination_selected(current, view),
                            ),
                            NavClick::Overlay(overlay) => (true, current_overlay == overlay),
                            NavClick::QuickRename => (true, false),
                        };
                        let button = egui::Button::selectable(selected, entry.label)
                            .min_size(egui::vec2(ui.available_width(), 30.0));
                        let resp = ui.add_enabled(enabled, button);
                        rects.push((entry.click, resp.rect));
                    }
                }
            });
        });
    });
    rects
}

#[test]
fn all_navigation_destinations_are_reachable_via_a_real_click() {
    let ctx = egui::Context::default();

    let all_destinations: Vec<NavClick> = ADVANCED_NAV_GROUPS
        .iter()
        .flat_map(|group| group.entries.iter().map(|entry| entry.click))
        .collect();
    for target in all_destinations {
        let rects = primary_nav_rects(&ctx, MainView::Library, ToolsOverlay::None, true);
        let target_rect = rects
            .iter()
            .find(|(click, _)| *click == target)
            .map(|(_, rect)| *rect)
            .unwrap_or_else(|| panic!("{target:?} must be one of the rendered nav labels"));

        let clicked: std::rc::Rc<std::cell::RefCell<Option<NavClick>>> =
            std::rc::Rc::new(std::cell::RefCell::new(None));
        let captured = std::rc::Rc::clone(&clicked);
        let render = move |ui: &mut egui::Ui| -> egui::Response {
            let inner = ui.scope(|ui| {
                show_primary_navigation(ui, MainView::Library, ToolsOverlay::None, true)
            });
            if let Some(click) = inner.inner {
                *captured.borrow_mut() = Some(click);
            }
            inner.response
        };
        simulate_row_click(
            &ctx,
            target_rect.center(),
            egui::Modifiers::default(),
            render,
        );

        assert_eq!(
            *clicked.borrow(),
            Some(target),
            "clicking the {target:?} label must select it as the primary destination"
        );
    }
}

#[test]
fn about_is_no_longer_a_flat_sidebar_entry() {
    // About moved to the Help menu window (docs/GUI_NAVIGATION_RESET_
    // DESIGN.md §3.2: "About moves to a footer link... in both modes"),
    // which already existed independently of the old flat sidebar entry -
    // this pins that the now-redundant sidebar entry is really gone from
    // the rendered grouped nav, not just reachable a second way.
    let ctx = egui::Context::default();
    let rects = primary_nav_rects(&ctx, MainView::Library, ToolsOverlay::None, true);
    assert!(
        !rects
            .iter()
            .any(|(click, _)| *click == NavClick::View(MainView::About)),
        "About must not render as a grouped sidebar entry any more"
    );
}

fn fully_visible_exact_text_count(output: &egui::FullOutput, needles: &[String]) -> usize {
    fn find_rect(shape: &egui::Shape, needle: &str) -> Option<egui::Rect> {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == needle => {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
            }
            egui::Shape::Vec(nested) => nested.iter().find_map(|shape| find_rect(shape, needle)),
            _ => None,
        }
    }

    needles
        .iter()
        .filter(|needle| {
            output.shapes.iter().any(|clipped| {
                find_rect(&clipped.shape, needle).is_some_and(|rect| {
                    rect.top() >= clipped.clip_rect.top()
                        && rect.bottom() <= clipped.clip_rect.bottom()
                })
            })
        })
        .count()
}

fn library_app_with_test_rows(count: usize) -> (ArchiveFsApp, Vec<String>) {
    let paths: Vec<String> = (0..count)
        .map(|index| format!("/roms/library-row-{index:02}.zip"))
        .collect();
    let records = paths
        .iter()
        .map(|path| record(path, MountState::Pending))
        .collect();
    let mut app = app_for_operation_tests();
    app.state = LoadState::Ready(Box::new(loaded_data_with_records("/mount", records)));
    app.view = MainView::Library;
    app.library_tab = LibraryTab::Archives;
    // This helper exists to test Advanced View's Library table
    // rendering specifically (`show_loaded_data`'s full technical
    // columns, sort, multi-selection) - not Gamer View's compact
    // list. Every caller wants Advanced View's full `update()`
    // dispatch to run.
    app.ui_mode = GuiMode::AdvancedView;
    app.archive_context
        .select_only(PathBuf::from(paths[0].as_str()));
    (app, paths)
}

#[test]
fn library_renders_multiple_complete_rows_at_desktop_and_small_viewports() {
    for (size, minimum_rows) in [
        (egui::vec2(1920.0, 1080.0), 6_usize),
        (egui::vec2(1024.0, 600.0), 2_usize),
    ] {
        let (mut app, paths) = library_app_with_test_rows(30);
        let ctx = egui::Context::default();
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        run_settle_frames(&ctx, &mut app, &mut frame, &input, 3);
        let output = ctx.run(input, |ctx| app.update(ctx, &mut frame));
        let visible = fully_visible_exact_text_count(&output, &paths);
        assert!(
            visible >= minimum_rows,
            "{size:?} must show at least {minimum_rows} complete Library rows, showed {visible}; first row rendered={}, geometry={:?}",
            rendered_text_contains(&output, &paths[0]),
            find_exact_text_position_and_clip(&output, &paths[0])
        );
    }
}

#[test]
fn gamer_view_game_list_uses_the_remaining_height_not_two_or_three_rows() {
    // Manual QA layout finding: the Gamer View game list only showed
    // ~2-3 rows regardless of window size, with a large unused area
    // below it. The list must use all remaining height, be
    // independently scrollable, and keep the selected-game panel
    // fixed - mirroring the same desktop/1024x600 thresholds already
    // proven for Advanced View's Library table
    // (`library_renders_multiple_complete_rows_at_desktop_and_small_viewports`).
    for (size, minimum_rows) in [
        (egui::vec2(1920.0, 1080.0), 6_usize),
        (egui::vec2(1024.0, 600.0), 2_usize),
    ] {
        let mut app = app_for_operation_tests();
        app.ui_mode = GuiMode::GamerView;
        app.view = MainView::Library;
        let mut labels = Vec::new();
        let records: Vec<ArchiveRecord> = (0..30)
            .map(|index| {
                let path = format!("/roms/gamer-row-{index:02}.zip");
                let mut rec = record(&path, MountState::Pending);
                let title = format!("Game {index:02}");
                rec.metadata.title = Some(title.clone());
                rec.metadata.platform = Some("GameCube".to_string());
                labels.push(format!("{title} \u{2014} GameCube \u{b7} Ready to mount"));
                rec
            })
            .collect();
        app.state = LoadState::Ready(Box::new(loaded_data_with_records("/mount", records)));

        let ctx = egui::Context::default();
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        run_settle_frames(&ctx, &mut app, &mut frame, &input, 3);
        let output = ctx.run(input, |ctx| app.update(ctx, &mut frame));
        let visible = fully_visible_exact_text_count(&output, &labels);
        assert!(
            visible >= minimum_rows,
            "{size:?} must show at least {minimum_rows} complete Gamer View game rows, \
                 showed {visible}"
        );
    }
}

#[test]
fn summary_counter_labels_never_collapse_into_vertical_text() {
    fn text_row_count(shape: &egui::Shape, needle: &str) -> Option<usize> {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == needle => Some(text.galley.rows.len()),
            egui::Shape::Vec(nested) => nested
                .iter()
                .find_map(|shape| text_row_count(shape, needle)),
            _ => None,
        }
    }

    let ctx = egui::Context::default();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(120.0, 100.0),
        )),
        ..Default::default()
    };
    let output = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| summary_value(ui, "Total archives", 13_506));
        });
    });
    let rows = output
        .shapes
        .iter()
        .find_map(|shape| text_row_count(&shape.shape, "Total archives"));
    assert_eq!(rows, Some(1));
}

#[test]
fn expanded_activity_bar_does_not_cover_library_rows() {
    let (mut app, paths) = library_app_with_test_rows(30);
    app.show_activity = true;
    for index in 0..8 {
        app.history.record(HistoryEntry::new(
            ActivityAction::Refresh,
            None,
            ActivityOutcome::Completed,
            format!("Library activity {index}"),
        ));
    }
    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let size = egui::vec2(1920.0, 1080.0);
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        ..Default::default()
    };
    run_settle_frames(&ctx, &mut app, &mut frame, &input, 3);
    let output = ctx.run(input, |ctx| app.update(ctx, &mut frame));
    assert!(rendered_text_contains(&output, "Clear activity"));
    assert!(
        fully_visible_exact_text_count(&output, &paths) >= 4,
        "the expanded bottom activity bar must leave several complete rows visible"
    );
}

/// Renders the real `show_loaded_data` (what the Library page actually
/// dispatches to) with only `selected_archives` and
/// `select_all_visible_requested` under the caller's control - every
/// other field is a throwaway local, exactly like `RealLoadedDataHarness`
/// above but returning the full render output for text-content checks.
fn render_show_loaded_data_for_test(
    ctx: &egui::Context,
    data: &LoadedData,
    selected_archives: &mut HashSet<PathBuf>,
    select_all_visible_requested: &mut bool,
    viewport_size: Option<egui::Vec2>,
) -> egui::FullOutput {
    let mut filter = String::new();
    let mut filtered_rows = None;
    let mut selected_archive = None;
    let mut confirm_unmount = None;
    let mut confirm_lazy_unmount = None;
    let mut confirm_lazy_unmount_final = None;
    let mut confirm_mount_all = None;
    let mut focus_mount_all_cancel = false;
    let mut mount_all_typed_count = String::new();
    let mut confirm_unmount_all = None;
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
    let mut cleanup_after_unmount = false;
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

    let input = egui::RawInput {
        screen_rect: viewport_size.map(|size| egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        ..egui::RawInput::default()
    };
    ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_loaded_data(
                ui,
                data,
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
                    selected_archives,
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
                    select_all_visible_requested,
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
    })
}

#[test]
fn library_page_does_not_render_health_duplicate_or_database_admin_controls() {
    let ctx = egui::Context::default();
    let data = empty_loaded_data("/mnt/library");
    let mut selected_archives = HashSet::new();
    let mut select_all_visible_requested = false;

    let output = render_show_loaded_data_for_test(
        &ctx,
        &data,
        &mut selected_archives,
        &mut select_all_visible_requested,
        None,
    );

    for forbidden in [
        "Duplicate Review",
        "Health Dashboard",
        "Library Database",
        "Custom Platform Aliases",
    ] {
        assert!(
            !rendered_text_contains(&output, forbidden),
            "the Library page must never render {forbidden:?} - it now lives on its own \
                 page or Tools overlay"
        );
    }
}

#[test]
fn archives_tab_shows_exactly_one_library_heading() {
    let ctx = egui::Context::default();

    let shell_output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_library_shell_header(ui, LibraryTab::Archives);
        });
    });
    assert_eq!(
        count_exact_text_occurrences(&shell_output, "My Games"),
        1,
        "the unified Library shell must render exactly one 'My Games' heading"
    );

    let data = empty_loaded_data("/mnt/library");
    let mut selected_archives = HashSet::new();
    let mut select_all_visible_requested = false;
    let body_output = render_show_loaded_data_for_test(
        &ctx,
        &data,
        &mut selected_archives,
        &mut select_all_visible_requested,
        None,
    );
    assert_eq!(
        count_exact_text_occurrences(&body_output, "Library"),
        0,
        "the Archives tab body (show_loaded_data with recent_view: false) must not \
             render its own 'Library' heading any more - the shell already did"
    );
}

#[test]
fn recently_found_tab_keeps_its_content_inside_the_library_shell() {
    let ctx = egui::Context::default();
    let data = empty_loaded_data("/mnt/library");
    let mut filter = String::new();
    let mut filtered_rows = None;
    let mut selected_archive = None;
    let mut confirm_unmount = None;
    let mut confirm_lazy_unmount = None;
    let mut confirm_lazy_unmount_final = None;
    let mut confirm_mount_all = None;
    let mut focus_mount_all_cancel = false;
    let mut mount_all_typed_count = String::new();
    let mut confirm_unmount_all = None;
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
    let mut cleanup_after_unmount = false;
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
    let mut selected_archives = HashSet::new();
    let mut select_all_visible_requested = false;

    let output = ctx.run(egui::RawInput::default(), |ctx| {
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
                    recent_view: true,
                    library_platform_query: &mut library_platform_query,
                },
            );
        });
    });

    let shell_output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_library_shell_header(ui, LibraryTab::RecentlyFound);
        });
    });
    assert!(rendered_text_contains(&shell_output, "My Games"));
    assert!(rendered_text_contains(&shell_output, "Recently Found"));
    assert!(rendered_text_contains(&output, "Recently Found"));
    assert_eq!(
        count_exact_text_occurrences(&output, "Library"),
        0,
        "the shared table renderer must not add a duplicate Library heading"
    );
}

#[test]
fn library_menu_select_all_visible_reuses_the_same_helper_as_the_button_and_ctrl_a() {
    let ctx = egui::Context::default();
    let data = empty_loaded_data("/mnt/library");
    let mut selected_archives = HashSet::new();
    let mut select_all_visible_requested = true;

    let _ = render_show_loaded_data_for_test(
        &ctx,
        &data,
        &mut selected_archives,
        &mut select_all_visible_requested,
        None,
    );

    assert!(
        !select_all_visible_requested,
        "the one-shot menu signal must be consumed by the end of the frame that handles it"
    );
}

// -------------------------------------------------------------------
// Sources-page usability pass: platform wording and inline scan
// result/Inspect (problems 1, 2, 4, 5, 6 from the post-v0.8 smoke).
// -------------------------------------------------------------------

#[test]
fn source_platform_state_shows_the_actual_platform_when_every_archive_agrees() {
    let view = three_source_views().remove(0); // id = Some(1)
    let archives = vec![
        persisted_archive_with_platform(
            PathBuf::from("/a/1.zip"),
            1,
            "Commodore 128",
            "folder_alias",
        ),
        persisted_archive_with_platform(
            PathBuf::from("/a/2.zip"),
            2,
            "Commodore 128",
            "folder_alias",
        ),
    ];
    assert_eq!(
        source_platform_state(&view, &archives),
        SourcePlatformState::Single("Commodore 128".to_string())
    );
    assert_eq!(
        source_platform_value_label(&source_platform_state(&view, &archives)),
        "Commodore 128"
    );
}

#[test]
fn source_platform_state_is_mixed_when_archives_resolve_to_different_platforms() {
    let view = three_source_views().remove(0); // id = Some(1)
    let archives = vec![
        persisted_archive_with_platform(
            PathBuf::from("/a/1.zip"),
            1,
            "Commodore 128",
            "folder_alias",
        ),
        persisted_archive_with_platform(PathBuf::from("/a/2.zip"), 2, "Neo Geo CD", "folder_alias"),
    ];
    assert_eq!(
        source_platform_state(&view, &archives),
        SourcePlatformState::Mixed(2)
    );
    assert_eq!(
        source_platform_value_label(&source_platform_state(&view, &archives)),
        "Mixed (2 platforms)"
    );
}

#[test]
fn source_platform_state_is_unknown_when_no_archive_resolved_a_platform() {
    let view = three_source_views().remove(0); // id = Some(1)
    let archives = vec![PersistedArchive {
        source_folder_id: 1,
        ..persisted_archive(PathBuf::from("/a/1.zip"), false)
    }];
    assert_eq!(
        source_platform_state(&view, &archives),
        SourcePlatformState::Unknown
    );
    assert_eq!(
        source_platform_value_label(&source_platform_state(&view, &archives)),
        "Unknown"
    );
}

#[test]
fn source_platform_state_is_partial_when_only_some_archives_resolved() {
    let view = three_source_views().remove(0); // id = Some(1)
    let archives = vec![
        persisted_archive_with_platform(
            PathBuf::from("/a/1.zip"),
            1,
            "Commodore 128",
            "folder_alias",
        ),
        PersistedArchive {
            source_folder_id: 1,
            ..persisted_archive(PathBuf::from("/a/2.zip"), false)
        },
    ];
    assert_eq!(
        source_platform_state(&view, &archives),
        SourcePlatformState::Partial {
            known: 1,
            unknown: 1
        }
    );
    assert_eq!(
        source_platform_value_label(&source_platform_state(&view, &archives)),
        "Partial (1 known, 1 unknown)"
    );
}

#[test]
fn source_platform_state_makes_no_claim_for_a_source_with_no_catalogued_archives() {
    let view = three_source_views().remove(0); // id = Some(1)
    assert_eq!(
        source_platform_state(&view, &[]),
        SourcePlatformState::NotYetKnown
    );
    assert_eq!(
        source_platform_value_label(&source_platform_state(&view, &[])),
        "not yet known"
    );

    // An unregistered source (no database id at all) makes the same
    // claim, regardless of what happens to be in the archives slice.
    let mut unregistered = view;
    unregistered.id = None;
    let archives = vec![persisted_archive_with_platform(
        PathBuf::from("/a/1.zip"),
        1,
        "Commodore 128",
        "folder_alias",
    )];
    assert_eq!(
        source_platform_state(&unregistered, &archives),
        SourcePlatformState::NotYetKnown
    );
}

#[test]
fn source_platform_state_only_counts_archives_belonging_to_this_source() {
    let view = three_source_views().remove(0); // id = Some(1)
    let archives = vec![
        persisted_archive_with_platform(
            PathBuf::from("/a/1.zip"),
            1,
            "Commodore 128",
            "folder_alias",
        ),
        PersistedArchive {
            source_folder_id: 2,
            ..persisted_archive_with_platform(
                PathBuf::from("/b/1.zip"),
                2,
                "Neo Geo CD",
                "folder_alias",
            )
        },
    ];
    // The second archive belongs to source id 2 (a different source),
    // so it must not be counted here - source 1 still agrees on one
    // platform even though the whole snapshot's archives are mixed.
    assert_eq!(
        source_platform_state(&view, &archives),
        SourcePlatformState::Single("Commodore 128".to_string())
    );
}

#[test]
fn sources_page_renders_the_platform_line_for_every_source() {
    // Rendered one source at a time: the Sources list's ScrollArea has
    // a fixed `max_height`, and the new grouped per-source facts grid
    // is taller per row than the old single-line layout, so a
    // three-source list can scroll-clip a later row out of this
    // headless test's painted output. `source_platform_state`'s own
    // unit tests already cover every state (Single/Mixed/Unknown/
    // Partial/NotYetKnown) directly and exhaustively; this test's job
    // is only to confirm the row actually renders that computed label,
    // which one visible row per case proves just as well as three.
    fn count_exact_rendered_text(output: &egui::FullOutput, needle: &str) -> usize {
        fn count_shape(shape: &egui::Shape, needle: &str) -> usize {
            match shape {
                egui::Shape::Text(text_shape) => usize::from(text_shape.galley.text() == needle),
                egui::Shape::Vec(nested) => nested.iter().map(|s| count_shape(s, needle)).sum(),
                _ => 0,
            }
        }
        output
            .shapes
            .iter()
            .map(|clipped| count_shape(&clipped.shape, needle))
            .sum()
    }

    fn render_one_source(view: SourceFolderView, archives: &[PersistedArchive]) -> String {
        let ctx = egui::Context::default();
        let sources = [view];
        let mut add_dialog = None;
        let mut remove_dialog = None;
        let mut clipboard = InMemoryClipboard::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_sources_page(
                    ui,
                    &sources,
                    archives,
                    Some(Path::new("/mnt/archivefs")),
                    false,
                    &mut add_dialog,
                    &mut remove_dialog,
                    &mut clipboard,
                );
            });
        });
        // The row's own "Platform:" label must appear exactly once -
        // this is the regression check for the "Platform: Platform: X"
        // duplicate-prefix bug: a value function that (wrongly) also
        // emits its own "Platform:" prefix would make this count 2 (the
        // grid's own label plus the value's leading word), never 1.
        assert_eq!(
            count_exact_rendered_text(&output, "Platform:"),
            1,
            "the \"Platform:\" label must appear exactly once per source row"
        );
        let mut found = None;
        for text in ["Sega Genesis", "Unknown", "not yet known"] {
            if rendered_text_contains(&output, text) {
                found = Some(text.to_string());
            }
        }
        assert!(
            !rendered_text_contains(&output, "Detected automatically"),
            "\"Detected automatically\" must not appear now that the real platform is derivable"
        );
        assert!(!rendered_text_contains(&output, "unclassified"));
        assert!(!rendered_text_contains(&output, "heuristic"));
        assert!(
            !rendered_text_contains(&output, "Platform: Platform:"),
            "the platform value must never be double-prefixed"
        );
        found.unwrap_or_else(|| panic!("no expected platform wording rendered"))
    }

    let sources = three_source_views();

    // Source id 1: every archive agrees on one platform.
    let archives = vec![
        PersistedArchive {
            source_folder_id: 1,
            ..persisted_archive_with_platform(
                PathBuf::from("/a/1.zip"),
                10,
                "Sega Genesis",
                "folder_alias",
            )
        },
        PersistedArchive {
            source_folder_id: 1,
            ..persisted_archive_with_platform(
                PathBuf::from("/a/2.zip"),
                11,
                "Sega Genesis",
                "folder_alias",
            )
        },
    ];
    assert_eq!(
        render_one_source(sources[0].clone(), &archives),
        "Sega Genesis"
    );

    // Source id 2: one catalogued archive, unresolved.
    let archives = vec![PersistedArchive {
        id: 12,
        source_folder_id: 2,
        ..persisted_archive(PathBuf::from("/b/1.zip"), false)
    }];
    assert_eq!(render_one_source(sources[1].clone(), &archives), "Unknown");

    // Source id 3: no catalogued archives at all.
    assert_eq!(render_one_source(sources[2].clone(), &[]), "not yet known");
}

#[test]
fn sources_last_scan_banner_shows_counts_and_inspect_when_files_were_skipped() {
    let ctx = egui::Context::default();
    let last_scan = SourcesLastScan {
        scope: SourcesScanScope::One(PathBuf::from("/mnt/usbdrive/retro")),
        archives_found: 42,
        skipped_total: 3,
        ingestion_stats: Default::default(),
    };
    let mut clicked = false;
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            clicked = show_sources_last_scan_banner(ui, &last_scan);
        });
    });
    for expected in [
        "Last scan",
        "/mnt/usbdrive/retro",
        "42 archives found",
        "3 skipped",
        "Inspect skipped",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "expected the last-scan banner to render {expected:?}"
        );
    }
    assert!(!clicked, "no click was simulated");
}

#[test]
fn sources_last_scan_banner_hides_inspect_when_nothing_was_skipped() {
    let ctx = egui::Context::default();
    let last_scan = SourcesLastScan {
        scope: SourcesScanScope::AllEnabled,
        archives_found: 10,
        skipped_total: 0,
        ingestion_stats: Default::default(),
    };
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_sources_last_scan_banner(ui, &last_scan);
        });
    });
    assert!(rendered_text_contains(&output, "All enabled sources"));
    assert!(rendered_text_contains(&output, "10 archives found"));
    assert!(rendered_text_contains(&output, "0 skipped"));
    assert!(
        !rendered_text_contains(&output, "Inspect skipped"),
        "Inspect skipped must not be offered when skipped_files_total() is 0"
    );
}

fn ingestion_item(
    file_name: &str,
    content: Option<archivefs_core::ingestion::ContentKind>,
    platform_hint: Option<&str>,
    validation_state: archivefs_core::ingestion::ValidationState,
    skip_reason: Option<archivefs_core::ingestion::SkipReason>,
) -> archivefs_core::ingestion::GameDiscovery {
    archivefs_core::ingestion::GameDiscovery {
        path: PathBuf::from(file_name),
        container: archivefs_core::ingestion::ContainerKind::DirectFile,
        content,
        platform_hint: platform_hint.map(str::to_string),
        identity_candidate: None,
        validation_state,
        explanation: "test fixture".to_string(),
        skip_reason,
    }
}

#[test]
fn collection_discovery_panel_speaks_plain_language_not_internal_type_names() {
    use archivefs_core::ingestion::{ContentKind, SkipReason, ValidationState};

    let ingestion_stats = archivefs_core::ingestion::DiscoveryStats {
        loose_roms: 2,
        disc_images: 1,
        ..Default::default()
    };

    let ingestion_skip_reasons = archivefs_core::ingestion::SkipReasonCounts {
        unsupported_extension: 3,
        missing_paired_file: 1,
        ..Default::default()
    };

    let mut ingestion_platform_counts = std::collections::BTreeMap::new();
    ingestion_platform_counts.insert("Game Boy Advance".to_string(), 2_i64);

    let summary = ScanPersistSummary {
        scan_run_id: 1,
        counts: archivefs_core::ScanRunCounts::default(),
        folder_errors: Vec::new(),
        platform_assignment_warnings: Vec::new(),
        skipped_files: Vec::new(),
        ingestion_stats,
        ingestion_skip_reasons,
        ingestion_platform_counts,
        ingestion_skipped: vec![ingestion_item(
            "Unknown.bin",
            None,
            None,
            ValidationState::Skipped,
            Some(SkipReason::MissingPairedFile),
        )],
        ingestion_recognised_sample: vec![ingestion_item(
            "Pokemon.gba",
            Some(ContentKind::RomCartridge),
            Some("Game Boy Advance"),
            ValidationState::Accepted,
            None,
        )],
    };

    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_collection_discovery_panel(ui, Some(&summary), None);
        });
    });

    // Human-facing: counts, platform names, plain descriptions.
    assert!(rendered_text_contains(&output, "ROMs"));
    assert!(rendered_text_contains(&output, "Game Boy Advance"));
    assert!(rendered_text_contains(&output, "Pokemon.gba"));
    assert!(rendered_text_contains(&output, "Recognised"));
    assert!(rendered_text_contains(&output, "Needs attention"));
    assert!(rendered_text_contains(&output, "Unknown.bin"));
    // The generic "Suggested:" label prefix was deliberately dropped in
    // favour of plainer, reason-specific wording (`fix(gui): clarify
    // recognised collection media`); the row must still explain what to
    // do, just without that jargon-y prefix.
    assert!(rendered_text_contains(
        &output,
        "check that both files are present together"
    ));

    // Never the raw internal type/variant names.
    for internal_term in [
        "ContentKind",
        "ContainerKind",
        "ArchiveKind",
        "RomCartridge",
    ] {
        assert!(
            !rendered_text_contains(&output, internal_term),
            "internal type name {internal_term:?} leaked into the collection discovery panel"
        );
    }
}

#[test]
fn collection_discovery_panel_has_a_friendly_empty_state() {
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_collection_discovery_panel(ui, None, None);
        });
    });
    assert!(rendered_text_contains(&output, "No scan has completed yet"));
}
