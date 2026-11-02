use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use archivefs_core::{
    Database, GameCollection, GameCollectionItem, GameCollectionKind, PersistedArchive,
};

use super::super::library::{Filter, Library, ReadinessFilter};
use super::{CollectionSort, Member, State, View, build_views, scoped_indices};

fn archive(id: i64, title: &str, platform: &str, source: i64) -> PersistedArchive {
    PersistedArchive {
        id,
        source_folder_id: source,
        relative_path: format!("{title}-{id}.zip").into(),
        absolute_path: format!("/fixture/{title}-{id}.zip").into(),
        archive_kind: "zip".into(),
        display_name: title.into(),
        normalized_name: title.to_lowercase(),
        size_bytes: Some(id as u64 + 1),
        modified_time_unix_seconds: Some(1),
        platform: Some(platform.into()),
        platform_source: Some("fixture".into()),
        last_known_health: "pending".into(),
        last_seen_at: "fixture".into(),
        last_verified_missing_at: None,
        identity_report: None,
    }
}

fn item(stored: i64, resolved: Option<i64>, added: &str) -> GameCollectionItem {
    GameCollectionItem {
        stored_archive_id: stored,
        stored_created_at: "created".into(),
        resolved_archive_id: resolved,
        resolved_created_at: resolved.map(|_| "created".into()),
        title: format!("Saved {stored}"),
        platform: Some("SNES".into()),
        absolute_path: PathBuf::from(format!("/old/saved-{stored}.zip")),
        added_at: added.into(),
    }
}

fn favourites(items: Vec<GameCollectionItem>) -> Vec<GameCollection> {
    vec![GameCollection {
        id: 1,
        kind: GameCollectionKind::Favourites,
        name: "Favourites".into(),
        items,
    }]
}

fn library() -> Library {
    Library::new(vec![
        archive(1, "Mario", "SNES", 1),
        archive(2, "Zelda", "SNES", 1),
        archive(3, "Sonic", "MegaDrive", 1),
        archive(4, "Mario", "MegaDrive", 1),
    ])
}

fn view_of(library: &Library, ids: &[(i64, &str)]) -> View {
    let items = ids
        .iter()
        .map(|(id, added)| item(*id, Some(*id), added))
        .collect();
    build_views(&favourites(items), library).0.remove(0)
}

fn titles(library: &Library, indices: &[usize]) -> Vec<(String, i64)> {
    indices
        .iter()
        .map(|index| {
            let game = &library.games[*index];
            (game.title.clone(), game.archive.id)
        })
        .collect()
}

#[test]
fn a_saved_game_with_no_row_is_listed_as_gone_and_not_as_a_member() {
    let (views, moves) = build_views(&favourites(vec![item(99, None, "a")]), &library());
    assert!(views[0].members.is_empty());
    assert_eq!(views[0].gone[0].title, "Saved 99");
    assert_eq!(views[0].gone[0].file_name, "saved-99.zip");
    assert!(moves.is_empty());
}

#[test]
fn a_row_this_library_does_not_list_is_gone_until_it_does() {
    let (views, _) = build_views(&favourites(vec![item(50, Some(50), "a")]), &library());
    assert!(views[0].members.is_empty());
    assert_eq!(views[0].gone.len(), 1);
}

#[test]
fn a_superseded_row_shows_its_current_game_once() {
    let mut old = archive(10, "Mario", "SNES", 1);
    old.relative_path = "old/mario.zip".into();
    old.absolute_path = "/fixture/old/mario.zip".into();
    old.last_verified_missing_at = Some("then".into());
    let mut new = archive(11, "Mario", "SNES", 1);
    new.relative_path = "new/mario.zip".into();
    new.absolute_path = "/fixture/new/mario.zip".into();
    new.size_bytes = old.size_bytes;
    let library = Library::new(vec![old, new]);
    assert_eq!(library.resolve(10), 11);
    let items = vec![item(10, Some(10), "a"), item(11, Some(11), "b")];
    let (views, moves) = build_views(&favourites(items), &library);
    assert_eq!(views[0].members.len(), 1);
    assert_eq!(views[0].members[&11].stored.len(), 2);
    assert!(moves.is_empty(), "derived links only steer the display");
}

