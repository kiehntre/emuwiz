//! The moved-library "fix it here" card (`library_view::show_missing_library_fixit_card`,
//! reached through `show_loaded_data`).
//!
//! These cover the beginner flow for a relocated game library: a single
//! summarised card on the same screen the missing rows appear on, with
//! navigation-only "Update game folder" / "Full rescan" / "Review missing
//! games" actions and a "Clean up confirmed missing entries" action that
//! reuses the existing `confirm_remove_missing` dialog (typed-count gate +
//! explicit confirm unchanged). The catalogue-cleanup safety model itself
//! (a failed / interrupted / stale scan never marks anything missing, a
//! rediscovered game clears its own missing flag, removal is database-only
//! and never touches a file) is enforced and tested in
//! `archivefs_core::database`; this file only proves the GUI surfaces it.

use super::*;

use crate::library_view::confirmed_missing_catalogue_paths;

fn missing_row(path: &str, id: i64) -> PersistedArchive {
    PersistedArchive {
        id,
        ..persisted_archive(PathBuf::from(path), true)
    }
}

fn present_row(path: &str, id: i64) -> PersistedArchive {
    PersistedArchive {
        id,
        ..persisted_archive(PathBuf::from(path), false)
    }
}

/// move -> press -> release on the same `ctx`, driving the real
/// `show_loaded_data` each frame so egui's previous-frame hit-testing
/// works exactly as `simulate_row_click` documents.
fn click_card(
    harness: &mut RealLoadedDataHarness,
    ctx: &egui::Context,
    data: &LoadedData,
    pos: egui::Pos2,
) {
    let moved = egui::RawInput {
        events: vec![egui::Event::PointerMoved(pos)],
        ..bounded_test_input()
    };
    harness.render(ctx, data, moved);
    let press = egui::RawInput {
        events: vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        }],
        ..bounded_test_input()
    };
    harness.render(ctx, data, press);
    let release = egui::RawInput {
        events: vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }],
        ..bounded_test_input()
    };
    harness.render(ctx, data, release);
}

#[test]
fn confirmed_missing_paths_are_only_missing_sorted_and_deduped() {
    let snapshot = cached_snapshot(vec![
        missing_row("/mnt/games/zelda.zip", 1),
        present_row("/mnt/games/mario.zip", 2),
        missing_row("/mnt/games/apex.zip", 3),
        missing_row("/mnt/games/apex.zip", 4),
    ]);

    let paths = confirmed_missing_catalogue_paths(Some(&snapshot));

    assert_eq!(
        paths,
        vec![
            PathBuf::from("/mnt/games/apex.zip"),
            PathBuf::from("/mnt/games/zelda.zip"),
        ],
        "only rows the last successful scan marked missing, sorted, no duplicates, and never a present row"
    );
}

#[test]
fn confirmed_missing_paths_are_empty_without_a_snapshot() {
    assert!(confirmed_missing_catalogue_paths(None).is_empty());
}

#[test]
fn fixit_card_summarises_a_large_missing_count_into_one_cleanup_action() {
    let ctx = egui::Context::default();
    let data = empty_loaded_data("/mount");

    let archives: Vec<PersistedArchive> = (0..500)
        .map(|index| missing_row(&format!("/mnt/games/game-{index:04}.zip"), index + 1))
        .collect();

    let mut harness = RealLoadedDataHarness::new();
    harness.cached = Some(cached_snapshot_with_completed_scan(archives));
    harness.missing_removal_available = true;
    harness.render(&ctx, &data, bounded_test_input());
    let output = harness.last_output.as_ref().unwrap();

    assert!(
        rendered_text_contains(
            output,
            "500 games are missing from their previous locations."
        ),
        "the card summarises every missing entry into one headline line"
    );
    assert_eq!(
        count_exact_text_occurrences(output, "Clean up confirmed missing entries (500)"),
        1,
        "one cleanup action for the whole group - never one control per missing game"
    );
}

#[test]
fn fixit_card_is_hidden_when_nothing_is_confirmed_missing() {
    let ctx = egui::Context::default();
    let data = empty_loaded_data("/mount");

    let mut harness = RealLoadedDataHarness::new();
    harness.cached = Some(cached_snapshot_with_completed_scan(vec![present_row(
        "/mnt/games/mario.zip",
        1,
    )]));
    harness.missing_removal_available = true;
    harness.render(&ctx, &data, bounded_test_input());

    let output = harness.last_output.as_ref().unwrap();
    assert!(!rendered_text_contains(
        output,
        "missing from their previous location"
    ));
    assert!(!rendered_text_contains(
        output,
        "Clean up confirmed missing entries"
    ));
}

#[test]
fn fixit_card_is_hidden_in_recently_found_view() {
    let ctx = egui::Context::default();
    let data = empty_loaded_data("/mount");

    let mut harness = RealLoadedDataHarness::new();
    harness.cached = Some(cached_snapshot_with_completed_scan(vec![missing_row(
        "/mnt/games/zelda.zip",
        1,
    )]));
    harness.missing_removal_available = true;
    harness.recent_view = true;
    harness.render(&ctx, &data, bounded_test_input());

    assert!(!rendered_text_contains(
        harness.last_output.as_ref().unwrap(),
        "missing from its previous location",
    ));
}

