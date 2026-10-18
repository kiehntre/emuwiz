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
        "HDF contents",
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
