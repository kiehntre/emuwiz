use super::*;

#[test]
fn large_browse_search_defers_work_and_hides_previous_results() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    app.library = Arc::new(Library::new(
        (1..=10_001)
            .map(|id| archive(id, &format!("Game {id:05}"), Some("SNES")))
            .collect(),
    ));
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    app.router.current = Route::BrowsePlay;
    app.filter.search = "Game 10001".into();
    let strings = text(&frame(&ctx, &mut app, [1280.0, 820.0]));
    assert!(
        app.filter_dirty.is_some() || app.filter_inflight,
        "large search must use existing asynchronous filtering, not scan the catalogue during paint"
    );
    assert!(
        strings.iter().any(|s| s.contains("Updating results")),
        "pending differs from no matches"
    );
    assert!(
        !strings.iter().any(|s| s == "Game 00001"),
        "old rows must not be painted under a new filter"
    );
}

#[test]
fn cached_platform_filters_and_title_hints_match_the_full_scan() {
    let mut rows = vec![
        archive(1, "Same", Some("SNES")),
        archive(2, "Same", Some("Amiga")),
        archive(3, "same", Some("SNES")),
        archive(4, "Unique", None),
    ];
    rows[2].last_verified_missing_at = Some("fixture missing".into());
    let library = Library::new(rows);
    for platform in ["", "SNES", "Amiga", "Unknown system", "absent"] {
        for search in ["", "same", " UNIQUE ", "no match"] {
            for attention in [false, true] {
                for unverified in [false, true] {
                    let filter = Filter {
                        platform: platform.into(),
                        search: search.into(),
                        attention_only: attention,
                        unverified_only: unverified,
                        ..Default::default()
                    };
                    let needle = search.trim().to_lowercase();
                    let expected: Vec<_> = library
                        .games
                        .iter()
                        .enumerate()
                        .filter(|(_, g)| {
                            (platform.is_empty() || g.platform == platform)
                                && (!attention || g.attention)
                                && (!unverified || !g.identified)
                                && (needle.is_empty()
                                    || format!("{} {}", g.title, g.platform)
                                        .to_lowercase()
                                        .contains(&needle))
                        })
                        .map(|(i, _)| i)
                        .collect();
                    assert_eq!(library.filter(&filter), expected);
                }
            }
        }
    }
    for game in &library.games {
        assert_eq!(
            library.has_title_twin(game.archive.id),
            library
                .games
                .iter()
                .any(|other| other.archive.id != game.archive.id && other.title == game.title)
        );
    }
    let replacement = Library::new(vec![archive(1, "Same", Some("Amiga"))]);
    assert!(!replacement.has_title_twin(1));
    assert!(!replacement.by_platform.contains_key("SNES"));
}

#[test]
fn large_browse_filter_settles_and_refreshes_on_a_same_size_snapshot() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let rows = || {
        (1..=10_001)
            .map(|id| archive(id, &format!("Game {id:05}"), Some("SNES")))
            .collect::<Vec<_>>()
    };
    app.library = Arc::new(Library::new(rows()));
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    app.router.current = Route::BrowsePlay;
    // Prime one request, then change the query before the worker can settle it.
    app.filter.search = "Game 00001".into();
    let _ = frame(&ctx, &mut app, [1280.0, 820.0]);
    app.filter_dirty = Some(Instant::now() - Duration::from_secs(1));
    app.poll(&ctx);
    app.filter.search = "Game 10001".into();
    let _ = frame(&ctx, &mut app, [1280.0, 820.0]);
    let wait = |app: &mut App| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.filter_dirty.is_some() || app.filter_inflight {
            app.filter_dirty = app
                .filter_dirty
                .map(|_| Instant::now() - Duration::from_secs(1));
            app.poll(&ctx);
            let _ = frame(&ctx, app, [1280.0, 820.0]);
            assert!(Instant::now() < deadline, "filter worker did not settle");
            std::thread::sleep(Duration::from_millis(2));
        }
    };
    wait(&mut app);
    let strings = text(&frame(&ctx, &mut app, [1280.0, 820.0]));
    assert_eq!(app.indices, vec![*app.library.by_id.get(&10001).unwrap()]);
    assert!(strings.iter().any(|s| s == "Game 10001"));
    assert!(!strings.iter().any(|s| s == "Game 00001"));
    let mut replacement = rows();
    replacement[10000].display_name = "Renamed after reload".into();
    app.library = Arc::new(Library::new(replacement));
    let strings = text(&frame(&ctx, &mut app, [1280.0, 820.0]));
    assert!(strings.iter().any(|s| s.contains("Updating results")));
    wait(&mut app);
    let strings = text(&frame(&ctx, &mut app, [1280.0, 820.0]));
    // The search field still echoes "Game 10001"; inspect result indices and
    // the rendered empty-result state rather than mistaking the input for a row.
    assert!(
        app.indices.is_empty(),
        "old snapshot cannot survive a same-size reload"
    );
    assert!(strings.iter().any(|s| s == "No games match"));
}
