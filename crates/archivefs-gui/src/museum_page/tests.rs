use super::*;

fn snapshot(platforms: Vec<home_page::HomePlatformSummary>) -> home_page::HomeLibrarySnapshot {
    home_page::HomeLibrarySnapshot {
        total: platforms.iter().map(|p| p.total).sum(),
        present: 0,
        identified: platforms.iter().map(|p| p.identified).sum(),
        unresolved: 0,
        missing: platforms.iter().map(|p| p.missing).sum(),
        duplicate_groups: 0,
        platforms,
        romm_media_coverage: None,
    }
}

fn platform(
    name: &str,
    total: usize,
    identified: usize,
    missing: usize,
) -> home_page::HomePlatformSummary {
    home_page::HomePlatformSummary {
        name: name.to_string(),
        total,
        identified,
        missing,
        romm_media_coverage: None,
    }
}

#[test]
fn build_platform_views_projects_the_existing_snapshot_without_recomputing_anything() {
    let library = snapshot(vec![
        platform("SNES", 10, 9, 1),
        platform("Nintendo 64", 5, 5, 0),
    ]);
    let views = build_platform_views(&library);
    assert_eq!(views.len(), 2);
    assert_eq!(views[0].name, "SNES");
    assert_eq!(views[0].games_total, 10);
    assert_eq!(views[0].games_identified, 9);
    assert_eq!(views[0].games_missing, 1);
    assert_eq!(views[1].name, "Nintendo 64");
}

#[test]
fn a_platform_recognised_by_the_registry_carries_registry_facts() {
    // "Super Nintendo Entertainment System" is registered in
    // `archivefs_core::platform::PLATFORMS` under that exact display name
    // (its `id` is the short "SNES", but the registry lookup joins on
    // `display_name`, matching how `HomePlatformSummary::name` is actually
    // populated in production - see `home_library_snapshot` in `main.rs`).
    let library = snapshot(vec![platform(
        "Super Nintendo Entertainment System",
        1,
        1,
        0,
    )]);
    let views = build_platform_views(&library);
    let registry = views[0]
        .registry
        .as_ref()
        .expect("Super Nintendo Entertainment System is a real registered platform");
    assert_eq!(registry.display_name, "Super Nintendo Entertainment System");
    assert!(!registry.explanation.is_empty());
}

#[test]
fn platform_detail_projects_romm_media_coverage_without_recomputing() {
    let mut library = snapshot(vec![platform("SNES", 10, 9, 1)]);
    library.platforms[0].romm_media_coverage =
        Some(archivefs_core::identity_source::model::MediaCoverage {
            total_games: 10,
            covers_available: 8,
            screenshots_available: 6,
            videos_available: None,
        });
    let view = build_platform_views(&library);
    assert_eq!(view[0].media_coverage.unwrap().covers_available, 8);
}

#[test]
fn an_unrecognised_platform_string_honestly_reports_no_registry_facts() {
    let library = snapshot(vec![platform("Totally Made Up Platform", 1, 1, 0)]);
    let views = build_platform_views(&library);
    assert!(
        views[0].registry.is_none(),
        "must never invent registry facts for a name the registry doesn't have"
    );
}

fn run(
    state: &mut MuseumPageState,
    library: Option<&home_page::HomeLibrarySnapshot>,
) -> (egui::FullOutput, Option<MuseumAction>) {
    let context = egui::Context::default();
    let mut action = None;
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 900.0),
            )),
            ..Default::default()
        },
        |context| {
            egui::CentralPanel::default().show(context, |ui| {
                action = show(ui, state, library);
            });
        },
    );
    (output, action)
}

