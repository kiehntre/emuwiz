use super::*;
use crate::gui_v2::tests::archive;

const PLAIN: ClickModifiers = ClickModifiers {
    toggle: false,
    range: false,
};
const CTRL: ClickModifiers = ClickModifiers {
    toggle: true,
    range: false,
};
const SHIFT: ClickModifiers = ClickModifiers {
    toggle: false,
    range: true,
};
const CTRL_SHIFT: ClickModifiers = ClickModifiers {
    toggle: true,
    range: true,
};

fn set(ids: &[i64]) -> HashSet<i64> {
    ids.iter().copied().collect()
}

fn selected(selection: &LibrarySelection) -> Vec<i64> {
    selection.ids().collect()
}

fn library(ids: &[(i64, &str)]) -> Arc<Library> {
    Arc::new(Library::new(
        ids.iter()
            .map(|(id, title)| archive(*id, title, Some("SNES")))
            .collect(),
    ))
}

#[test]
fn plain_click_opens_the_game_and_empties_the_bulk_set() {
    let order = [1, 2, 3];
    let mut selection = LibrarySelection::default();
    assert_eq!(
        selection.click(2, CTRL, None, &order),
        ClickOutcome::Selection
    );
    assert_eq!(
        selection.click(3, CTRL, None, &order),
        ClickOutcome::Selection
    );
    assert_eq!(selection.len(), 2);
    assert_eq!(
        selection.click(1, PLAIN, None, &order),
        ClickOutcome::Open(1)
    );
    assert!(selection.is_empty());
    assert_eq!(selection.anchor(), Some(1));
}

#[test]
fn ctrl_click_toggles_and_seeds_from_the_game_already_open() {
    let order = [10, 20, 30];
    let mut selection = LibrarySelection::default();
    // The open game joins the set on the first Ctrl+click.
    selection.click(30, CTRL, Some(10), &order);
    assert_eq!(selected(&selection), vec![10, 30]);
    // Toggling the same game again removes it.
    selection.click(30, CTRL, Some(10), &order);
    assert_eq!(selected(&selection), vec![10]);
    // Ctrl+click on the open game itself does not double-seed it.
    let mut other = LibrarySelection::default();
    other.click(10, CTRL, Some(10), &order);
    assert_eq!(selected(&other), vec![10]);
    other.click(10, CTRL, Some(10), &order);
    assert!(other.is_empty());
}

#[test]
fn shift_click_selects_the_shown_range_from_the_anchor_either_direction() {
    let order = [5, 4, 3, 2, 1];
    let mut selection = LibrarySelection::default();
    selection.click(4, PLAIN, None, &order);
    selection.click(2, SHIFT, Some(4), &order);
    assert_eq!(selected(&selection), vec![2, 3, 4]);
    assert_eq!(selection.anchor(), Some(4));
    // Shift again from the same anchor replaces the range, upwards this time.
    selection.click(5, SHIFT, Some(4), &order);
    assert_eq!(selected(&selection), vec![4, 5]);
}

#[test]
fn ctrl_shift_adds_a_range_and_ctrl_moves_the_anchor() {
    let order = [1, 2, 3, 4, 5, 6];
    let mut selection = LibrarySelection::default();
    selection.click(1, CTRL, None, &order);
    selection.click(5, CTRL, None, &order);
    assert_eq!(selection.anchor(), Some(5));
    selection.click(6, CTRL_SHIFT, None, &order);
    assert_eq!(selected(&selection), vec![1, 5, 6]);
    // Plain Shift from anchor 5 replaces everything with 3..=5.
    selection.click(3, SHIFT, None, &order);
    assert_eq!(selected(&selection), vec![3, 4, 5]);
}

#[test]
fn shift_click_with_the_anchor_filtered_out_selects_only_the_clicked_game() {
    let mut selection = LibrarySelection::default();
    selection.click(9, PLAIN, None, &[9, 1, 2]);
    // The filter changed: 9 is no longer shown.
    selection.click(2, SHIFT, None, &[1, 2, 3]);
    assert_eq!(selected(&selection), vec![2]);
    assert_eq!(selection.anchor(), Some(2));
}

#[test]
fn range_follows_the_shown_sort_order_not_the_id_order() {
    let mut selection = LibrarySelection::default();
    selection.click(30, PLAIN, None, &[]);
    // Sorted newest first: ids no longer ascend.
    let order = [7, 30, 2, 41, 11];
    selection.click(41, SHIFT, None, &order);
    assert_eq!(selected(&selection), vec![2, 30, 41]);
    // Re-sorted by title: the same ids keep the same games selected.
    let resorted = [41, 30, 11, 2, 7];
    let shown = set(&resorted);
    assert_eq!(
        selection.count(&shown),
        SelectionCount {
            selected: 3,
            hidden: 0
        }
    );
    assert_eq!(selection.ordered(&resorted), vec![41, 30, 2]);
}

