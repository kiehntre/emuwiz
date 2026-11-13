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
    assert_eq!(count.label(), "Selected games (4, 2 hidden by filter)");
    // Hidden games follow the shown ones in bulk-action order.
    assert_eq!(selection.ordered(&[4, 2]), vec![4, 2, 1, 3]);
    selection.keep_only(&shown);
    assert_eq!(selected(&selection), vec![2, 4]);
    assert_eq!(selection.count(&shown).label(), "Selected games (2)");
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
fn count_label_groups_thousands_like_the_rest_of_the_library() {
    let count = SelectionCount {
        selected: 3_738,
        hidden: 1_200,
    };
    assert_eq!(
        count.label(),
        "Selected games (3,738, 1,200 hidden by filter)"
    );
    let tally = ReadinessTally {
        ready: 0,
        not_ready: 0,
        not_checked: 3_738,
    };
    assert_eq!(
        tally.line(),
        "Launch check: 0 ready · 0 not ready · 3,738 not checked"
    );
}

#[test]
fn count_label_reads_selected_games_with_the_hidden_count() {
    let count = SelectionCount {
        selected: 1,
        hidden: 1,
    };
    assert_eq!(count.label(), "Selected games (1, 1 hidden by filter)");
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
        "Selected games (3, 1 hidden by filter)",
        SEPARATE_FROM_ARTWORK,
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
fn shortcuts_select_all_and_leave_an_unclaimed_escape_alone() {
    let context = egui::Context::default();
    let run = |input: egui::RawInput| {
        let mut got = None;
        let _ = context.run(input, |context| {
            got = take_shortcut(context);
        });
        got
    };
    assert_eq!(
        run(key_press(egui::Key::A, egui::Modifiers::COMMAND)),
        Some(Shortcut::SelectAll)
    );
    // Escape is the selection's only when the shared shortcuts layer handed
    // it over (the page claimed it); otherwise it is left for Back.
    assert_eq!(
        run(key_press(egui::Key::Escape, egui::Modifiers::NONE)),
        None
    );
    // A plain A is typing, not Select all.
    assert_eq!(run(key_press(egui::Key::A, egui::Modifiers::NONE)), None);
}

const PAGE: [f32; 2] = [1280.0, 820.0];

fn page_frame(
    context: &egui::Context,
    app: &mut App,
    events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
) -> egui::FullOutput {
    page_frame_at(context, app, PAGE, events, modifiers)
}

fn page_frame_at(
    context: &egui::Context,
    app: &mut App,
    size: [f32; 2],
    events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
) -> egui::FullOutput {
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(size[0], size[1]),
            )),
            events,
            modifiers,
            ..Default::default()
        },
        |context| app.show_with_shortcuts(context),
    )
}

fn page_key(context: &egui::Context, app: &mut App, key: egui::Key, modifiers: egui::Modifiers) {
    let input = key_press(key, modifiers);
    let _ = page_frame(context, app, input.events, modifiers);
}

fn page_idle(context: &egui::Context, app: &mut App) -> egui::FullOutput {
    page_frame(context, app, Vec::new(), egui::Modifiers::NONE)
}

#[test]
fn library_page_ctrl_a_selects_shown_games_and_escape_clears_before_back() {
    use crate::gui_v2::routes::Section;
    let context = egui::Context::default();
    let mut app = crate::gui_v2::tests::fixture(&context);
    app.simple = crate::gui_v2::simple_shell::ShellState::default();
    app.library = library(&[(1, "Alpha"), (2, "Beta"), (3, "Gamma")]);
    app.loaded = true;
    app.go(Route::Section(Section::Games));
    app.go(Route::BrowsePlay);
    for _ in 0..3 {
        page_idle(&context, &mut app);
    }

    page_key(&context, &mut app, egui::Key::A, egui::Modifiers::COMMAND);
    assert_eq!(selected(&app.library_selection), vec![1, 2, 3]);
    let text = crate::gui_v2::tests::text(&page_idle(&context, &mut app));
    assert!(
        text.iter().any(|s| s == "Selected games (3)"),
        "the selection bar should show the count: {text:?}"
    );
    assert!(text.iter().any(|s| s == "Copy titles"), "{text:?}");

    // With a selection, Escape clears it and stays on the Library.
    page_key(&context, &mut app, egui::Key::Escape, egui::Modifiers::NONE);
    assert!(app.library_selection.is_empty());
    assert_eq!(app.router.current, Route::BrowsePlay);
    page_idle(&context, &mut app);

    // Nothing selected: Escape is the ordinary Back again.
    page_key(&context, &mut app, egui::Key::Escape, egui::Modifiers::NONE);
    assert_eq!(app.router.current, Route::Section(Section::Games));
}

