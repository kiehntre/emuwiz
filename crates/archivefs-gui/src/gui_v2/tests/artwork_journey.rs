//! Synthetic provider-to-Blue regressions for Job 36. No network or real profile.
use super::*;
use archivefs_core::metadata_aggregation::{self as aggregation, *};

fn evidence() -> ResolvedMetadata {
    let candidate = |value: &str, provider| MetadataCandidate {
        field: MetadataField::Developer,
        value: value.into(),
        provenance: Provenance {
            provider,
            source_class: SourceClass::ProviderCache,
            retrieved_at_unix_seconds: 0,
            detail: "synthetic saved source".into(),
            cache_path: None,
        },
    };
    aggregation::resolve(AggregationInput {
        metadata: vec![
            candidate("ES-DE Studio", Provider::EsDe),
            candidate("Alternative Studio", Provider::Romm),
        ],
        ..Default::default()
    })
}

#[test]
fn job36_blue_information_uses_resolved_values_and_explains_alternatives() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.simple.enabled = true;
    app.library = Arc::new(Library::new(vec![archive(
        36,
        "Synthetic artwork",
        Some("NES"),
    )]));
    let mut index = MediaIndex::default();
    index.resolved.insert(36, evidence());
    app.artwork.index = Some(Arc::new(index));
    app.router.current = Route::Game(36);
    let size = [1024.0, 600.0];
    scroll_to_text(&context, &mut app, size, "Game information");
    click_label(&context, &mut app, size, "Game information");
    let output = scroll_to_text(&context, &mut app, size, "ES-DE Studio");
    assert!(visible_text(&output, "ES-DE Studio"));
    scroll_to_text(&context, &mut app, size, "Metadata sources & alternatives");
    click_label(&context, &mut app, size, "Metadata sources & alternatives");
    let output = scroll_to_text(&context, &mut app, size, "Alternative Studio");
    assert!(visible_text(&output, "RomM"));
}

#[test]
fn job36_all_screenshots_from_selected_local_source_are_scheduled() {
    // A separate test process owns all discovery settings. Other tests keep
    // their own private HOME, even when the full suite runs concurrently.
    const CHILD: &str = "EMUWIZ_JOB36_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "gui_v2::tests::artwork_journey::job36_all_screenshots_from_selected_local_source_are_scheduled", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let home = archivefs_core::test_environment::private_home_for_fixtures();
    let root = home.join("local-provider");
    fs::create_dir_all(root.join("Data/Platforms")).unwrap();
    let shots = root.join("Images/Nintendo 64/Screenshot - Gameplay");
    fs::create_dir_all(&shots).unwrap();
    for n in 1..=3 {
        image::RgbaImage::from_pixel(16, 12, image::Rgba([n * 50, 20, 30, 255]))
            .save(shots.join(format!("Example-0{n}.png")))
            .unwrap();
    }
    let game_path = home.join("Example.z64");
    fs::write(&game_path, b"synthetic game sentinel").unwrap();
    fs::write(root.join("Data/Platforms/Nintendo 64.xml"), format!("<LaunchBox><Game><ID>lb36</ID><Platform>Nintendo 64</Platform><Title>Example</Title><ApplicationPath>{}</ApplicationPath></Game></LaunchBox>", game_path.display())).unwrap();
    let settings = archivefs_core::app_dirs::config_path("local-artwork-folders.json").unwrap();
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    fs::write(
        &settings,
        serde_json::to_vec(
            &serde_json::json!({"folders":[{"path":root,"layout":"launch_box","enabled":true}]}),
        )
        .unwrap(),
    )
    .unwrap();
    let mut row = archive(36, "Example", Some("N64"));
    row.absolute_path = game_path.clone();
    let library = Library::new(vec![row]);
    let index = MediaIndex::discover(&library);
    assert_eq!(index.screenshots.get(&36).map(Vec::len), Some(3));
    assert!(
        index
            .diagnostic_for(library.game(36).unwrap())
            .unwrap()
            .contains("Local screenshot candidates: 3")
    );
    let names: Vec<_> = index.screenshots[&36]
        .iter()
        .map(|source| match source {
            Source::Local(path) => path.file_name().unwrap().to_string_lossy().into_owned(),
            Source::Remote { .. } => panic!("local gallery became remote"),
        })
        .collect();
    assert_eq!(
        names,
        ["Example-01.png", "Example-02.png", "Example-03.png"]
    );
    let original_settings = fs::read(&settings).unwrap();
    let mut disabled: serde_json::Value = serde_json::from_slice(&original_settings).unwrap();
    disabled["folders"][0]["enabled"] = false.into();
    fs::write(&settings, serde_json::to_vec(&disabled).unwrap()).unwrap();
    let disabled_index = MediaIndex::discover(&library);
    assert!(!disabled_index.screenshots.contains_key(&36));
    fs::write(&settings, original_settings).unwrap();
    assert_eq!(MediaIndex::discover(&library).screenshots[&36].len(), 3);
    assert_eq!(fs::read(game_path).unwrap(), b"synthetic game sentinel");
}

