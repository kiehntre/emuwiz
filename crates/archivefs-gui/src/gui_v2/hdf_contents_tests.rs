use super::*;
#[path = "../../../archivefs-core/src/hdf_contents_fixtures.rs"]
mod fixture;

fn disk() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("owned.hdf");
    std::fs::write(&path, fixture::rdb(&fixture::flat(1, 150))).unwrap();
    (dir, path)
}

#[test]
fn hdf_contents_browser_navigates_pages_and_directories_without_writes() {
    let (_dir, path) = disk();
    let before = std::fs::read(&path).unwrap();
    let opened = inspect(Action::Open(path.clone())).unwrap();
    assert!(opened.page.is_none());
    let root = inspect(Action::Root(opened, 0)).unwrap();
    assert_eq!(root.page.as_ref().unwrap().entries.len(), 128);
    let next = root.page.as_ref().unwrap().next_offset.unwrap();
    let page = inspect(Action::Page(root, next)).unwrap();
    assert_eq!(page.offset, 128);
    let first = inspect(Action::Page(page, 0)).unwrap();
    let child = first.page.as_ref().unwrap().entries[0].directory.unwrap();
    let sub = inspect(Action::Directory(first, "Games".into(), child)).unwrap();
    assert!(sub.page.as_ref().unwrap().entries.is_empty());
    let parent = inspect(Action::Up(sub)).unwrap();
    assert_eq!(parent.breadcrumbs.len(), 1);
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn hdf_contents_old_selection_worker_cannot_populate_new_selection() {
    let (_dir, path) = disk();
    let mut panel = Panel::default();
    panel.select((1, path.clone()));
    let (tx, receiver) = mpsc::channel();
    panel.job = Some(Job {
        generation: panel.generation,
        receiver,
    });
    panel.select((2, path.clone()));
    tx.send(inspect(Action::Open(path)).map_err(|e| e.to_string()))
        .ok()
        .unwrap();
    panel.poll();
    assert!(panel.browser.is_none());
    assert!(panel.job.is_none());
    assert!(panel.error.is_none());
}

#[test]
fn hdf_contents_changed_media_requires_explicit_reinspection() {
    let (_dir, path) = disk();
    let mut panel = Panel::default();
    panel.select((1, path.clone()));
    panel.browser = Some(inspect(Action::Open(path.clone())).unwrap());
    std::fs::write(path, b"changed").unwrap();
    panel.recheck();
    assert!(panel.browser.is_none());
    assert!(panel.error.as_deref().unwrap().contains("Inspect again"));
    assert!(panel.job.is_none());
}

#[test]
fn hdf_contents_filesystem_refusal_keeps_other_partitions_available() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("partitions.hdf");
    let mut bytes = fixture::two_partitions();
    let offset = bytes.len() / 3;
    bytes[offset..offset + 4].copy_from_slice(b"PFS\x03");
    fixture::put(&mut bytes, 512 + 192, u32::from_be_bytes(*b"PFS\x03"));
    fixture::checksum(&mut bytes[512..1024], 8);
    std::fs::write(&path, bytes).unwrap();
    let opened = inspect(Action::Open(path)).unwrap();
    let refused = inspect(Action::Root(opened, 0)).unwrap();
    assert!(
        refused
            .error
            .as_deref()
            .unwrap()
            .contains("UnsupportedFilesystem")
    );
    assert!(refused.page.is_none());
    assert_eq!(refused.inspection.disk().rdb.partitions.len(), 2);
    let supported = inspect(Action::Root(refused, 1)).unwrap();
    assert_eq!(supported.page.unwrap().entries.len(), 3);
}

#[test]
fn hdf_contents_refusals_do_not_claim_verification_or_readiness() {
    assert!(EXPLANATION.contains("do not identify a game"));
    assert!(EXPLANATION.contains("verify a DAT match"));
    assert!(EXPLANATION.contains("launch readiness"));
    assert!(LIMITS.contains("detection-only"));
    let mut panel = Panel::default();
    panel.select((1, PathBuf::from("unused-disposable-path.hdf")));
    let (tx, receiver) = mpsc::channel();
    panel.job = Some(Job {
        generation: panel.generation,
        receiver,
    });
    tx.send(Err("Corrupt directory".into())).ok().unwrap();
    panel.poll();
    assert!(panel.browser.is_none());
    assert_eq!(panel.error.as_deref(), Some("Corrupt directory"));
}