#[test]
fn filtered_out_games_stay_selected_and_are_counted_as_hidden() {
    let mut selection = LibrarySelection::default();
    let all = [1, 2, 3, 4];
    selection.select_all(&all);
    let shown = set(&[2, 4]);
    let count = selection.count(&shown);
    assert_eq!(
        count,
        SelectionCount {
            selected: 4,
            hidden: 2
        }
    );
    assert_eq!(count.label(), "4 games selected (2 hidden by filter)");
    // Hidden games follow the shown ones in bulk-action order.
    assert_eq!(selection.ordered(&[4, 2]), vec![4, 2, 1, 3]);
    selection.keep_only(&shown);
    assert_eq!(selected(&selection), vec![2, 4]);
    assert_eq!(selection.count(&shown).label(), "2 games selected");
    // The anchor (1) was hidden, so it went too.
    assert_eq!(selection.anchor(), None);
}

#[test]
fn reload_rekeys_moved_games_and_drops_missing_ones_with_a_notice() {
    let mut selection = LibrarySelection::default();
    selection.select_all(&[1, 2, 3]);
    // 1 was replaced by current row 11; 3 is gone.
    let dropped = selection.reconcile(|id| match id {
        1 => Some(11),
        2 => Some(2),
        _ => None,
    });
    assert_eq!(dropped, 1);
    assert_eq!(selected(&selection), vec![2, 11]);
    assert_eq!(selection.anchor(), Some(11));
    assert!(
        selection
            .notice
            .as_deref()
            .is_some_and(|notice| notice.starts_with("1 selected game is no longer"))
    );
    // Two ids resolving to one current game are counted once, not dropped.
    let mut twins = LibrarySelection::default();
    twins.select_all(&[1, 11]);
    twins.reconcile(|_| Some(11));
    assert_eq!(selected(&twins), vec![11]);
}

#[test]
fn reconcile_against_a_reloaded_library_runs_once_per_library() {
    let first = library(&[(1, "Alpha"), (2, "Beta"), (3, "Gamma")]);
    let mut selection = LibrarySelection::default();
    selection.reconcile_with(&first);
    selection.select_all(&[1, 2, 3]);
    let second = library(&[(1, "Alpha"), (3, "Gamma")]);
    selection.reconcile_with(&second);
    assert_eq!(selected(&selection), vec![1, 3]);
    // Same library again: nothing re-done, the notice stays as it was.
    selection.notice = None;
    selection.reconcile_with(&second);
    assert!(selection.notice.is_none());
}

#[test]
fn shown_cache_follows_a_new_list_even_at_the_same_length() {
    let library = library(&[(1, "Alpha"), (2, "Beta"), (3, "Gamma")]);
    let mut selection = LibrarySelection::default();
    let a = Arc::new(vec![0, 1]);
    assert_eq!(selection.shown(&library, &a), &set(&[1, 2]));
    let b = Arc::new(vec![1, 2]);
    assert_eq!(selection.shown(&library, &b), &set(&[2, 3]));
}

#[test]
fn select_all_and_ranges_refuse_past_the_limit_without_truncating() {
    let big: Vec<i64> = (0..=MAX_SELECTED as i64).collect();
    let mut selection = LibrarySelection::default();
    selection.select_all(&big);
    assert!(selection.is_empty());
    assert!(selection.notice.is_some());
    selection.click(0, PLAIN, None, &big);
    selection.click(MAX_SELECTED as i64, SHIFT, None, &big);
    assert!(selection.is_empty());
    assert!(selection.notice.is_some());
    selection.select_all(&big[..MAX_SELECTED]);
    assert_eq!(selection.len(), MAX_SELECTED);
    selection.click(MAX_SELECTED as i64, CTRL, None, &big);
    assert_eq!(selection.len(), MAX_SELECTED);
    // Removing still works at the limit.
    selection.click(0, CTRL, None, &big);
    assert_eq!(selection.len(), MAX_SELECTED - 1);
}

#[test]
fn count_label_is_singular_for_one_game() {
    let count = SelectionCount {
        selected: 1,
        hidden: 1,
    };
    assert_eq!(count.label(), "1 game selected (1 hidden by filter)");
}

#[test]
fn copy_text_lists_titles_and_paths_one_per_line() {
    let library = library(&[(1, "Alpha"), (2, "Beta")]);
    let games: Vec<&Game> = [2, 1].iter().filter_map(|id| library.game(*id)).collect();
    assert_eq!(titles_text(&games), "Beta\nAlpha");
    assert_eq!(paths_text(&games), "/fixture/Beta.iso\n/fixture/Alpha.iso");
}

#[test]
fn folders_to_open_dedupes_folders_and_refuses_too_many() {
    let mut archives = Vec::new();
    for id in 0..(MAX_FOLDERS as i64 + 1) {
        let mut row = archive(id, &format!("g{id}"), Some("SNES"));
        row.absolute_path = format!("/roms/dir{id}/g{id}.iso").into();
        archives.push(row);
    }
    let mut twin = archive(100, "twin", Some("SNES"));
    twin.absolute_path = "/roms/dir0/twin.iso".into();
    archives.push(twin);
    let library = Library::new(archives);
    let pick =
        |ids: &[i64]| -> Vec<&Game> { ids.iter().filter_map(|id| library.game(*id)).collect() };
    let paths = folders_to_open(&pick(&[0, 100, 1])).unwrap();
    assert_eq!(
        paths,
        vec![
            PathBuf::from("/roms/dir0/g0.iso"),
            PathBuf::from("/roms/dir1/g1.iso")
        ]
    );
    let all: Vec<i64> = (0..=MAX_FOLDERS as i64).collect();
    let refusal = folders_to_open(&pick(&all)).unwrap_err();
    assert!(refusal.contains(&format!("{} folders", MAX_FOLDERS + 1)));
}