fn output_contains(output: &egui::FullOutput, needle: &str) -> bool {
    fn walk(shape: &egui::Shape, needle: &str) -> bool {
        match shape {
            egui::Shape::Text(text) => text.galley.text().contains(needle),
            egui::Shape::Vec(nested) => nested.iter().any(|s| walk(s, needle)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|clipped| walk(&clipped.shape, needle))
}

#[test]
fn no_library_shows_an_explained_empty_state_not_a_blank_page() {
    let mut state = MuseumPageState::default();
    let (output, action) = run(&mut state, None);
    assert!(output_contains(&output, "Museum"));
    assert!(output_contains(&output, "No library loaded yet"));
    assert!(action.is_none());
}

#[test]
fn no_identified_platforms_shows_an_explained_empty_state() {
    let mut state = MuseumPageState::default();
    let library = snapshot(vec![]);
    let (output, _) = run(&mut state, Some(&library));
    assert!(output_contains(&output, "No platforms identified yet"));
}

#[test]
fn approved_museum_hero_uses_the_source_aspect_ratio_and_cached_texture() {
    let decoded = image::load_from_memory(MUSEUM_HERO_PNG).expect("approved Museum hero asset");
    let width = 1_024.0_f32;
    let height = museum_hero_height(width);

    assert!((width / height - decoded.width() as f32 / decoded.height() as f32).abs() < 0.01);
    assert_eq!((decoded.width(), decoded.height()), (1916, 821));

    let context = egui::Context::default();
    let mut state = MuseumHeroState::default();
    let _ = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            assert!(cached_museum_hero(ui, &mut state));
            assert!(state.poster_load_attempted);
            assert!(state.poster_texture.is_some());
        });
    });
}

#[test]
fn poster_mode_owns_the_museum_header_and_keeps_empty_state_functional() {
    let context = egui::Context::default();
    let mut hero = MuseumHeroState::default();
    let mut state = MuseumPageState::default();
    let mut requests = Vec::new();
    let output = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            let _ = show_with_selected_game_and_artwork_with_hero(
                ui,
                &mut hero,
                &mut state,
                None,
                None,
                None,
                None,
                None,
                &mut requests,
            );
        });
    });

    assert!(hero.poster_texture.is_some());
    assert!(output_contains(&output, "No library loaded yet"));
    assert!(!output_contains(
        &output,
        "Browse your collection by platform"
    ));
}

#[test]
fn the_grid_lists_every_platform_with_its_real_count() {
    let mut state = MuseumPageState::default();
    let library = snapshot(vec![platform("SNES", 42, 40, 2)]);
    let (output, _) = run(&mut state, Some(&library));
    assert!(output_contains(&output, "SNES"));
    assert!(output_contains(&output, "42 games"));
    assert!(output_contains(&output, "2 missing"));
}

#[test]
fn selecting_a_platform_switches_to_its_detail_view() {
    let mut state = MuseumPageState::default();
    state.selected_platform = Some("SNES".to_string());
    let library = snapshot(vec![platform("SNES", 42, 40, 2)]);
    let (output, _) = run(&mut state, Some(&library));
    assert!(output_contains(&output, "Games: 42"));
    assert!(output_contains(&output, "Identified: 40"));
    assert!(output_contains(&output, "< All platforms"));
}

#[test]
fn a_selected_platform_no_longer_in_the_snapshot_falls_back_to_the_grid_without_panicking() {
    let mut state = MuseumPageState::default();
    state.selected_platform = Some("Ghost Platform".to_string());
    let library = snapshot(vec![platform("SNES", 1, 1, 0)]);
    let (output, _) = run(&mut state, Some(&library));
    assert_eq!(state.selected_platform, None);
    assert!(output_contains(&output, "SNES"));
}

#[test]
fn platform_with_no_preferred_emulator_does_not_offer_an_emulator_setup_action() {
    let mut state = MuseumPageState::default();
    state.selected_platform = Some("Totally Made Up Platform".to_string());
    let library = snapshot(vec![platform("Totally Made Up Platform", 1, 1, 0)]);
    let (output, _) = run(&mut state, Some(&library));
    assert!(!output_contains(&output, "Emulator Setup"));
}