#[test]
fn same_titled_games_are_separate_members_and_counted_by_system() {
    let library = library();
    let view = view_of(&library, &[(1, "a"), (4, "b")]);
    assert_eq!(view.members.len(), 2);
    assert_eq!(
        view.platforms,
        vec![("MegaDrive".to_string(), 1), ("SNES".to_string(), 1)]
    );
}

#[test]
fn search_system_and_readiness_filters_apply_inside_a_collection() {
    let library = library();
    let view = view_of(&library, &[(1, "a"), (2, "b"), (3, "c")]);
    let sort = CollectionSort::TitleAscending;
    let list = |filter: &Filter| titles(&library, &scoped_indices(&library, &view, filter, sort));
    assert_eq!(list(&Filter::default()).len(), 3);
    let search = Filter {
        search: "ZEL".into(),
        ..Filter::default()
    };
    assert_eq!(list(&search), vec![("Zelda".to_string(), 2)]);
    let system = Filter {
        platform: "MegaDrive".into(),
        ..Filter::default()
    };
    assert_eq!(list(&system), vec![("Sonic".to_string(), 3)]);
    let attention = Filter {
        readiness: ReadinessFilter::NeedsAttention,
        ..Filter::default()
    };
    assert!(list(&attention).is_empty());
}

#[test]
fn every_sort_order_is_stable_and_matches_its_label() {
    let library = library();
    let view = view_of(&library, &[(1, "2026-01"), (2, "2026-03"), (3, "2026-02")]);
    let ids = |sort| -> Vec<i64> {
        titles(
            &library,
            &scoped_indices(&library, &view, &Filter::default(), sort),
        )
        .into_iter()
        .map(|(_, id)| id)
        .collect()
    };
    assert_eq!(ids(CollectionSort::TitleAscending), vec![1, 3, 2]);
    assert_eq!(ids(CollectionSort::TitleDescending), vec![2, 3, 1]);
    assert_eq!(ids(CollectionSort::System), vec![3, 1, 2]);
    assert_eq!(ids(CollectionSort::RecentlyAdded), vec![2, 3, 1]);
}

