use super::*;
#[path = "../../../archivefs-core/src/psp_iso_fixtures.rs"]
mod fixture;

fn iso() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("game.iso");
    std::fs::write(&path, fixture::psp_iso()).unwrap();
    (dir, path)
}

fn wait(panel: &mut Panel) {
    for _ in 0..500 {
        panel.poll();
        if panel.job.is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("analysis did not finish");
}

fn started(panel: &mut Panel, path: &std::path::Path) {
    panel.select((1, path.to_path_buf()));
    panel.start(&egui::Context::default());
}

fn visible_text(output: &egui::FullOutput) -> String {
    fn collect(shape: &egui::epaint::Shape, text: &mut String) {
        match shape {
            egui::epaint::Shape::Text(t) => {
                text.push_str(t.galley.text());
                text.push('\n');
            }
            egui::epaint::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, text)),
            _ => {}
        }
    }
    let mut text = String::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, &mut text);
    }
    text
}

#[test]
fn background_analysis_produces_a_report_and_leaves_the_iso_untouched() {
    let (dir, path) = iso();
    let before = std::fs::read(&path).unwrap();
    let mut panel = Panel::default();
    started(&mut panel, &path);
    assert!(panel.job.is_some());
    wait(&mut panel);
    let report = panel.report.as_ref().expect("report");
    assert_eq!(report.analysis.total_file_bytes, 8192);
    assert!(panel.error.is_none());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn stale_worker_output_cannot_populate_a_new_selection() {
    let (_dir, path) = iso();
    let mut panel = Panel::default();
    panel.select((1, path.to_path_buf()));
    let (tx, receiver) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    panel.job = Some(Job {
        generation: panel.generation,
        cancel: cancel.clone(),
        receiver,
    });
    let old_generation = panel.generation;
    panel.select((2, path.clone()));
    assert!(
        cancel.load(Ordering::Relaxed),
        "selection change cancels the worker"
    );
    assert!(panel.job.is_none());
    // A replacement job from the old generation must still be ignored.
    let (tx2, receiver2) = mpsc::channel();
    panel.job = Some(Job {
        generation: old_generation,
        cancel: Arc::new(AtomicBool::new(false)),
        receiver: receiver2,
    });
    tx2.send(analyze_psp_iso_report(
        &path,
        Arc::new(AtomicBool::new(false)),
    ))
    .ok()
    .unwrap();
    drop(tx);
    panel.poll();
    assert!(panel.report.is_none());
    assert!(panel.job.is_none());
    assert!(panel.error.is_none());
}

#[test]
fn cancel_stops_the_worker_and_reports_it() {
    let (_dir, path) = iso();
    let mut panel = Panel::default();
    panel.select((1, path.to_path_buf()));
    let (_tx, receiver) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    panel.job = Some(Job {
        generation: panel.generation,
        cancel: cancel.clone(),
        receiver,
    });
    panel.cancel();
    assert!(cancel.load(Ordering::Relaxed));
    assert!(panel.job.is_none());
    assert!(analyze_psp_iso_report(&path, cancel).is_err());
}

#[test]
fn changed_iso_invalidates_the_report_and_requires_new_analysis() {
    let (_dir, path) = iso();
    let mut panel = Panel::default();
    started(&mut panel, &path);
    wait(&mut panel);
    assert!(panel.report.is_some());
    let mut longer = fixture::psp_iso();
    longer.extend(vec![0u8; 2048]);
    std::fs::write(&path, longer).unwrap();
    panel.checked = None;
    panel.recheck();
    assert!(panel.report.is_none());
    assert!(panel.error.as_deref().unwrap().contains("Analyse again"));
}

#[test]
fn change_during_analysis_is_rejected_on_arrival() {
    let (_dir, path) = iso();
    let mut panel = Panel::default();
    panel.select((1, path.to_path_buf()));
    let (tx, receiver) = mpsc::channel();
    panel.job = Some(Job {
        generation: panel.generation,
        cancel: Arc::new(AtomicBool::new(false)),
        receiver,
    });
    tx.send(analyze_psp_iso_report(
        &path,
        Arc::new(AtomicBool::new(false)),
    ))
    .ok()
    .unwrap();
    std::fs::write(&path, vec![0u8; 4096]).unwrap();
    panel.poll();
    assert!(panel.report.is_none());
    assert!(panel.error.as_deref().unwrap().contains("changed"));
}

#[test]
fn symlink_and_malformed_sources_show_a_refusal_not_a_report() {
    let (dir, path) = iso();
    let link = dir.path().join("link.iso");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    let mut panel = Panel::default();
    started(&mut panel, &link);
    wait(&mut panel);
    assert!(panel.report.is_none());
    assert!(panel.error.as_deref().unwrap().contains("refused"));
    let bad = dir.path().join("bad.iso");
    std::fs::write(&bad, vec![0u8; 8192]).unwrap();
    let mut panel = Panel::default();
    started(&mut panel, &bad);
    wait(&mut panel);
    assert!(panel.report.is_none());
    assert!(
        panel
            .error
            .as_deref()
            .unwrap()
            .contains("No analysis available")
    );
    assert_eq!(std::fs::read(&bad).unwrap(), vec![0u8; 8192]);
}

fn game(path: &std::path::Path, platform: &str, kind: &str) -> Game {
    let mut archive = super::super::tests::archive(5, "Disposable", Some(platform));
    archive.absolute_path = path.to_path_buf();
    archive.archive_kind = kind.into();
    Game::from_archive(archive)
}

#[test]
fn applies_only_to_loose_psp_iso_files() {
    let p = std::path::Path::new("x.iso");
    assert!(applies(&game(p, "PSP", "direct_file")));
    assert!(!applies(&game(p, "PSP", "zip")));
    assert!(!applies(&game(p, "PS2", "direct_file")));
    assert!(!applies(&game(
        std::path::Path::new("x.cso"),
        "PSP",
        "direct_file"
    )));
}

#[test]
fn rendered_panel_offers_analysis_and_converter_navigation_only() {
    let (_dir, path) = iso();
    let game = game(&path, "PSP", "direct_file");
    let context = egui::Context::default();
    let output = context.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            assert!(!show(ui, &game));
        });
    });
    let text = visible_text(&output);
    assert!(text.contains("PSP ISO content analysis"));
    assert!(text.contains("Analyse ISO contents"));
    assert!(text.contains("Open Converter (ISO"));
    assert!(!text.to_lowercase().contains("delete"));
    assert!(!text.contains("Remove "));
}