#[test]
fn fixit_card_offers_rescan_and_clean_up_before_any_completed_scan_is_recorded() {
    let ctx = egui::Context::default();
    let data = empty_loaded_data("/mount");

    let mut harness = RealLoadedDataHarness::new();
    // `cached_snapshot` leaves `last_completed_scan` at None.
    harness.cached = Some(cached_snapshot(vec![missing_row(
        "/mnt/games/zelda.zip",
        1,
    )]));
    harness.missing_removal_available = true;
    harness.render(&ctx, &data, bounded_test_input());

    let output = harness.last_output.as_ref().unwrap();
    assert!(
        rendered_text_contains(output, "Rescan and clean up confirmed missing entries"),
        "without a completed scan the card asks for a rescan first, never an immediate delete"
    );
    assert_eq!(
        count_exact_text_occurrences(output, "Clean up confirmed missing entries (1)"),
        0,
    );
}

#[test]
fn fixit_card_clean_up_opens_the_confirm_dialog_for_every_confirmed_missing_path() {
    let ctx = egui::Context::default();
    let data = empty_loaded_data("/mount");

    let mut harness = RealLoadedDataHarness::new();
    harness.cached = Some(cached_snapshot_with_completed_scan(vec![
        missing_row("/mnt/games/apex.zip", 1),
        missing_row("/mnt/games/zelda.zip", 2),
        present_row("/mnt/games/mario.zip", 3),
    ]));
    harness.missing_removal_available = true;
    // Deliberately no row selection: the beginner flow must not require it.
    harness.render(&ctx, &data, bounded_test_input());
    let pos = find_exact_text_center(
        harness.last_output.as_ref().unwrap(),
        "Clean up confirmed missing entries (2)",
    )
    .expect("cleanup button is rendered");

    click_card(&mut harness, &ctx, &data, pos);

    assert_eq!(
        harness.confirm_remove_missing,
        Some(vec![
            PathBuf::from("/mnt/games/apex.zip"),
            PathBuf::from("/mnt/games/zelda.zip"),
        ]),
        "the cleanup button pre-selects exactly the confirmed-missing set, present rows excluded"
    );
    assert!(
        !matches!(
            harness.requested_action,
            Some(AppOperationRequest::RemoveMissing(_))
        ),
        "opening the dialog must not itself dispatch a removal - confirmation is still required"
    );
}

#[test]
fn fixit_card_clean_up_is_inert_while_a_catalogue_operation_is_running() {
    let ctx = egui::Context::default();
    let data = empty_loaded_data("/mount");

    let mut harness = RealLoadedDataHarness::new();
    harness.cached = Some(cached_snapshot_with_completed_scan(vec![missing_row(
        "/mnt/games/zelda.zip",
        1,
    )]));
    harness.missing_removal_available = false;
    harness.missing_removal_busy = true;
    harness.render(&ctx, &data, bounded_test_input());
    let output = harness.last_output.as_ref().unwrap();
    assert!(rendered_text_contains(
        output,
        "Cleaning up catalogue entries..."
    ));
    let pos = find_exact_text_center(output, "Clean up confirmed missing entries (1)")
        .expect("cleanup button is still rendered, just disabled");

    click_card(&mut harness, &ctx, &data, pos);

    assert!(
        harness.confirm_remove_missing.is_none(),
        "a disabled cleanup button must not open the confirmation dialog"
    );
}

#[test]
fn fixit_card_update_game_folder_only_requests_navigation() {
    let ctx = egui::Context::default();
    let data = empty_loaded_data("/mount");

    let mut harness = RealLoadedDataHarness::new();
    harness.cached = Some(cached_snapshot_with_completed_scan(vec![missing_row(
        "/mnt/games/zelda.zip",
        1,
    )]));
    harness.missing_removal_available = true;
    harness.render(&ctx, &data, bounded_test_input());
    let pos = find_exact_text_center(harness.last_output.as_ref().unwrap(), "Update game folder")
        .expect("update-folder button is rendered");

    click_card(&mut harness, &ctx, &data, pos);

    assert!(matches!(
        harness.requested_action,
        Some(AppOperationRequest::UpdateGameFolder)
    ));
    assert!(
        harness.confirm_remove_missing.is_none(),
        "updating the source folder stays an explicit, separate step - no catalogue change here"
    );
}

#[test]
fn fixit_card_full_rescan_button_requests_a_scan() {
    let ctx = egui::Context::default();
    let data = empty_loaded_data("/mount");

    let mut harness = RealLoadedDataHarness::new();
    harness.cached = Some(cached_snapshot_with_completed_scan(vec![missing_row(
        "/mnt/games/zelda.zip",
        1,
    )]));
    harness.missing_removal_available = true;
    harness.render(&ctx, &data, bounded_test_input());
    let pos = find_exact_text_center(harness.last_output.as_ref().unwrap(), "Full rescan")
        .expect("full-rescan button is rendered");

    click_card(&mut harness, &ctx, &data, pos);

    assert!(matches!(
        harness.requested_action,
        Some(AppOperationRequest::FullRescan)
    ));
}

#[test]
fn fixit_card_review_missing_games_button_requests_the_review() {
    let ctx = egui::Context::default();
    let data = empty_loaded_data("/mount");

    let mut harness = RealLoadedDataHarness::new();
    harness.cached = Some(cached_snapshot_with_completed_scan(vec![missing_row(
        "/mnt/games/zelda.zip",
        1,
    )]));
    harness.missing_removal_available = true;
    harness.render(&ctx, &data, bounded_test_input());
    let pos = find_exact_text_center(
        harness.last_output.as_ref().unwrap(),
        "Review missing games (1)",
    )
    .expect("review button is rendered");

    click_card(&mut harness, &ctx, &data, pos);

    assert!(matches!(
        harness.requested_action,
        Some(AppOperationRequest::ReviewMissingGames)
    ));
}