#[test]
fn a_saved_preferences_file_without_a_sort_still_loads() {
    let preferences: super::super::backend::Preferences =
        serde_json::from_str(r#"{"welcome_dismissed":true}"#).unwrap();
    assert_eq!(preferences.collection_sort, CollectionSort::TitleAscending);
}

#[test]
fn the_page_filter_walks_members_not_the_library_at_133000_games() {
    let archives: Vec<_> = (1..=133_000)
        .map(|id| archive(id, &format!("Game {id:06}"), "SNES", 1))
        .collect();
    let library = Library::new(archives);
    let members: HashMap<i64, Member> = (1..=5_000)
        .map(|id| {
            (
                id * 26,
                Member {
                    stored: Vec::new(),
                    added_at: "a".into(),
                },
            )
        })
        .collect();
    let view = View {
        id: 1,
        favourites: true,
        name: "Favourites".into(),
        members,
        gone: Vec::new(),
        platforms: Vec::new(),
    };
    let started = Instant::now();
    for _ in 0..100 {
        let indices = scoped_indices(
            &library,
            &view,
            &Filter::default(),
            CollectionSort::RecentlyAdded,
        );
        assert_eq!(indices.len(), 5_000);
    }
    // Generous for a debug build: 100 lists is far more than one click needs.
    assert!(started.elapsed() < Duration::from_secs(10));
    let started = Instant::now();
    for id in 0..100_000 {
        std::hint::black_box(view.members.contains_key(&id));
    }
    assert!(started.elapsed() < Duration::from_millis(500));
}

// The worker against a real disposable database, as the App drives it.

struct Fixture {
    _temp: tempfile::TempDir,
    path: PathBuf,
    library: Arc<Library>,
    ids: Vec<i64>,
    state: State,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    for name in ["Alpha.zip", "Beta.zip"] {
        std::fs::write(source.join(name), name).unwrap();
    }
    let path = temp.path().join("library.sqlite3");
    let mut database = Database::open_or_create(&path).unwrap();
    archivefs_core::scan_and_persist(
        &mut database,
        &archivefs_core::Config {
            source_folders: vec![source],
            mount_root: temp.path().join("mount"),
            ratarmount_bin: "ratarmount".into(),
            master_rom_root: None,
        },
        "fixture",
    )
    .unwrap();
    let archives = database.load_archives().unwrap();
    database.close().unwrap();
    let ids = archives.iter().map(|archive| archive.id).collect();
    let library = Arc::new(Library::new(archives));
    let state = State::with_database(Some(path.clone()), eframe::egui::Context::default());
    let mut fixture = Fixture {
        _temp: temp,
        path,
        library,
        ids,
        state,
    };
    let library = fixture.library.clone();
    fixture.state.library_changed(&library);
    fixture.settle();
    fixture
}

impl Fixture {
    fn settle(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.state.pending > 0 {
            assert!(Instant::now() < deadline, "worker did not answer");
            self.state.poll(&self.library);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn favourites_id(&self) -> i64 {
        self.state.favourites().unwrap().id
    }
}

#[test]
fn a_star_shows_at_once_saves_and_is_there_after_a_restart() {
    let mut f = fixture();
    let favourites = f.favourites_id();
    let game = f.ids[0];
    let library = f.library.clone();
    f.state.toggle(favourites, game, &library);
    assert!(
        f.state.is_favourite(game),
        "on screen before the worker ran"
    );
    f.settle();
    assert!(f.state.is_favourite(game));

    let mut restarted = State::with_database(Some(f.path.clone()), Default::default());
    restarted.library_changed(&f.library);
    f.state = restarted;
    f.settle();
    assert!(f.state.is_favourite(game));

    f.state.toggle(favourites, game, &library);
    assert!(!f.state.is_favourite(game));
    f.settle();
    assert!(!f.state.is_favourite(game));
}

#[test]
fn quick_repeated_clicks_end_on_the_last_click() {
    let mut f = fixture();
    let favourites = f.favourites_id();
    let game = f.ids[1];
    let library = f.library.clone();
    for _ in 0..7 {
        f.state.toggle(favourites, game, &library);
    }
    assert!(f.state.is_favourite(game));
    f.settle();
    assert!(f.state.is_favourite(game));
    let saved = Database::open_read_only(&f.path).unwrap();
    assert_eq!(saved.load_game_collections().unwrap()[0].items.len(), 1);
}

#[test]
fn a_failed_save_puts_back_what_is_saved_and_says_so() {
    let mut f = fixture();
    let favourites = f.favourites_id();
    let game = f.ids[0];
    // Make the database refuse this write, with reads still working.
    let c = rusqlite::Connection::open(&f.path).unwrap();
    c.execute_batch(
        "CREATE TRIGGER reject BEFORE INSERT ON game_collection_items \
         BEGIN SELECT RAISE(ABORT,'simulated'); END;",
    )
    .unwrap();
    drop(c);
    let library = f.library.clone();
    f.state.toggle(favourites, game, &library);
    assert!(f.state.is_favourite(game));
    f.settle();
    assert!(!f.state.is_favourite(game));
    assert!(f.state.message.as_deref().unwrap().contains("not saved"));
}

#[test]
fn deleting_the_listed_collection_returns_to_all_games() {
    let mut f = fixture();
    f.state.open_after_create = true;
    f.state.change(super::Change::Create {
        name: "Mine".into(),
        game: None,
    });
    f.settle();
    let id = f.state.scope.expect("a new collection opens");
    assert_eq!(f.state.scoped().unwrap().name, "Mine");
    f.state.change(super::Change::Delete {
        collection: id,
        seen: "Mine".into(),
    });
    f.settle();
    assert_eq!(f.state.scope, None);
}

#[test]
fn a_duplicate_name_is_refused_with_the_reason_and_nothing_changes() {
    let mut f = fixture();
    f.state.change(super::Change::Create {
        name: "Favourites".into(),
        game: None,
    });
    f.settle();
    assert!(
        f.state
            .message
            .as_deref()
            .unwrap()
            .contains("already exists")
    );
    assert_eq!(f.state.views.len(), 1);
}

#[test]
fn members_use_only_ids_the_library_lists() {
    let library = library();
    let ids: HashSet<i64> = [1, 999].into();
    assert_eq!(library.filter_members(&Filter::default(), &ids).len(), 1);
}

// The real page, drawn and clicked.

fn star_app(context: &eframe::egui::Context) -> super::super::App {
    let mut app = super::super::tests::review_app(
        context,
        vec![
            (1, "Alpha", "SNES"),
            (2, "Beta", "SNES"),
            (3, "Gamma", "PSP"),
        ],
    );
    app.simple = super::super::simple_shell::ShellState::default();
    app.collections.saved = favourites(vec![item(2, Some(2), "a")]);
    let library = app.library.clone();
    app.collections.library_changed(&library);
    app.router.current = super::super::routes::Route::BrowsePlay;
    app
}

fn frame(
    context: &eframe::egui::Context,
    app: &mut super::super::App,
    events: Vec<eframe::egui::Event>,
) -> eframe::egui::FullOutput {
    context.run(
        eframe::egui::RawInput {
            screen_rect: Some(eframe::egui::Rect::from_min_size(
                eframe::egui::Pos2::ZERO,
                eframe::egui::vec2(1400.0, 900.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| app.show(ctx),
    )
}

/// Every lone star drawn, top to bottom, left to right.
fn stars(output: &eframe::egui::FullOutput) -> Vec<eframe::egui::Pos2> {
    fn walk(shape: &eframe::egui::Shape, found: &mut Vec<eframe::egui::Pos2>) {
        match shape {
            eframe::egui::Shape::Text(text) if text.galley.text() == super::STAR => {
                found.push(text.pos + text.galley.rect.center().to_vec2());
            }
            eframe::egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    output
        .shapes
        .iter()
        .for_each(|clipped| walk(&clipped.shape, &mut found));
    found.sort_by(|a, b| (a.y, a.x).partial_cmp(&(b.y, b.x)).unwrap());
    found
}

#[test]
fn clicking_a_star_favourites_the_game_without_opening_it() {
    use eframe::egui::{Event, PointerButton};
    let context = eframe::egui::Context::default();
    let mut app = star_app(&context);
    let route = app.router.current.clone();
    frame(&context, &mut app, vec![]);
    let drawn = frame(&context, &mut app, vec![]);
    let found = stars(&drawn);
    assert_eq!(found.len(), 3, "one star for each game");
    // Games are in title order, so the first card is Alpha.
    let target = found[0];
    let click = |pressed| Event::PointerButton {
        pos: target,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(&context, &mut app, vec![Event::PointerMoved(target)]);
    frame(&context, &mut app, vec![click(true)]);
    frame(&context, &mut app, vec![click(false)]);
    assert!(app.collections.is_favourite(1));
    assert_eq!(app.router.current, route, "the star must not open the game");
}

#[test]
fn the_favourites_view_lists_members_with_counts_and_leaves_the_shared_filter_alone() {
    let context = eframe::egui::Context::default();
    let mut app = star_app(&context);
    let before = app.filter.clone();
    let favourites = app.collections.favourites().unwrap().id;
    app.choose_scope(Some(favourites));
    frame(&context, &mut app, vec![]);
    let output = frame(&context, &mut app, vec![]);
    let strings = super::super::tests::text(&output);
    assert!(strings.iter().any(|s| s == "Beta"), "{strings:?}");
    assert!(!strings.iter().any(|s| s == "Alpha"), "{strings:?}");
    assert!(strings.iter().any(|s| s.contains("1 game")), "{strings:?}");
    assert_eq!(app.filter, before);
}

#[test]
fn choosing_a_collection_drops_a_system_it_has_no_games_for() {
    let context = eframe::egui::Context::default();
    let mut app = star_app(&context);
    app.filter.platform = "PSP".into();
    let favourites = app.collections.favourites().unwrap().id;
    app.choose_scope(Some(favourites));
    assert!(app.filter.platform.is_empty());
}

#[test]
fn an_empty_favourites_view_says_how_to_add_one() {
    let context = eframe::egui::Context::default();
    let mut app = star_app(&context);
    app.collections.saved = favourites(Vec::new());
    let library = app.library.clone();
    app.collections.library_changed(&library);
    let favourites = app.collections.favourites().unwrap().id;
    app.choose_scope(Some(favourites));
    frame(&context, &mut app, vec![]);
    let strings = super::super::tests::text(&frame(&context, &mut app, vec![]));
    assert!(
        strings.iter().any(|s| s == "No favourites yet"),
        "{strings:?}"
    );
}

// Independent review regressions.

fn superseded(evidence_journal: bool) -> (Library, GameCollectionItem) {
    let mut old = archive(10, "Mario", "SNES", 1);
    old.relative_path = "old/mario.zip".into();
    old.absolute_path = "/fixture/old/mario.zip".into();
    old.last_verified_missing_at = Some("then".into());
    let mut new = archive(11, "Mario", "SNES", 1);
    new.relative_path = "new/mario.zip".into();
    new.absolute_path = "/fixture/new/mario.zip".into();
    new.size_bytes = old.size_bytes;
    let renames = if evidence_journal {
        vec![(old.absolute_path.clone(), new.absolute_path.clone())]
    } else {
        Vec::new()
    };
    let library = Library::with_history(vec![old, new], &renames, None);
    (library, item(10, Some(10), "a"))
}

#[test]
fn a_derived_link_steers_the_display_but_never_moves_the_item() {
    let (library, item) = superseded(false);
    assert_eq!(library.resolve(10), 11);
    let (views, moves) = build_views(&favourites(vec![item]), &library);
    assert!(views[0].members.contains_key(&11));
    assert!(moves.is_empty());
}

#[test]
fn a_recorded_rename_moves_the_item() {
    let (library, item) = superseded(true);
    let (_, moves) = build_views(&favourites(vec![item]), &library);
    assert_eq!(moves.len(), 1);
    assert_eq!(moves[0].current_archive_id, 11);
}

#[test]
fn a_row_found_at_the_items_own_place_moves_the_item() {
    let (_, moves) = build_views(&favourites(vec![item(99, Some(2), "a")]), &library());
    assert_eq!(moves[0].current_archive_id, 2);
}

#[test]
fn the_delete_dialog_names_and_deletes_only_its_own_collection() {
    let context = eframe::egui::Context::default();
    let mut app = star_app(&context);
    let mut saved = favourites(Vec::new());
    for (id, name) in [(5, "Alpha list"), (6, "Beta list")] {
        saved.push(GameCollection {
            id,
            kind: GameCollectionKind::User,
            name: name.into(),
            items: Vec::new(),
        });
    }
    app.collections.saved = saved;
    let library = app.library.clone();
    app.collections.library_changed(&library);
    app.collections.scope = Some(6);
    app.collections.dialog = super::Dialog::Delete { collection: 5 };
    frame(&context, &mut app, vec![]);
    let strings = super::super::tests::text(&frame(&context, &mut app, vec![]));
    let prompt = strings.iter().find(|s| s.starts_with("Delete ")).unwrap();
    assert!(prompt.contains("Alpha list"), "{prompt}");
}

#[test]
fn a_dialog_whose_collection_has_gone_closes() {
    let context = eframe::egui::Context::default();
    let mut app = star_app(&context);
    app.collections.dialog = super::Dialog::Delete { collection: 77 };
    frame(&context, &mut app, vec![]);
    assert!(matches!(app.collections.dialog, super::Dialog::None));
}

#[test]
fn removing_a_star_before_its_add_was_read_back_still_removes_it() {
    let mut f = fixture();
    let favourites = f.favourites_id();
    let game = f.ids[0];
    let library = f.library.clone();
    f.state.toggle(favourites, game, &library);
    f.state.toggle(favourites, game, &library);
    f.settle();
    assert!(!f.state.is_favourite(game));
    let saved = Database::open_read_only(&f.path).unwrap();
    assert!(saved.load_game_collections().unwrap()[0].items.is_empty());
}

#[test]
fn a_new_row_on_the_items_own_id_and_place_moves_the_item() {
    let mut saved = item(2, Some(2), "a");
    saved.resolved_created_at = Some("rescanned".into());
    let (_, moves) = build_views(&favourites(vec![saved]), &library());
    assert_eq!(moves.len(), 1);
    assert_eq!(moves[0].current_archive_id, 2);
}

#[test]
fn a_write_failure_is_not_replaced_by_the_read_failure_after_it() {
    let mut f = fixture();
    let favourites = f.favourites_id();
    let game = f.ids[0];
    let c = rusqlite::Connection::open(&f.path).unwrap();
    c.execute_batch("DROP TABLE game_collection_items;")
        .unwrap();
    drop(c);
    let library = f.library.clone();
    f.state.toggle(favourites, game, &library);
    f.settle();
    assert!(f.state.message.as_deref().unwrap().contains("not saved"));
}

#[test]
fn a_failed_first_read_says_so_and_offers_another_try() {
    let context = eframe::egui::Context::default();
    let mut app = star_app(&context);
    app.collections.saved.clear();
    app.collections.views.clear();
    app.collections.message = Some(super::READ_FAILED.into());
    frame(&context, &mut app, vec![]);
    let strings = super::super::tests::text(&frame(&context, &mut app, vec![]));
    assert!(
        strings.iter().any(|s| s == super::READ_FAILED),
        "{strings:?}"
    );
    assert!(strings.iter().any(|s| s == "Try again"), "{strings:?}");
}

#[test]
fn the_gone_dialog_stays_on_its_own_collection_when_the_page_changes() {
    let context = eframe::egui::Context::default();
    let mut app = star_app(&context);
    let mut saved = favourites(Vec::new());
    for (id, name) in [(5, "Alpha list"), (6, "Beta list")] {
        saved.push(GameCollection {
            id,
            kind: GameCollectionKind::User,
            name: name.into(),
            items: vec![item(1000 + id, None, "2026-01-01")],
        });
    }
    app.collections.saved = saved;
    let library = app.library.clone();
    app.collections.library_changed(&library);
    app.collections.scope = Some(6);
    app.collections.dialog = super::Dialog::Gone { collection: 5 };
    frame(&context, &mut app, vec![]);
    let strings = super::super::tests::text(&frame(&context, &mut app, vec![]));
    assert!(
        strings
            .iter()
            .any(|s| s.contains("Alpha list") && s.starts_with("Saved in")),
        "{strings:?}"
    );
    assert!(strings.iter().any(|s| s == "Saved 1005"), "{strings:?}");
    assert!(!strings.iter().any(|s| s == "Saved 1006"), "{strings:?}");
}

#[test]
fn removing_a_game_while_its_item_is_being_moved_removes_the_moved_item() {
    let mut f = fixture();
    let favourites = f.favourites_id();
    let (old, new) = (f.ids[0], f.ids[1]);
    let library = f.library.clone();
    f.state.toggle(favourites, old, &library);
    f.settle();
    let stored = Database::open_read_only(&f.path)
        .unwrap()
        .load_game_collections()
        .unwrap()[0]
        .items[0]
        .stored_created_at
        .clone();
    let step = archivefs_core::CollectionReanchor {
        stored_archive_id: old,
        stored_created_at: stored,
        current_archive_id: new,
    };
    // The move lands while the page still shows the old key.
    let mut database = Database::open_existing(&f.path).unwrap();
    database
        .reanchor_collection_items(std::slice::from_ref(&step))
        .unwrap();
    database.close().unwrap();
    f.state.reanchoring = vec![step];
    f.state.toggle(favourites, old, &library);
    f.settle();
    let saved = Database::open_read_only(&f.path).unwrap();
    assert!(saved.load_game_collections().unwrap()[0].items.is_empty());
}