#[test]
fn job36_thumbnail_survives_process_restart_and_source_replacement() {
    const ROOT: &str = "EMUWIZ_JOB36_CACHE_FIXTURE";
    if let Some(root) = std::env::var_os(ROOT) {
        let root = std::path::PathBuf::from(root);
        let pixels = thumbnail::load_local(&root.join("source.png"), &root.join("cache")).unwrap();
        fs::write(
            root.join("result.json"),
            serde_json::to_vec(&serde_json::json!({
                "hit": pixels.timings.cache_hit,
                "color": pixels.image.pixels[0].to_array(),
            }))
            .unwrap(),
        )
        .unwrap();
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let source = root.join("source.png");
    let save = |color| {
        image::RgbaImage::from_pixel(24, 32, image::Rgba(color))
            .save(&source)
            .unwrap()
    };
    save([200, 30, 40, 255]);
    let original = fs::read(&source).unwrap();
    let run = || {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "gui_v2::tests::artwork_journey::job36_thumbnail_survives_process_restart_and_source_replacement"])
            .env(ROOT, root).output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&fs::read(root.join("result.json")).unwrap())
            .unwrap()
    };
    assert_eq!(run()["hit"], false);
    let second = run();
    assert_eq!(second["hit"], true);
    assert_eq!(second["color"], serde_json::json!([200, 30, 40, 255]));
    assert_eq!(fs::read(&source).unwrap(), original);
    save([20, 210, 40, 255]);
    let changed = run();
    assert_eq!(changed["hit"], false);
    assert_eq!(changed["color"], serde_json::json!([20, 210, 40, 255]));
    assert_eq!(run()["hit"], true);
    fs::remove_file(source).unwrap();
    assert!(thumbnail::load_local(&root.join("source.png"), &root.join("cache")).is_err());
}

#[test]
fn job36_source_controls_cannot_save_a_change_that_misses_an_active_refresh() {
    const CHILD: &str = "EMUWIZ_JOB36_SOURCE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "gui_v2::tests::artwork_journey::job36_source_controls_cannot_save_a_change_that_misses_an_active_refresh", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let home = archivefs_core::test_environment::private_home_for_fixtures();
    let source = home.join("esde-source");
    fs::create_dir_all(source.join("gamelists/n64")).unwrap();
    fs::create_dir_all(source.join("downloaded_media")).unwrap();
    let settings = archivefs_core::app_dirs::config_path("local-artwork-folders.json").unwrap();
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    let original = serde_json::to_vec(
        &serde_json::json!({"folders":[{"path":source,"layout":"es_de","enabled":true}]}),
    )
    .unwrap();
    fs::write(&settings, &original).unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.simple.enabled = true;
    app.artwork.index_loading = true;
    app.router.current = Route::Section(Section::Artwork);
    let size = [1024.0, 600.0];
    scroll_to_text(&context, &mut app, size, "Enabled");
    click_label(&context, &mut app, size, "Enabled");
    assert_eq!(
        fs::read(settings).unwrap(),
        original,
        "busy controls must not persist an unrefreshed source change"
    );
}