#[test]
fn museum_grid_layout_stays_within_the_available_width() {
    // At MUSEUM_CARD_MIN_WIDTH=220 + MUSEUM_CARD_GAP=14, 500px genuinely
    // fits two columns (2*220+14=454<=500) - 300px is the width that
    // actually forces a single column.
    let narrow = museum_grid_layout(300.0, 8);
    assert_eq!(narrow.columns, 1);
    let wide = museum_grid_layout(1400.0, 8);
    assert!(wide.columns <= MUSEUM_CARD_MAX_COLUMNS);
    assert!(wide.card_width <= MUSEUM_CARD_MAX_WIDTH);
    let single = museum_grid_layout(1400.0, 1);
    assert_eq!(single.columns, 1);
}

fn selected_game(
    screenshot_count: Option<usize>,
    video_available: Option<bool>,
    dat_verified: bool,
) -> MuseumSelectedGameView {
    MuseumSelectedGameView {
        archive_path: "/roms/example.zip".into(),
        title: "Example Game".to_string(),
        platform: "SNES".to_string(),
        facts: vec![("Media format".to_string(), "ZIP".to_string())],
        evidence_highlights: if dat_verified {
            vec!["DAT identity verified".to_string()]
        } else {
            Vec::new()
        },
        feature_view: None,
        screenshot_count,
        video_available,
        dat_verified,
    }
}

fn run_selected(game: MuseumSelectedGameView) -> egui::FullOutput {
    let context = egui::Context::default();
    let library = snapshot(vec![platform("SNES", 1, 1, 0)]);
    let mut state = MuseumPageState {
        selected_platform: Some("SNES".to_string()),
    };
    let mut screenshots = crate::gamer_artwork::GamerScreenshotCache::default();
    let mut screenshot_requests = Vec::new();
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 1000.0),
            )),
            ..Default::default()
        },
        |context| {
            egui::CentralPanel::default().show(context, |ui| {
                let _ = show_with_selected_game(
                    ui,
                    &mut state,
                    Some(&library),
                    Some(&game),
                    Some(&crate::gamer_artwork::GamerCoverCache::default()),
                    Some(&mut screenshots),
                    &mut screenshot_requests,
                );
            });
        },
    )
}

#[test]
fn selected_game_showcase_keeps_cover_and_video_states_honest() {
    let output = run_selected(selected_game(None, None, false));
    assert!(output_contains(&output, "No cover art available"));
    assert!(output_contains(&output, "Loading screenshots…"));
    assert!(output_contains(&output, "Video coverage not indexed"));
    assert!(output_contains(&output, "Collection facts"));
}

#[test]
fn selected_game_showcase_renders_empty_screenshots_and_verified_evidence() {
    let output = run_selected(selected_game(Some(0), Some(false), true));
    assert!(output_contains(&output, "No screenshots available"));
    assert!(output_contains(&output, "DAT verified"));
    assert!(output_contains(&output, "DAT identity verified"));
}

#[test]
fn selected_game_showcase_distinguishes_known_video_absence() {
    let output = run_selected(selected_game(Some(0), Some(false), false));
    assert!(output_contains(&output, "Video not available"));
    assert!(!output_contains(&output, "Video coverage not indexed"));
}

// --- P1 MEDIA RELIABILITY PASS: WHY DO SCREENSHOTS WORK FOR SOME GAMES BUT
// NOT OTHERS? V2 -----------------------------------------------------------
//
// Root cause: `show_selected_game_showcase` only ever *read*
// `GamerScreenshotCache` (an `Option<&GamerScreenshotCache>`). The only
// place that ever called `GamerScreenshotCache::visible` - the scheduling
// primitive that actually asks the worker for a screenshot - was Gamer
// View's own "Details" sub-screen (`show_gamer_details_panel` in
// `gamer_view.rs`). A game whose provider data had real screenshots but
// whose Details screen was never opened this session showed nothing in
// Museum, indistinguishable from a game with genuinely zero provider
// screenshots - not because the data was missing, but because nothing had
// ever asked for it on this surface.