#[test]
fn readiness_tally_reads_only_saved_answers() {
    let library = library(&[(1, "Alpha"), (2, "Beta"), (3, "Gamma")]);
    let mut cache = ReadinessCache::default();
    cache.record(library.game(1).unwrap(), Some(CheckedReadiness::Ready));
    cache.record(
        library.game(2).unwrap(),
        Some(CheckedReadiness::NotReady("Needs BIOS")),
    );
    let games: Vec<&Game> = [1, 2, 3]
        .iter()
        .filter_map(|id| library.game(*id))
        .collect();
    let tally = ReadinessTally::of(&games, &cache);
    assert_eq!(
        tally,
        ReadinessTally {
            ready: 1,
            not_ready: 1,
            not_checked: 1
        }
    );
    assert_eq!(
        tally.line(),
        "Launch check: 1 ready · 1 not ready · 1 not checked"
    );
}

fn render_bar(view: &BarView<'_>) -> String {
    let context = egui::Context::default();
    let output = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            let _ = show_bar(ui, view);
        });
    });
    crate::transaction_outcome::test_support::rendered_text(&output)
}

#[test]
fn bar_shows_the_count_clear_and_only_read_only_actions() {
    let text = render_bar(&BarView {
        count: SelectionCount {
            selected: 3,
            hidden: 1,
        },
        readiness: ReadinessTally {
            ready: 1,
            not_ready: 0,
            not_checked: 2,
        },
        notice: Some("Copied 3 titles."),
    });
    for expected in [
        "3 games selected (1 hidden by filter)",
        "Clear",
        "Keep only shown",
        "Copy titles",
        "Copy file paths",
        "Show in file manager",
        "Launch check: 1 ready · 0 not ready · 2 not checked",
        "actions include the hidden games",
        "Copied 3 titles.",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in {text}");
    }
    let lower = text.to_lowercase();
    for forbidden in [
        "delete",
        "move",
        "rename",
        "apply",
        "launch game",
        "play",
        "collection",
    ] {
        assert!(
            !lower.contains(forbidden),
            "bar offers {forbidden:?}: {text}"
        );
    }
    // Nothing hidden: no Keep only shown.
    let text = render_bar(&BarView {
        count: SelectionCount {
            selected: 2,
            hidden: 0,
        },
        readiness: ReadinessTally::default(),
        notice: None,
    });
    assert!(!text.contains("Keep only shown"));
}

fn key_press(key: egui::Key, modifiers: egui::Modifiers) -> egui::RawInput {
    egui::RawInput {
        modifiers,
        events: vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }],
        ..Default::default()
    }
}

#[test]
fn shortcuts_select_all_and_escape_clears_only_with_a_selection() {
    let context = egui::Context::default();
    let run = |input: egui::RawInput, has_selection: bool| {
        let mut got = None;
        let _ = context.run(input, |context| {
            got = take_shortcut(context, has_selection);
        });
        got
    };
    assert_eq!(
        run(key_press(egui::Key::A, egui::Modifiers::COMMAND), false),
        Some(Shortcut::SelectAll)
    );
    assert_eq!(
        run(key_press(egui::Key::Escape, egui::Modifiers::NONE), true),
        Some(Shortcut::Clear)
    );
    // Without a selection Escape is left for Back.
    assert_eq!(
        run(key_press(egui::Key::Escape, egui::Modifiers::NONE), false),
        None
    );
    // A plain A is typing, not Select all.
    assert_eq!(
        run(key_press(egui::Key::A, egui::Modifiers::NONE), false),
        None
    );
}

#[test]
fn app_click_keeps_plain_clicks_for_the_existing_open_path() {
    let context = egui::Context::default();
    let mut app = crate::gui_v2::tests::fixture(&context);
    app.library = library(&[(1, "Alpha"), (2, "Beta"), (3, "Gamma")]);
    app.router.current = Route::BrowsePlayGame(1);
    let list = vec![0, 1, 2];
    assert!(!app.library_selection_click(2, egui::Modifiers::NONE, &list));
    assert!(app.library_selection.is_empty());
    // Ctrl+click from the open game: both are selected and nothing navigates.
    app.router.current = Route::BrowsePlayGame(2);
    assert!(app.library_selection_click(3, egui::Modifiers::COMMAND, &list));
    assert_eq!(selected(&app.library_selection), vec![2, 3]);
    assert_eq!(app.router.current, Route::BrowsePlayGame(2));
    assert!(app.library_selection_click(1, egui::Modifiers::SHIFT, &list));
    assert_eq!(selected(&app.library_selection), vec![1, 2, 3]);
}