#[test]
fn job36_numbered_image_never_overrides_an_exact_sibling_title() {
    use archivefs_core::identity_source::launchbox_local::discover_launchbox_local;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join("Data/Platforms")).unwrap();
    let shots = root.join("Images/Nintendo 64/Screenshot - Gameplay");
    fs::create_dir_all(&shots).unwrap();
    let image = shots.join("Example-02.png");
    image::RgbaImage::from_pixel(8, 8, image::Rgba([40, 80, 120, 255]))
        .save(&image)
        .unwrap();
    fs::write(root.join("Data/Platforms/Nintendo 64.xml"), br#"<LaunchBox>
        <Game><ID>a</ID><Platform>Nintendo 64</Platform><Title>Example</Title><ApplicationPath>/games/a.z64</ApplicationPath></Game>
        <Game><ID>b</ID><Platform>Nintendo 64</Platform><Title>Example-02</Title><ApplicationPath>/games/b.z64</ApplicationPath></Game>
        </LaunchBox>"#).unwrap();
    let index = discover_launchbox_local(root, 1).unwrap();
    let first = index
        .lookup(None, Some(Path::new("/games/a.z64")), Some("N64"), None)
        .unwrap();
    let second = index
        .lookup(None, Some(Path::new("/games/b.z64")), Some("N64"), None)
        .unwrap();
    assert!(!first.game.media.iter().any(|media| media.path == image));
    assert!(second.game.media.iter().any(|media| media.path == image));
}

#[test]
fn job36_complete_gallery_keeps_the_details_side_column_inside_the_viewport() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.simple.enabled = true;
    context.style_mut(|style| style.debug.warn_if_rect_changes_id = false);
    app.library = Arc::new(Library::new(vec![archive(
        36,
        "Synthetic gallery",
        Some("N64"),
    )]));
    app.artwork = Artwork::with_indexer(
        context.clone(),
        Some(directory.path().join("cache")),
        Arc::new(|_| MediaIndex::default()),
    );
    let mut index = MediaIndex::default();
    let mut gallery = Vec::new();
    for ordinal in 0..3 {
        let path = directory.path().join(format!("shot{ordinal}.png"));
        image::RgbaImage::from_pixel(24, 18, image::Rgba([30, 60, 90, 255]))
            .save(&path)
            .unwrap();
        gallery.push(Source::Local(path));
    }
    index.screenshots.insert(36, gallery);
    app.artwork.index = Some(Arc::new(index));
    app.screenshots = true;
    app.router.current = Route::Game(36);
    let size = [1440.0, 1000.0];
    frame(&context, &mut app, size);
    let output = frame(&context, &mut app, size);
    let heading = text_bounds(&output, "Can I play it?")[0];
    assert!(
        heading.min.x <= size[0] - 340.0,
        "gallery pushed the 360px side card out of the viewport: {heading:?}"
    );
}

#[test]
fn job36_details_entry_remains_visible_in_a_compact_selected_game_card() {
    use super::super::launch_readiness_summary as readiness;
    let context = egui::Context::default();
    readable_style(&context);
    let view = readiness::CompactReadiness {
        badge: "Emulator needed",
        tone: readiness::ReadinessPresentationState::NeedsEmulator,
        spinner: false,
        reason: "Synthetic local game has no configured emulator.".into(),
        headline: "Emulator needed".into(),
        play_enabled: false,
        fix: Some(readiness::ReadinessAction::SetUpEmulator),
        pending: false,
        more_findings: 0,
    };
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(620.0, 480.0),
            )),
            ..Default::default()
        },
        |context| {
            egui::CentralPanel::default().show(context, |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(320.0, 400.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
                        readiness::show_compact(ui, &view, 320.0, 3, Some("Game details"));
                    },
                );
            });
        },
    );
    assert!(
        visible_text(&output, "Game details"),
        "the entry to artwork details is clipped in the compact card"
    );
    assert!(visible_text(&output, "Set up emulator"));
}