#[test]
fn hdf_contents_headless_panel_shows_evidenced_labels_without_identity_changes() {
    let (_dir, path) = disk();
    let mut archive = super::super::tests::archive(73, "Disposable image", None);
    archive.absolute_path = path.clone();
    let game = Game::from_archive(archive);
    assert!(applies(&game));
    assert!(!game.identified);
    assert!(game.dat_exact.is_none());
    let context = egui::Context::default();
    let mut panel = Panel::default();
    panel.select((game.archive.id, path.clone()));
    panel.browser = Some(inspect(Action::Root(inspect(Action::Open(path)).unwrap(), 0)).unwrap());
    context.data_mut(|data| {
        data.insert_temp(
            egui::Id::new("hdf_contents_controller"),
            Arc::new(Mutex::new(panel)),
        )
    });
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 1200.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, &game));
        },
    );
    fn text(shape: &egui::epaint::Shape, output: &mut String) {
        match shape {
            egui::epaint::Shape::Text(t) => {
                output.push_str(t.galley.text());
                output.push('\n');
            }
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    text(shape, output);
                }
            }
            _ => {}
        }
    }
    let mut visible = String::new();
    for clipped in output.shapes {
        text(&clipped.shape, &mut visible);
    }
    for expected in [
        "Amiga filesystem contents",
        "Partition 1: DH0",
        "Volume: Work",
        "Games",
        "file0000",
        "Next page",
        "Filesystem inspection only",
    ] {
        assert!(visible.contains(expected), "Missing {expected}: {visible}");
    }
    assert!(!game.identified);
    assert!(game.dat_exact.is_none());
    assert!(game.archive.identity_report.is_none());
}

#[test]
fn amiga_contents_supported_suffixes_offer_explicit_inspection_without_changing_game_evidence() {
    for extension in ["hdf", "rdb", "hdfx", "adf", "RDB", "HDFX", "ADF"] {
        let mut archive = super::super::tests::archive(73, "Disposable image", None);
        archive.absolute_path = PathBuf::from(format!("unused-disposable.{extension}"));
        let game = Game::from_archive(archive);
        assert!(
            applies(&game),
            "supported contents inspector is unreachable for {extension}"
        );
        assert!(!game.identified);
        assert!(game.dat_exact.is_none());
        assert!(game.archive.identity_report.is_none());
    }
    for extension in ["adz", "dms", "ipf", "hfe", "zip", "adfs"] {
        let mut archive = super::super::tests::archive(74, "Not a supported container", None);
        archive.absolute_path = PathBuf::from(format!("unused-disposable.{extension}"));
        assert!(!applies(&Game::from_archive(archive)));
    }
}

fn visible_text(output: &egui::FullOutput) -> String {
    fn collect(shape: &egui::epaint::Shape, text: &mut String) {
        match shape {
            egui::epaint::Shape::Text(t) => {
                text.push_str(t.galley.text());
                text.push('\n');
            }
            egui::epaint::Shape::Vec(shapes) => {
                for s in shapes {
                    collect(s, text);
                }
            }
            _ => {}
        }
    }
    let mut text = String::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, &mut text);
    }
    text
}

fn frame_contents(context: &egui::Context, game: &Game) -> egui::FullOutput {
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 2200.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, game));
        },
    )
}

#[test]
fn amiga_contents_explicit_action_and_validated_labels_are_shared_by_all_four_suffixes() {
    for extension in ["hdf", "rdb", "hdfx", "adf"] {
        for dos in [0, 1] {
            for wrapped in [false, true] {
                let directory = tempfile::tempdir().unwrap();
                let path = directory.path().join(format!("owned.{extension}"));
                let flat = if extension == "adf" {
                    fixture::floppy(dos, 150, false)
                } else {
                    fixture::flat(dos, 150)
                };
                let bytes = if wrapped { fixture::rdb(&flat) } else { flat };
                std::fs::write(&path, &bytes).unwrap();
                let mut archive =
                    super::super::tests::archive(73, "Disposable image", Some("Amiga"));
                archive.absolute_path = path.clone();
                let game = Game::from_archive(archive);
                let before = game.archive.clone();
                let canonical_before = game
                    .archive
                    .identity_report
                    .as_ref()
                    .map(archivefs_core::launch::canonical_identity_from_game_report);
                let context = egui::Context::default();
                let output = frame_contents(&context, &game);
                let text = visible_text(&output);
                assert!(text.contains("Amiga filesystem contents"));
                assert!(text.contains("Inspect / refresh contents"));
                assert!(text.contains(READ_SCOPE));
                assert!(text.contains(FORMATS));
                let shared = context
                    .data_mut(|data| {
                        data.get_temp::<Arc<Mutex<Panel>>>(egui::Id::new("hdf_contents_controller"))
                    })
                    .unwrap();
                assert!(
                    shared.lock().unwrap().job.is_none(),
                    "opening Details must not start inspection"
                );
                assert!(shared.lock().unwrap().browser.is_none());
                // The existing worker's action is exercised below; no alternate UI reader.
                let browser = inspect(Action::Root(
                    inspect(Action::Open(path.clone())).unwrap(),
                    0,
                ))
                .unwrap();
                shared.lock().unwrap().browser = Some(browser);
                let output = frame_contents(&context, &game);
                let text = visible_text(&output);
                for expected in [
                    "Volume: Work",
                    "Games",
                    "file0000",
                    "Next page",
                    "Filesystem inspection only",
                    "not a filesystem integrity check",
                ] {
                    assert!(text.contains(expected), "{extension} {expected}: {text}");
                }
                assert!(text.contains(if wrapped {
                    "RDB partition container"
                } else {
                    "flat filesystem image at byte 0"
                }));
                assert_eq!(game.archive, before);
                assert_eq!(
                    game.archive
                        .identity_report
                        .as_ref()
                        .map(archivefs_core::launch::canonical_identity_from_game_report),
                    canonical_before
                );
                assert!(!game.identified);
                assert!(game.dat_exact.is_none());
                assert!(game.dat_state.is_none());
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
            }
        }
    }
    for wording in [
        "512-byte blocks only",
        "PFS/SFS/MuFS",
        "detection-only",
        "Links are not followed",
        "128 entries",
        "4096",
        "depth 32",
        "10,000",
    ] {
        assert!(LIMITS.contains(wording));
    }
}