#[test]
fn library_page_ctrl_a_is_left_to_a_focused_text_field() {
    let context = egui::Context::default();
    let mut app = crate::gui_v2::tests::fixture(&context);
    app.simple = crate::gui_v2::simple_shell::ShellState::default();
    app.library = library(&[(1, "Alpha"), (2, "Beta")]);
    app.loaded = true;
    app.go(Route::BrowsePlay);
    page_idle(&context, &mut app);
    // `/` focuses the page's search field.
    let slash = vec![
        egui::Event::Key {
            key: egui::Key::Slash,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::Text("/".into()),
    ];
    let _ = page_frame(&context, &mut app, slash, egui::Modifiers::NONE);
    page_idle(&context, &mut app);
    assert!(context.text_edit_focused(), "`/` should focus the search");
    page_key(&context, &mut app, egui::Key::A, egui::Modifiers::COMMAND);
    assert!(app.library_selection.is_empty());
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

fn games_page(context: &egui::Context) -> App {
    let mut app = crate::gui_v2::tests::fixture(context);
    app.simple = crate::gui_v2::simple_shell::ShellState::default();
    app.library = library(&[(1, "Alpha"), (2, "Beta"), (3, "Gamma")]);
    app.loaded = true;
    app.go(Route::BrowsePlay);
    for _ in 0..3 {
        page_idle(context, &mut app);
    }
    app
}

#[test]
fn the_artwork_picker_is_labelled_for_artwork_and_says_it_is_separate() {
    use crate::gui_v2::browse_play::SELECT_FOR_ARTWORK;
    let context = egui::Context::default();
    let mut app = games_page(&context);
    let text = crate::gui_v2::tests::text(&page_idle(&context, &mut app));
    assert!(text.iter().any(|s| s == SELECT_FOR_ARTWORK), "{text:?}");
    assert!(
        !text.iter().any(|s| s == "Select games"),
        "the old ambiguous label is gone: {text:?}"
    );
    assert!(crate::gui_v2::browse_play::SELECT_FOR_ARTWORK_HINT.contains("25"));
    assert!(crate::gui_v2::browse_play::SELECT_FOR_ARTWORK_HINT.contains("ScreenScraper"));
    // No general bar until something is selected.
    assert!(!text.iter().any(|s| s.starts_with(SELECTED_GAMES)));
}

#[test]
fn artwork_and_general_selections_stay_independent() {
    let context = egui::Context::default();
    let mut app = games_page(&context);
    // An artwork pick, with the artwork picker open.
    app.batch_selecting = true;
    app.batch_selection.toggle(2).unwrap();
    page_idle(&context, &mut app);

    // Ctrl+A fills the general selection only.
    page_key(&context, &mut app, egui::Key::A, egui::Modifiers::COMMAND);
    assert_eq!(selected(&app.library_selection), vec![1, 2, 3]);
    assert_eq!(app.batch_selection.len(), 1);
    assert!(app.batch_selection.contains(2));
    let text = crate::gui_v2::tests::text(&page_idle(&context, &mut app));
    assert!(
        text.iter().any(|s| s == "1 / 25 selected for artwork"),
        "{text:?}"
    );
    assert!(text.iter().any(|s| s == "Selected games (3)"), "{text:?}");

    // Clearing the general selection leaves the artwork pick and its mode.
    page_key(&context, &mut app, egui::Key::Escape, egui::Modifiers::NONE);
    assert!(app.library_selection.is_empty());
    assert!(app.batch_selecting);
    assert_eq!(app.batch_selection.len(), 1);
    assert_eq!(app.router.current, Route::BrowsePlay);

    // A general Ctrl+click never adds to the artwork pick.
    app.batch_selecting = false;
    assert!(app.library_selection_click(3, egui::Modifiers::COMMAND, &[0, 1, 2]));
    assert_eq!(app.batch_selection.len(), 1);
    assert!(!app.batch_selection.contains(3));
    // Clearing the artwork pick leaves the general selection.
    app.batch_selection.clear();
    assert!(app.library_selection.contains(3));
}

#[test]
fn the_artwork_pick_keeps_its_25_game_cap() {
    let context = egui::Context::default();
    let mut app = games_page(&context);
    let cap = archivefs_core::screenscraper_enrichment::MAX_EXPLICIT_BATCH_SIZE;
    assert_eq!(cap, 25);
    for id in 1..=cap as i64 {
        app.batch_selection.toggle(id).unwrap();
    }
    assert_eq!(app.batch_selection.toggle(26), Err(cap));
    assert_eq!(app.batch_selection.len(), cap);
    // The general selection is untouched by the artwork pick.
    assert!(app.library_selection.is_empty());
}

#[test]
fn tab_still_reaches_a_card_and_shows_focus_with_a_selection() {
    let context = egui::Context::default();
    let mut app = games_page(&context);
    page_key(&context, &mut app, egui::Key::A, egui::Modifiers::COMMAND);
    assert_eq!(app.library_selection.len(), 3);
    let mut reached = false;
    for _ in 0..200 {
        let output = page_frame(
            &context,
            &mut app,
            vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            egui::Modifiers::NONE,
        );
        if output.platform_output.events.iter().any(|event| {
            matches!(event, egui::output::OutputEvent::FocusGained(info)
                if info.label.as_deref() == Some("Beta"))
        }) {
            reached = true;
            break;
        }
    }
    assert!(reached, "Tab never reached the Beta card");
    // Focus moved; the selection did not change and nothing navigated.
    assert_eq!(app.library_selection.len(), 3);
    assert_eq!(app.router.current, Route::BrowsePlay);
}

#[test]
fn the_artwork_label_is_the_widest_that_fits() {
    use crate::gui_v2::browse_play::{ArtworkLabel, artwork_label_for};
    // spare, full_extra, medium_extra, min
    assert_eq!(
        artwork_label_for(300.0, 80.0, 40.0, 140.0),
        ArtworkLabel::Full
    );
    assert_eq!(
        artwork_label_for(200.0, 80.0, 40.0, 140.0),
        ArtworkLabel::Medium
    );
    assert_eq!(
        artwork_label_for(150.0, 80.0, 40.0, 140.0),
        ArtworkLabel::Short
    );
    assert_eq!(ArtworkLabel::Full.text(), "Select for artwork");
    assert_eq!(ArtworkLabel::Medium.text(), "Artwork (25 max)");
    assert_eq!(ArtworkLabel::Short.text(), "Artwork");
}

const NARROW: [f32; 2] = [700.0, 520.0];

fn narrow_page(context: &egui::Context) -> App {
    let mut app = crate::gui_v2::tests::fixture(context);
    app.simple = crate::gui_v2::simple_shell::ShellState::default();
    app.library = library(&[(1, "Alpha"), (2, "Beta"), (3, "Gamma")]);
    app.loaded = true;
    app.go(Route::BrowsePlay);
    for _ in 0..3 {
        let _ = page_frame_at(context, &mut app, NARROW, Vec::new(), egui::Modifiers::NONE);
    }
    app
}

#[test]
fn a_narrow_toolbar_names_the_artwork_picker_without_hover() {
    use crate::gui_v2::browse_play::{
        SELECT_FOR_ARTWORK_MEDIUM, SELECT_FOR_ARTWORK_NAME, SELECT_FOR_ARTWORK_SHORT,
    };
    let context = egui::Context::default();
    let mut app = narrow_page(&context);
    let text = crate::gui_v2::tests::text(&page_frame_at(
        &context,
        &mut app,
        NARROW,
        Vec::new(),
        egui::Modifiers::NONE,
    ));
    assert!(
        text.iter().any(|s| s == SELECT_FOR_ARTWORK_SHORT),
        "the very narrow toolbar shows the short label: {text:?}"
    );
    assert!(!text.iter().any(|s| s == SELECT_FOR_ARTWORK_MEDIUM));
    assert!(!text.iter().any(|s| s == "Artwork…"));

    // Keyboard focus announces the full name, whichever label is drawn.
    let mut announced = false;
    for _ in 0..80 {
        let output = page_frame_at(
            &context,
            &mut app,
            NARROW,
            vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            egui::Modifiers::NONE,
        );
        if output.platform_output.events.iter().any(|event| {
            matches!(event, egui::output::OutputEvent::FocusGained(info)
                if info.label.as_deref() == Some(SELECT_FOR_ARTWORK_NAME))
        }) {
            announced = true;
            break;
        }
    }
    assert!(
        announced,
        "Tab focus never announced {SELECT_FOR_ARTWORK_NAME}"
    );
    assert!(!app.batch_selecting, "focus alone must not open the picker");
}

#[test]
fn the_open_artwork_picker_states_its_purpose_on_screen() {
    let context = egui::Context::default();
    let mut app = narrow_page(&context);
    app.batch_selecting = true;
    let _ = page_frame_at(
        &context,
        &mut app,
        NARROW,
        Vec::new(),
        egui::Modifiers::NONE,
    );
    let text = crate::gui_v2::tests::text(&page_frame_at(
        &context,
        &mut app,
        NARROW,
        Vec::new(),
        egui::Modifiers::NONE,
    ));
    let purpose = crate::gui_v2::browse_play::ARTWORK_PICKER_PURPOSE;
    assert!(
        text.iter()
            .any(|s| s.contains("up to 25 games") && s.contains("ScreenScraper")),
        "the purpose ({purpose}) is shown, not only on hover: {text:?}"
    );
    assert!(
        text.iter().any(|s| s == "0 / 25 selected for artwork"),
        "{text:?}"
    );
    // Closing the picker removes the line again.
    app.batch_selecting = false;
    let _ = page_frame_at(
        &context,
        &mut app,
        NARROW,
        Vec::new(),
        egui::Modifiers::NONE,
    );
    let text = crate::gui_v2::tests::text(&page_frame_at(
        &context,
        &mut app,
        NARROW,
        Vec::new(),
        egui::Modifiers::NONE,
    ));
    assert!(!text.iter().any(|s| s.contains("then Enrich metadata")));
}

#[test]
fn a_mid_width_toolbar_uses_artwork_25_max() {
    use crate::gui_v2::browse_play::SELECT_FOR_ARTWORK_MEDIUM;
    // Wide enough for one toolbar row, too narrow for the full label: find
    // such a width and check the medium label is what is drawn there.
    let mut seen = false;
    // The band where only the medium label fits is narrow (the two labels
    // differ by a few characters), so sweep in small steps.
    for width in (600..=1400).step_by(2) {
        let context = egui::Context::default();
        let mut app = crate::gui_v2::tests::fixture(&context);
        app.simple = crate::gui_v2::simple_shell::ShellState::default();
        app.library = library(&[(1, "Alpha")]);
        app.loaded = true;
        app.go(Route::BrowsePlay);
        let size = [width as f32, 820.0];
        for _ in 0..2 {
            let _ = page_frame_at(&context, &mut app, size, Vec::new(), egui::Modifiers::NONE);
        }
        let text = crate::gui_v2::tests::text(&page_frame_at(
            &context,
            &mut app,
            size,
            Vec::new(),
            egui::Modifiers::NONE,
        ));
        if text.iter().any(|s| s == SELECT_FOR_ARTWORK_MEDIUM) {
            seen = true;
            break;
        }
    }
    assert!(
        seen,
        "no width between 900 and 1280 showed {SELECT_FOR_ARTWORK_MEDIUM}"
    );
}