fn run_selected_with_cache(
    game: &MuseumSelectedGameView,
    cache: &mut crate::gamer_artwork::GamerScreenshotCache,
) -> (egui::FullOutput, Vec<crate::gamer_artwork::CoverJob>) {
    let context = egui::Context::default();
    let library = snapshot(vec![platform("SNES", 1, 1, 0)]);
    let mut state = MuseumPageState {
        selected_platform: Some("SNES".to_string()),
    };
    let mut screenshot_requests = Vec::new();
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 1000.0),
            )),
            ..Default::default()
        },
        |context| {
            egui::CentralPanel::default().show(context, |ui| {
                let _ = show_with_selected_game(
                    ui,
                    &mut state,
                    Some(&library),
                    Some(game),
                    Some(&crate::gamer_artwork::GamerCoverCache::default()),
                    Some(cache),
                    &mut screenshot_requests,
                );
            });
        },
    );
    (output, screenshot_requests)
}

/// 1/2. Museum now schedules a real request for the selected game's
/// screenshots - the exact fix. Before this pass, `screenshot_requests`
/// would always be empty here because Museum held no `&mut` access to ask
/// for anything.
#[test]
fn museum_schedules_a_screenshot_request_for_the_selected_game() {
    let game = selected_game(None, None, false);
    let mut cache = crate::gamer_artwork::GamerScreenshotCache::default();
    let (_, requests) = run_selected_with_cache(&game, &mut cache);
    assert_eq!(
        requests.len(),
        1,
        "exactly one request: index 0, count unknown"
    );
    let job = &requests[0];
    assert_eq!(
        job.local_path, game.archive_path,
        "must target the exact selected path"
    );
    assert_eq!(
        job.kind,
        crate::gamer_artwork::GamerArtworkKind::Screenshot(0)
    );
}

/// A screenshot count already learned by the cache (from a prior answer -
/// `game.screenshot_count` alone is not enough; that view field and the
/// cache's own count map are the same value in production only because
/// `main.rs` derives one from the other) schedules every index up to it, not
/// just index 0 - the same bounded batch Gamer View's Details screen would
/// request.
#[test]
fn museum_schedules_every_known_index_once_count_is_discovered() {
    let game = selected_game(Some(3), None, false);
    let mut cache = crate::gamer_artwork::GamerScreenshotCache::default();
    // Simulate the cache having already learned the real count from a prior
    // answer for index 0, exactly as `GamerScreenshotCache::absorb` would.
    let _ = cache.visible(&game.archive_path);
    cache.absorb(
        &egui::Context::default(),
        crate::gamer_artwork::CoverReply {
            // A freshly `default()`-constructed cache starts at generation 0
            // and nothing here bumps it.
            generation: 0,
            local_path: game.archive_path.clone(),
            provider_game_id: Some("game-1".to_string()),
            kind: crate::gamer_artwork::GamerArtworkKind::Screenshot(0),
            screenshot_count: Some(3),
            answer: crate::gamer_artwork::CoverAnswer::None(
                crate::gamer_artwork::NoCover::NoArtwork,
            ),
        },
    );
    let (_, requests) = run_selected_with_cache(&game, &mut cache);
    assert_eq!(
        requests.len(),
        2,
        "indices 1 and 2 - index 0 was already answered"
    );
    assert_eq!(
        requests[0].kind,
        crate::gamer_artwork::GamerArtworkKind::Screenshot(1)
    );
    assert_eq!(
        requests[1].kind,
        crate::gamer_artwork::GamerArtworkKind::Screenshot(2)
    );
}

/// 5. Already-ready screenshots are never re-requested - `visible` is a
/// no-op wherever a slot already exists, so a game whose screenshots were
/// already fetched (by Museum itself, or previously by Gamer View's Details
/// screen for the same path) stays free to redraw.
#[test]
fn museum_does_not_reschedule_an_already_answered_screenshot() {
    let game = selected_game(Some(1), None, false);
    let mut cache = crate::gamer_artwork::GamerScreenshotCache::default();
    // Prime the cache exactly as a prior successful request would have:
    // one visible() call, then the count becomes known.
    let _ = cache.visible(&game.archive_path);
    let (_, first_pass) = run_selected_with_cache(&game, &mut cache);
    assert_eq!(
        first_pass.len(),
        0,
        "the primed index 0 must not be re-requested"
    );
}