#[test]
fn amiga_contents_cross_format_selection_replacement_and_late_workers_are_isolated() {
    let directory = tempfile::tempdir().unwrap();
    for extension in ["rdb", "hdfx", "adf"] {
        let old = directory.path().join("old.hdf");
        let path = directory.path().join(format!("new.{extension}"));
        std::fs::write(&old, fixture::rdb(&fixture::flat(1, 1))).unwrap();
        std::fs::write(
            &path,
            if extension == "adf" {
                fixture::floppy(0, 1, false)
            } else {
                fixture::rdb(&fixture::flat(0, 1))
            },
        )
        .unwrap();
        let mut panel = Panel::default();
        panel.select((1, old.clone()));
        let (tx, receiver) = mpsc::channel();
        panel.job = Some(Job {
            generation: panel.generation,
            receiver,
        });
        panel.select((2, path.clone()));
        tx.send(inspect(Action::Open(old))).ok().unwrap();
        panel.poll();
        assert!(panel.browser.is_none());
        assert!(panel.error.is_none());
        panel.browser = Some(
            inspect(Action::Root(
                inspect(Action::Open(path.clone())).unwrap(),
                0,
            ))
            .unwrap(),
        );
        let replaced = path.with_extension("replacement");
        std::fs::write(&replaced, fixture::flat(1, 1)).unwrap();
        std::fs::rename(&replaced, &path).unwrap();
        panel.checked = None;
        panel.recheck();
        assert!(panel.browser.is_none());
        assert!(panel.error.as_deref().unwrap().contains("Inspect again"));
    }
}

#[test]
fn amiga_contents_game_details_reachability_does_not_start_launch_or_promote_evidence() {
    use super::super::{library::Library, media_sources::MediaIndex, routes::Route};
    for extension in ["rdb", "hdfx", "adf"] {
        let context = egui::Context::default();
        let mut app = super::super::tests::fixture(&context);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("owned.{extension}"));
        let bytes = if extension == "adf" {
            fixture::floppy(1, 2, false)
        } else {
            fixture::rdb(&fixture::flat(1, 2))
        };
        std::fs::write(&path, &bytes).unwrap();
        let mut archive = super::super::tests::archive(73, "Disposable image", Some("Amiga"));
        archive.absolute_path = path.clone();
        let before = archive.clone();
        app.library = Arc::new(Library::new(vec![archive]));
        app.artwork.index = Some(Arc::new(MediaIndex::default()));
        app.router.current = Route::Game(73);
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
        assert!(
            text.contains("Amiga filesystem contents"),
            "{extension}: {text}"
        );
        assert!(text.contains("Inspect / refresh contents"));
        assert_eq!(app.router.current, Route::Game(73));
        let game = app.library.game(73).unwrap();
        assert_eq!(game.archive, before);
        assert!(!game.identified);
        assert!(game.dat_exact.is_none());
        assert!(game.dat_state.is_none());
        if let Some(workflow) = &app.native_workflows {
            assert!(!workflow.app.launch_amiga_whdload.is_active());
            assert!(!workflow.app.launch_standalone.is_active());
            assert!(!workflow.app.launch_retroarch.is_active());
        }
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}
