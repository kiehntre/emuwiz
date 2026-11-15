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
        proposed_archive_id: None,
        title: format!("Saved {stored}"),
        platform: Some("SNES".into()),
        absolute_path: PathBuf::from(format!("/old/saved-{stored}.zip")),
        added_at: added.into(),
    }
}

/// An item whose own row is gone, with a row at its place and size.
fn proposed_item(stored: i64, proposed: i64) -> GameCollectionItem {
    GameCollectionItem {
        proposed_archive_id: Some(proposed),
        ..item(stored, None, "a")
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
    build_views(&favourites(items), library).remove(0)
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
    let views = build_views(&favourites(vec![item(99, None, "a")]), &library());
    assert!(views[0].members.is_empty());
    assert_eq!(views[0].gone[0].title, "Saved 99");
    assert_eq!(views[0].gone[0].file_name, "saved-99.zip");
    assert_eq!(views[0].gone[0].proposed, None);
}

#[test]
fn a_row_this_library_does_not_list_is_gone_until_it_does() {
    let views = build_views(&favourites(vec![item(50, Some(50), "a")]), &library());
    assert!(views[0].members.is_empty());
    assert_eq!(views[0].gone.len(), 1);
}

#[test]
fn a_row_replaced_by_name_and_size_only_proposes_its_replacement() {
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
    let views = build_views(&favourites(items), &library);
    // Owner decision 2026-11-15: a same-name-and-size link is a guess.
    assert_eq!(views[0].members.len(), 1);
    assert_eq!(views[0].members[&11].stored.len(), 1);
    assert_eq!(views[0].gone.len(), 1);
    assert_eq!(views[0].gone[0].proposed, Some(11));
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
            configured_source_roots: None,
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
fn a_recorded_rename_keeps_the_star_and_a_derived_link_is_only_proposed() {
    let (library, item) = superseded(true);
    assert_eq!(library.resolve(10), 11);
    let views = build_views(&favourites(vec![item]), &library);
    assert!(views[0].members.contains_key(&11));
    assert!(views[0].gone.is_empty());

    let (library, item) = superseded(false);
    assert_eq!(library.resolve(10), 11, "the library still shows 11 for 10");
    let views = build_views(&favourites(vec![item]), &library);
    assert!(views[0].members.is_empty(), "not starred on a guess");
    assert_eq!(views[0].gone[0].proposed, Some(11));
}

/// Owner decision 2026-11-15: a match by place, name and size is proposed,
/// never treated as the saved game, until the user confirms it.
#[test]
fn a_row_found_at_the_items_own_place_is_only_proposed() {
    let views = build_views(&favourites(vec![proposed_item(99, 2)]), &library());
    assert!(views[0].members.is_empty(), "not starred on a guess");
    assert_eq!(views[0].gone.len(), 1, "the saved item stays listed");
    assert_eq!(views[0].gone[0].title, "Saved 99");
    assert_eq!(views[0].gone[0].proposed, Some(2));
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
fn a_new_row_on_the_items_own_id_and_place_is_only_proposed() {
    // The loader gives a row that took the saved id (another creation time)
    // only as a proposal.
    let views = build_views(&favourites(vec![proposed_item(2, 2)]), &library());
    assert!(views[0].members.is_empty());
    assert_eq!(views[0].gone[0].proposed, Some(2));
}

/// Owner decision 2026-11-15: loading the library never saves an inferred
/// match; the user's confirmation does, and only for that item.
#[test]
fn loading_writes_nothing_and_a_confirmed_match_is_saved() {
    let mut f = fixture();
    let favourites = f.favourites_id();
    let game = f.ids[0];
    let library = f.library.clone();
    f.state.toggle(favourites, game, &library);
    f.settle();
    // Its own row went and the same file came back as this row.
    rusqlite::Connection::open(&f.path)
        .unwrap()
        .execute_batch(&format!(
            "UPDATE game_collection_items SET archive_id = 990000, archive_created_at = 'gone' WHERE archive_id = {game}"
        ))
        .unwrap();
    let before = std::fs::read(&f.path).unwrap();
    for _ in 0..3 {
        let mut restarted = State::with_database(Some(f.path.clone()), Default::default());
        restarted.library_changed(&library);
        f.state = restarted;
        f.settle();
        f.state.poll(&library);
    }
    assert_eq!(
        std::fs::read(&f.path).unwrap(),
        before,
        "loading wrote nothing"
    );
    assert!(!f.state.is_favourite(game), "not starred on a guess");
    let gone = &f.state.favourites().unwrap().gone;
    assert_eq!(gone.len(), 1);
    assert_eq!(gone[0].proposed, Some(game));
    let step = archivefs_core::CollectionReanchor {
        stored_archive_id: 990000,
        stored_created_at: "gone".into(),
        current_archive_id: game,
    };
    f.state.change(super::Change::Confirm(step));
    f.settle();
    assert!(f.state.is_favourite(game));
    assert!(f.state.favourites().unwrap().gone.is_empty());
    let saved = Database::open_read_only(&f.path)
        .unwrap()
        .load_game_collections()
        .unwrap();
    assert_eq!(saved[0].items[0].stored_archive_id, game);
    assert_eq!(saved[0].items[0].resolved_archive_id, Some(game));
}

#[test]
fn the_gone_list_shows_a_proposed_match_and_confirming_sends_only_that_item() {
    let context = eframe::egui::Context::default();
    let (mut app, sent) = recording_app(&context, &[]);
    app.collections.saved = favourites(vec![proposed_item(99, 2), item(98, None, "b")]);
    let library = app.library.clone();
    app.collections.library_changed(&library);
    while sent.try_recv().is_ok() {}
    app.collections.pending = 0;
    app.collections.scope = Some(1);
    app.collections.dialog = super::Dialog::Gone { collection: 1 };
    frame(&context, &mut app, vec![]);
    let strings = super::super::tests::text(&frame(&context, &mut app, vec![]));
    assert!(
        strings
            .iter()
            .any(|s| s.starts_with("Possible match: Beta.")),
        "{strings:?}"
    );
    assert_eq!(
        strings.iter().filter(|s| *s == "Keep as this game").count(),
        1,
        "only the item with a match offers it"
    );
    assert!(sent.try_recv().is_err(), "showing it sent nothing");
    click_text(&context, &mut app, "Keep as this game");
    match sent.try_recv().unwrap() {
        super::Job::Change(super::Change::Confirm(step)) => {
            assert_eq!((step.stored_archive_id, step.current_archive_id), (99, 2));
        }
        _ => panic!("expected a confirmed match"),
    }
    assert!(sent.try_recv().is_err());
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
    assert!(f.state.reanchoring.is_empty(), "cleared once answered");
}

/// `star_app` with a worker that only records what it is sent.
fn recording_app(
    context: &eframe::egui::Context,
    names: &[(i64, &str)],
) -> (super::super::App, std::sync::mpsc::Receiver<super::Job>) {
    let mut app = star_app(context);
    for (id, name) in names {
        app.collections.saved.push(GameCollection {
            id: *id,
            kind: GameCollectionKind::User,
            name: (*name).into(),
            items: Vec::new(),
        });
    }
    let library = app.library.clone();
    app.collections.library_changed(&library);
    let (jobs, sent) = std::sync::mpsc::channel();
    let (answers, results) = std::sync::mpsc::channel();
    app.collections.worker = Some((jobs, results));
    // Held open for the test: a dropped sender means the worker has died.
    std::mem::forget(answers);
    (app, sent)
}

fn click_text(context: &eframe::egui::Context, app: &mut super::super::App, label: &str) {
    use eframe::egui::{Event, PointerButton};
    frame(context, app, vec![]);
    let drawn = frame(context, app, vec![]);
    let target =
        super::super::tests::text_pos(&drawn, label).expect(label) + eframe::egui::vec2(4.0, 6.0);
    let click = |pressed| Event::PointerButton {
        pos: target,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(context, app, vec![Event::PointerMoved(target)]);
    frame(context, app, vec![click(true)]);
    frame(context, app, vec![click(false)]);
}

#[test]
fn rename_sends_the_name_shown_when_the_dialog_opened() {
    let context = eframe::egui::Context::default();
    let (mut app, sent) = recording_app(&context, &[(5, "Kids")]);
    app.collections.dialog = super::Dialog::Rename {
        collection: 5,
        seen: "Retro".into(),
        name: "New".into(),
    };
    click_text(&context, &mut app, "Rename");
    match sent.try_recv().unwrap() {
        super::Job::Change(super::Change::Rename {
            collection,
            seen,
            name,
        }) => assert_eq!(
            (collection, seen.as_str(), name.as_str()),
            (5, "Retro", "New")
        ),
        _ => panic!("expected a rename"),
    }
}

#[test]
fn delete_sends_the_name_the_dialog_shows() {
    let context = eframe::egui::Context::default();
    let (mut app, sent) = recording_app(&context, &[(5, "Alpha list"), (6, "Beta list")]);
    app.collections.scope = Some(6);
    app.collections.dialog = super::Dialog::Delete { collection: 5 };
    click_text(&context, &mut app, "Delete collection");
    match sent.try_recv().unwrap() {
        super::Job::Change(super::Change::Delete { collection, seen }) => {
            assert_eq!((collection, seen.as_str()), (5, "Alpha list"));
        }
        _ => panic!("expected a delete"),
    }
}

#[test]
fn a_second_delete_cannot_be_sent_while_the_first_is_unanswered() {
    let context = eframe::egui::Context::default();
    let (mut app, sent) = recording_app(&context, &[(5, "Alpha list")]);
    app.collections.dialog = super::Dialog::Delete { collection: 5 };
    click_text(&context, &mut app, "Delete collection");
    assert!(sent.try_recv().is_ok());
    assert_eq!(app.collections.pending, 1);
    app.collections.dialog = super::Dialog::Delete { collection: 5 };
    click_text(&context, &mut app, "Delete collection");
    assert!(sent.try_recv().is_err(), "the button is off until answered");
}

#[test]
fn try_again_reads_the_saved_collections_again() {
    let context = eframe::egui::Context::default();
    let (mut app, sent) = recording_app(&context, &[]);
    app.collections.saved.clear();
    app.collections.views.clear();
    app.collections.message = Some(super::READ_FAILED.into());
    click_text(&context, &mut app, "Try again");
    assert!(matches!(sent.try_recv().unwrap(), super::Job::Reload));
    assert_eq!(app.collections.message, None);
}

#[test]
fn browse_shows_a_failure_once_with_a_game_selected() {
    let context = eframe::egui::Context::default();
    let mut app = star_app(&context);
    app.router.current = super::super::routes::Route::BrowsePlayGame(1);
    app.collections.message = Some("That change was not saved: fixture".into());
    frame(&context, &mut app, vec![]);
    let strings = super::super::tests::text(&frame(&context, &mut app, vec![]));
    assert!(
        strings
            .iter()
            .any(|s| s == "Favourite" || s == "In Favourites")
    );
    let shown = strings.iter().filter(|s| s.contains("not saved")).count();
    assert_eq!(shown, 1, "{strings:?}");
}

#[test]
fn game_details_shows_a_failure_once() {
    let context = eframe::egui::Context::default();
    let mut app = star_app(&context);
    app.router.current = super::super::routes::Route::Game(1);
    app.collections.message = Some("That change was not saved: fixture".into());
    frame(&context, &mut app, vec![]);
    let strings = super::super::tests::text(&frame(&context, &mut app, vec![]));
    let shown = strings.iter().filter(|s| s.contains("not saved")).count();
    assert_eq!(shown, 1, "{strings:?}");
}

#[test]
fn opening_rename_keeps_the_name_on_screen_at_that_moment() {
    let context = eframe::egui::Context::default();
    let (mut app, _sent) = recording_app(&context, &[(5, "Kids")]);
    app.collections.scope = Some(5);
    click_text(&context, &mut app, "Rename");
    assert!(matches!(
        &app.collections.dialog,
        super::Dialog::Rename { seen, .. } if seen == "Kids"
    ));
}

#[test]
fn rename_and_delete_stay_off_until_the_last_change_is_answered() {
    let context = eframe::egui::Context::default();
    for label in ["Rename", "Delete"] {
        let (mut app, _sent) = recording_app(&context, &[(5, "Kids")]);
        app.collections.scope = Some(5);
        app.collections.pending = 1;
        click_text(&context, &mut app, label);
        assert!(
            matches!(app.collections.dialog, super::Dialog::None),
            "{label}"
        );
    }
    let (mut app, sent) = recording_app(&context, &[(5, "Kids")]);
    app.collections.pending = 1;
    app.collections.dialog = super::Dialog::Rename {
        collection: 5,
        seen: "Kids".into(),
        name: "New".into(),
    };
    click_text(&context, &mut app, "Rename");
    assert!(sent.try_recv().is_err());
}

#[test]
fn a_worker_that_dies_mid_job_does_not_leave_the_buttons_off() {
    let mut state = State::default();
    let (jobs, _requests) = std::sync::mpsc::channel();
    let (answers, results) = std::sync::mpsc::channel();
    state.worker = Some((jobs, results));
    state.pending = 1;
    drop(answers);
    state.poll(&Arc::new(library()));
    assert_eq!(state.pending, 0);
    assert_eq!(state.message.as_deref(), Some(super::UNAVAILABLE));
}

/// Search inside a collection matches the way the whole library does: every
/// word, in any order.
#[test]
fn a_search_of_several_words_matches_inside_a_collection_as_in_the_library() {
    let library = library();
    let view = view_of(&library, &[(1, "a"), (3, "b"), (4, "c")]);
    let search = Filter {
        search: "megadrive mario".into(),
        ..Filter::default()
    };
    let inside = titles(
        &library,
        &scoped_indices(&library, &view, &search, CollectionSort::TitleAscending),
    );
    assert_eq!(inside, vec![("Mario".to_string(), 4)]);
    let everywhere: Vec<i64> = library
        .filter(&search)
        .iter()
        .map(|index| library.games[*index].archive.id)
        .collect();
    assert_eq!(everywhere, vec![4]);
}

/// Inside a collection its own order applies; the Library sort (hidden there)
/// does not reorder it.
#[test]
fn the_library_sort_does_not_reorder_a_collection() {
    let library = library();
    let view = view_of(&library, &[(1, "2026-01"), (2, "2026-03"), (3, "2026-02")]);
    for sort in super::super::library::SortOrder::ALL {
        let filter = Filter {
            sort,
            ..Filter::default()
        };
        let ids: Vec<i64> = titles(
            &library,
            &scoped_indices(&library, &view, &filter, CollectionSort::TitleAscending),
        )
        .into_iter()
        .map(|(_, id)| id)
        .collect();
        assert_eq!(ids, vec![1, 3, 2], "{sort:?}");
    }
}

fn click_at(
    context: &eframe::egui::Context,
    app: &mut super::super::App,
    target: eframe::egui::Pos2,
) {
    use eframe::egui::{Event, PointerButton};
    let click = |pressed| Event::PointerButton {
        pos: target,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(context, app, vec![Event::PointerMoved(target)]);
    frame(context, app, vec![click(true)]);
    frame(context, app, vec![click(false)]);
}

/// Where each drawn text equal to `wanted` sits, top to bottom.
fn texts(output: &eframe::egui::FullOutput, wanted: &str) -> Vec<eframe::egui::Pos2> {
    fn walk(shape: &eframe::egui::Shape, wanted: &str, found: &mut Vec<eframe::egui::Pos2>) {
        match shape {
            eframe::egui::Shape::Text(text) if text.galley.text() == wanted => {
                found.push(text.pos + text.galley.rect.center().to_vec2());
            }
            eframe::egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, wanted, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    output
        .shapes
        .iter()
        .for_each(|clipped| walk(&clipped.shape, wanted, &mut found));
    found.sort_by(|a, b| (a.y, a.x).partial_cmp(&(b.y, b.x)).unwrap());
    found
}

/// Owner decision (2026-11-15): during Ctrl/Shift multi-selection every star
/// stays drawn and usable beside the selection tick, and starring a selected
/// game leaves the selection as it was.
#[test]
fn stars_stay_visible_and_usable_while_games_are_multi_selected() {
    let context = eframe::egui::Context::default();
    let mut app = star_app(&context);
    frame(&context, &mut app, vec![]);
    let drawn = frame(&context, &mut app, vec![]);
    let unselected = stars(&drawn);
    assert_eq!(unselected.len(), 3);
    let alpha = texts(&drawn, "Alpha")[0];
    app.library_selection.select_all(&[1, 2, 3]);
    frame(&context, &mut app, vec![]);
    let drawn = frame(&context, &mut app, vec![]);
    let selected = stars(&drawn);
    // The selection bar above the shelf moves every card down alike; on its
    // card each star stays where it was.
    let shift = texts(&drawn, "Alpha")[0] - alpha;
    let moved: Vec<_> = unselected.iter().map(|star| *star + shift).collect();
    assert_eq!(selected, moved, "selection must not move or hide a star");
    // Alpha (first in title order) is selected; its star still favourites it.
    click_at(&context, &mut app, selected[0]);
    assert!(app.collections.is_favourite(1));
    assert!(app.collections.is_favourite(2));
    assert_eq!(
        app.library_selection.len(),
        3,
        "starring kept the selection"
    );
    assert!((1..=3).all(|id| app.library_selection.contains(id)));
}

/// Owner decision (2026-11-15): "Select for artwork" shows only its own
/// checkbox, no star, and a click there never changes Favourites.
#[test]
fn selecting_for_artwork_hides_the_star_and_never_changes_favourites() {
    let context = eframe::egui::Context::default();
    let mut app = star_app(&context);
    frame(&context, &mut app, vec![]);
    let drawn = frame(&context, &mut app, vec![]);
    let where_stars_were = stars(&drawn);
    assert_eq!(where_stars_were.len(), 3);
    let alpha = texts(&drawn, "Alpha")[0];
    app.batch_selecting = true;
    frame(&context, &mut app, vec![]);
    let drawn = frame(&context, &mut app, vec![]);
    assert!(stars(&drawn).is_empty());
    let shift = texts(&drawn, "Alpha")[0] - alpha;
    let saved_before = app.collections.saved.clone();
    // Click exactly where Alpha's star was on its card: it selects for
    // artwork only.
    click_at(&context, &mut app, where_stars_were[0] + shift);
    assert!(app.batch_selection.contains(1), "the click selected Alpha");
    assert!(!app.collections.is_favourite(1));
    assert!(app.collections.is_favourite(2));
    assert_eq!(app.collections.saved, saved_before, "Favourites unchanged");
}

/// Review finding (2026-11-15): every card holds a focusable star. A
/// controller Up/Down from that star must step one grid row exactly as it does
/// from the card, even far down a scrolled shelf, rather than escape it.
#[test]
fn controller_up_and_down_from_a_cards_star_step_one_grid_row() {
    use super::super::big_picture::nav::{Control, Direction, PadEvent};
    use super::super::browse_play::{focused_card_control_id, focused_card_id};
    use eframe::egui::{Event, Key, Modifiers};

    let titles: Vec<String> = (1..=90).map(|i| format!("Game {i:03}")).collect();
    let context = eframe::egui::Context::default();
    let mut app = super::super::tests::review_app(
        &context,
        titles
            .iter()
            .enumerate()
            .map(|(i, t)| (i as i64 + 1, t.as_str(), "SNES"))
            .collect(),
    );
    app.simple.enabled = true;
    app.collections.saved = favourites(vec![]);
    let library = app.library.clone();
    app.collections.library_changed(&library);
    app.router.current = super::super::routes::Route::BrowsePlay;
    let frames = |app: &mut super::super::App, events: Vec<Event>| {
        frame(&context, app, events);
        frame(&context, app, vec![]);
        frame(&context, app, vec![]);
    };
    let tab = || Event::Key {
        key: Key::Tab,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    let focused = || context.memory(|m| m.focused());
    let card_game = || context.data(|d| d.get_temp::<i64>(focused_card_id()));
    let is_card = |id| context.read_response(id).expect("drawn").rect.height() > 50.0;
    let pad = |app: &mut super::super::App, direction| {
        let control = Control::Direction(direction);
        let shell = app.pad_shell.as_mut().expect("shell exists after a frame");
        shell.inject(PadEvent::Pressed { device: 1, control });
        shell.inject(PadEvent::Released { device: 1, control });
    };
    frames(&mut app, vec![]);
    // Tab to the first card (the star follows its card in Tab order).
    for _ in 0..80 {
        if focused().is_some_and(|id| card_game().is_some() && is_card(id)) {
            break;
        }
        frames(&mut app, vec![tab()]);
    }
    assert_eq!(card_game(), Some(1), "Tab reached the first card");
    app.pad_shell.as_mut().unwrap().set_pad_mode_for_test();
    pad(&mut app, Direction::Down);
    frames(&mut app, vec![]);
    let columns = card_game().expect("a card below") - 1;
    assert!(columns >= 1);
    for _ in 0..5 {
        pad(&mut app, Direction::Down); // scroll well down the shelf
        frames(&mut app, vec![]);
    }
    let from = card_game().expect("still on a card");
    assert_eq!(from, 1 + 6 * columns);
    for (direction, expected) in [(Direction::Down, from + columns), (Direction::Up, from)] {
        let start = card_game().unwrap();
        frames(&mut app, vec![tab()]);
        let star = focused().expect("the star has focus");
        assert!(!is_card(star), "Tab moved to the card's star");
        assert_eq!(
            context.data(|d| d.get_temp::<eframe::egui::Id>(focused_card_control_id())),
            Some(star),
            "a focused star counts as its card"
        );
        assert_eq!(card_game(), Some(start));
        pad(&mut app, direction);
        frames(&mut app, vec![]);
        assert_eq!(
            card_game(),
            Some(expected),
            "{direction:?} from the star stepped one row"
        );
        assert!(
            focused().is_some_and(is_card),
            "and landed on the card itself"
        );
    }
}

/// Owner decision 2026-11-15: while selecting for artwork no star and no
/// Collections editing is offered anywhere (list, selected-game panel, Game
/// Details); everything returns when artwork mode ends.
#[test]
fn artwork_mode_hides_every_favourites_and_collections_control_until_it_ends() {
    let context = eframe::egui::Context::default();
    let editing = [
        "Favourite",
        "In Favourites",
        "Collections",
        "Rename",
        "Delete",
        "1 no longer in the library",
    ];
    for route in [
        super::super::routes::Route::BrowsePlayGame(2),
        super::super::routes::Route::Game(2),
    ] {
        let (mut app, sent) = recording_app(&context, &[(5, "Kids")]);
        app.collections.saved[1].items = vec![item(77, None, "a")];
        let library = app.library.clone();
        app.collections.library_changed(&library);
        app.collections.pending = 0;
        app.collections.scope = Some(5);
        app.router.current = route.clone();
        let shown = |app: &mut super::super::App| {
            frame(&context, app, vec![]);
            let drawn = frame(&context, app, vec![]);
            (super::super::tests::text(&drawn), stars(&drawn).len())
        };
        let (normal, _) = shown(&mut app);
        let saved_before = app.collections.saved.clone();
        app.batch_selecting = true;
        app.collections.dialog = super::Dialog::Gone { collection: 5 };
        let (artwork, star_count) = shown(&mut app);
        for label in editing {
            assert!(
                !artwork.iter().any(|s| s == label),
                "{route:?}: {label} offered while selecting for artwork: {artwork:?}"
            );
        }
        assert_eq!(
            star_count, 0,
            "{route:?}: no star while selecting for artwork"
        );
        assert!(
            !artwork.iter().any(|s| s == "No longer in the library"),
            "{route:?}: dialogs wait until artwork mode ends"
        );
        assert_eq!(app.collections.saved, saved_before);
        app.batch_selecting = false;
        let (after, after_stars) = shown(&mut app);
        assert!(after_stars > 0, "{route:?}: stars return");
        assert!(
            after
                .iter()
                .any(|s| s == "Favourite" || s == "In Favourites"),
            "{route:?}: the panel star returns: {after:?}"
        );
        assert!(after.iter().any(|s| s == "Collections"), "{route:?}");
        assert!(
            after.iter().any(|s| s == "No longer in the library"),
            "{route:?}: the kept dialog returns"
        );
        let _ = normal;
        let _ = sent;
    }
}

/// Owner clarification 2026-11-15: a completed EmuWiz rename is saved when the
/// library loads (restart), so the favourite keeps its star once the old entry
/// is cleaned up; a mere name/size guess elsewhere is still not saved.
#[test]
fn a_completed_rename_is_saved_on_load_and_a_guess_is_not() {
    let mut f = fixture();
    let favourites = f.favourites_id();
    let library = f.library.clone();
    let alpha = f.ids[0];
    let beta = f.ids[1];
    f.state.toggle(favourites, alpha, &library);
    f.state.toggle(favourites, beta, &library);
    f.settle();
    let source = f.path.parent().unwrap().join("source");
    // Alpha: renamed by EmuWiz, recorded as a finished transaction.
    std::fs::rename(source.join("Alpha.zip"), source.join("Alpha (Renamed).zip")).unwrap();
    let transaction: archivefs_core::dat::rename_apply::model::RenameTransaction =
        serde_json::from_value(serde_json::json!({
            "transaction_id": "1-0", "plan_generation": 0, "created_at_unix": 1,
            "source_scan_root": source, "state": "applied",
            "entries": [{
                "source_path": source.join("Alpha.zip"),
                "destination_path": source.join("Alpha (Renamed).zip"),
                "original_basename": "Alpha.zip", "proposed_basename": "Alpha (Renamed).zip",
                "identity": {"size_bytes": 9, "modified_unix": 1, "kind": "regular_file"},
                "state": "applied"
            }]
        }))
        .unwrap();
    archivefs_core::dat::rename_apply::journal::write_journal(
        &f.path
            .parent()
            .unwrap()
            .join(archivefs_core::dat::rename_apply::RENAME_TRANSACTIONS_DIRECTORY),
        &transaction,
    )
    .unwrap();
    // Beta: moved by hand, no record.
    std::fs::create_dir_all(source.join("moved")).unwrap();
    std::fs::rename(
        source.join("Beta.zip"),
        source.join("moved").join("Beta.zip"),
    )
    .unwrap();
    let mut database = Database::open_existing(&f.path).unwrap();
    archivefs_core::scan_and_persist(
        &mut database,
        &archivefs_core::Config {
            source_folders: vec![source.clone()],
            mount_root: f.path.parent().unwrap().join("mount"),
            ratarmount_bin: "ratarmount".into(),
            master_rom_root: None,
            configured_source_roots: None,
        },
        "after rename",
    )
    .unwrap();
    let archives = database.load_archives().unwrap();
    database.close().unwrap();
    let current = |name: &str| {
        archives
            .iter()
            .find(|a| a.relative_path.ends_with(name) && a.last_verified_missing_at.is_none())
            .unwrap()
            .id
    };
    let (renamed, moved) = (current("Alpha (Renamed).zip"), current("moved/Beta.zip"));
    // Restart: a new state loads the rescanned library.
    f.library = Arc::new(Library::new(archives));
    let library = f.library.clone();
    f.state = State::with_database(Some(f.path.clone()), Default::default());
    f.state.library_changed(&library);
    f.settle();
    let saved = Database::open_read_only(&f.path)
        .unwrap()
        .load_game_collections()
        .unwrap();
    let mut stored: Vec<i64> = saved[0].items.iter().map(|i| i.stored_archive_id).collect();
    stored.sort();
    let mut expected = vec![renamed, beta];
    expected.sort();
    assert_eq!(stored, expected, "the rename was saved, the guess was not");
    assert!(f.state.is_favourite(renamed));
    // The hand-moved file: the library links it by name and size, which is a
    // guess, so it is only proposed and nothing was written for it.
    assert_eq!(library.resolve(beta), moved);
    assert!(!f.state.is_favourite(moved), "a guess is never starred");
    let gone = &f.state.favourites().unwrap().gone;
    assert_eq!(gone.len(), 1);
    assert_eq!(gone[0].proposed, Some(moved));
    assert!(stored.contains(&beta) && !stored.contains(&moved));
    // A second load changes nothing.
    let before = std::fs::read(&f.path).unwrap();
    f.state.library_changed(&library);
    f.settle();
    assert_eq!(std::fs::read(&f.path).unwrap(), before);
}

/// Owner follow-up 2026-11-15: when automatic updates are paused by an
/// untrusted rename record, Blue says so quietly; it says nothing otherwise,
/// and browsing and stars work as usual either way.
#[test]
fn the_paused_note_shows_only_while_paused_and_stars_still_work() {
    let context = eframe::egui::Context::default();
    let mut app = star_app(&context);
    let shown = |app: &mut super::super::App| {
        frame(&context, app, vec![]);
        let drawn = frame(&context, app, vec![]);
        super::super::tests::text(&drawn)
            .iter()
            .any(|s| s == super::JOURNAL_PAUSED)
    };
    assert!(!shown(&mut app), "no note without a problem");
    app.collections.journal_paused = true;
    assert!(shown(&mut app), "the note appears");
    // Manual favourites still work while it shows.
    frame(&context, &mut app, vec![]);
    let drawn = frame(&context, &mut app, vec![]);
    let alpha_star = stars(&drawn)[0];
    click_at(&context, &mut app, alpha_star);
    assert!(app.collections.is_favourite(1));
    app.collections.journal_paused = false;
    assert!(!shown(&mut app), "the note goes when the problem does");
}

#[test]
fn an_unreadable_rename_record_pauses_automatic_updates_until_it_is_gone() {
    let mut f = fixture();
    let favourites = f.favourites_id();
    let library = f.library.clone();
    let alpha = f.ids[0];
    f.state.toggle(favourites, alpha, &library);
    f.settle();
    assert!(!f.state.journal_paused);
    let source = f.path.parent().unwrap().join("source");
    let journals = f
        .path
        .parent()
        .unwrap()
        .join(archivefs_core::dat::rename_apply::RENAME_TRANSACTIONS_DIRECTORY);
    // A finished rename that would otherwise be saved ...
    std::fs::rename(source.join("Alpha.zip"), source.join("Alpha (Renamed).zip")).unwrap();
    let transaction: archivefs_core::dat::rename_apply::model::RenameTransaction =
        serde_json::from_value(serde_json::json!({
            "transaction_id": "1-0", "plan_generation": 0, "created_at_unix": 1,
            "source_scan_root": source, "state": "applied",
            "entries": [{
                "source_path": source.join("Alpha.zip"),
                "destination_path": source.join("Alpha (Renamed).zip"),
                "original_basename": "Alpha.zip", "proposed_basename": "Alpha (Renamed).zip",
                "identity": {"size_bytes": 9, "modified_unix": 1, "kind": "regular_file"},
                "state": "applied"
            }]
        }))
        .unwrap();
    archivefs_core::dat::rename_apply::journal::write_journal(&journals, &transaction).unwrap();
    let mut database = Database::open_existing(&f.path).unwrap();
    archivefs_core::scan_and_persist(
        &mut database,
        &archivefs_core::Config {
            source_folders: vec![source.clone()],
            mount_root: f.path.parent().unwrap().join("mount"),
            ratarmount_bin: "ratarmount".into(),
            master_rom_root: None,
            configured_source_roots: None,
        },
        "after rename",
    )
    .unwrap();
    let archives = database.load_archives().unwrap();
    database.close().unwrap();
    // ... beside a record that cannot be read.
    let broken = journals.join("2-0.json");
    std::fs::write(&broken, b"{ not a journal").unwrap();
    f.library = Arc::new(Library::new(archives));
    let library = f.library.clone();
    let before = std::fs::read(&f.path).unwrap();
    f.state.library_changed(&library);
    f.settle();
    assert!(f.state.journal_paused, "the pause is shown");
    assert_eq!(
        std::fs::read(&f.path).unwrap(),
        before,
        "fail closed: nothing moved"
    );
    let saved = Database::open_read_only(&f.path)
        .unwrap()
        .load_game_collections()
        .unwrap();
    assert_eq!(
        saved[0].items[0].stored_archive_id, alpha,
        "the favourite is kept as it was"
    );
    // The problem goes: the note goes, and the proven rename is saved.
    std::fs::remove_file(&broken).unwrap();
    f.state.library_changed(&library);
    f.settle();
    assert!(!f.state.journal_paused);
    let saved = Database::open_read_only(&f.path)
        .unwrap()
        .load_game_collections()
        .unwrap();
    assert_ne!(
        saved[0].items[0].stored_archive_id, alpha,
        "now saved to the new entry"
    );
}