#[test]
fn blue_game_details_reaches_the_analyser_and_stays_read_only() {
    use super::super::{library::Library, media_sources::MediaIndex, routes::Route};
    let (_dir, path) = iso();
    let before = std::fs::read(&path).unwrap();
    let context = egui::Context::default();
    let mut app = super::super::tests::fixture(&context);
    let mut archive = super::super::tests::archive(74, "Disposable PSP", Some("PSP"));
    archive.absolute_path = path.clone();
    archive.archive_kind = "direct_file".into();
    let record = archive.clone();
    app.library = Arc::new(Library::new(vec![archive]));
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    app.router.current = Route::Game(74);
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 4800.0),
            )),
            ..Default::default()
        },
        |ctx| app.show(ctx),
    );
    let text = visible_text(&output);
    assert!(text.contains("PSP ISO content analysis"), "{text}");
    assert!(text.contains("Open Converter (ISO"));
    assert_eq!(app.router.current, Route::Game(74));
    assert_eq!(app.library.game(74).unwrap().archive, record);
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

fn text_center(output: &egui::FullOutput, needle: &str) -> Option<egui::Pos2> {
    fn walk(shape: &egui::epaint::Shape, needle: &str) -> Option<egui::Pos2> {
        match shape {
            egui::epaint::Shape::Text(t) if t.galley.text().contains(needle) => {
                Some(t.pos + t.galley.rect.center().to_vec2())
            }
            egui::epaint::Shape::Vec(shapes) => shapes.iter().find_map(|s| walk(s, needle)),
            _ => None,
        }
    }
    output.shapes.iter().find_map(|c| walk(&c.shape, needle))
}

fn tall_frame(
    context: &egui::Context,
    app: &mut super::super::App,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 4800.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| app.show(ctx),
    )
}

fn blue_app_with_psp_games(
    context: &egui::Context,
    games: &[(i64, &std::path::Path)],
) -> super::super::App {
    use super::super::{library::Library, media_sources::MediaIndex};
    let mut app = super::super::tests::fixture(context);
    app.simple = super::super::simple_shell::ShellState::default();
    let archives = games
        .iter()
        .map(|(id, path)| {
            let mut archive = super::super::tests::archive(*id, "Disposable PSP", Some("PSP"));
            archive.absolute_path = path.to_path_buf();
            archive.archive_kind = "direct_file".into();
            archive
        })
        .collect();
    app.library = Arc::new(Library::new(archives));
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    app
}

fn shared_panel(context: &egui::Context) -> Arc<Mutex<Panel>> {
    context.data_mut(|d| {
        d.get_temp_mut_or_default::<Arc<Mutex<Panel>>>(egui::Id::new("psp_iso_analyser"))
            .clone()
    })
}

#[test]
fn blue_converter_button_navigates_to_the_existing_iso_to_cso_workflow() {
    use super::super::routes::{Route, Section};
    let (_dir, path) = iso();
    let before = std::fs::read(&path).unwrap();
    let context = egui::Context::default();
    let mut app = blue_app_with_psp_games(&context, &[(74, path.as_path())]);
    app.router.current = Route::Game(74);
    let first = tall_frame(&context, &mut app, vec![]);
    let target = text_center(&first, "Open Converter (ISO to CSO)").expect("button is drawn");
    let click = |pressed| egui::Event::PointerButton {
        pos: target,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    tall_frame(&context, &mut app, vec![egui::Event::PointerMoved(target)]);
    tall_frame(&context, &mut app, vec![click(true)]);
    tall_frame(&context, &mut app, vec![click(false)]);
    assert_eq!(app.router.current, Route::Section(Section::Converter));
    let strings = super::super::tests::text(&tall_frame(&context, &mut app, vec![]));
    for expected in ["Disc Conversion", "Other supported conversions"] {
        assert!(
            strings.iter().any(|s| s.contains(expected)),
            "existing Converter content missing: {expected}"
        );
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn selection_change_and_cancellation_never_expose_another_games_report() {
    use super::super::routes::Route;
    let (dir_a, path_a) = iso();
    let path_b = dir_a.path().join("other.iso");
    std::fs::write(&path_b, fixture::psp_iso()).unwrap();
    let context = egui::Context::default();
    let mut app =
        blue_app_with_psp_games(&context, &[(1, path_a.as_path()), (2, path_b.as_path())]);
    let marker = "INTRO.PMF";

    // Completed report for game 1 is visible only for game 1.
    app.router.current = Route::Game(1);
    tall_frame(&context, &mut app, vec![]);
    let panel = shared_panel(&context);
    panel.lock().unwrap().start(&context);
    for _ in 0..500 {
        tall_frame(&context, &mut app, vec![]);
        if panel.lock().unwrap().report.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let own = super::super::tests::text(&tall_frame(&context, &mut app, vec![])).join("\n");
    assert!(own.contains(marker), "game 1 shows its own report");
    app.router.current = Route::Game(2);
    let other = super::super::tests::text(&tall_frame(&context, &mut app, vec![])).join("\n");
    assert!(
        !other.contains(marker),
        "game 2 must not show game 1's report"
    );
    assert!(panel.lock().unwrap().report.is_none());

    // Cancel by switching away mid-flight, then wait out the old worker.
    app.router.current = Route::Game(1);
    tall_frame(&context, &mut app, vec![]);
    panel.lock().unwrap().start(&context);
    let cancel = panel.lock().unwrap().job.as_ref().unwrap().cancel.clone();
    app.router.current = Route::Game(2);
    tall_frame(&context, &mut app, vec![]);
    assert!(
        cancel.load(Ordering::Relaxed),
        "switching cancels the old worker"
    );
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(10));
        let shown = super::super::tests::text(&tall_frame(&context, &mut app, vec![])).join("\n");
        assert!(!shown.contains(marker), "late output leaked into game 2");
    }
    assert!(panel.lock().unwrap().report.is_none());
    // Returning to game 1 does not resurrect the cancelled analysis.
    app.router.current = Route::Game(1);
    let back = super::super::tests::text(&tall_frame(&context, &mut app, vec![])).join("\n");
    assert!(!back.contains(marker));
    assert!(panel.lock().unwrap().report.is_none());
}
