use super::*;
#[test]
fn disconnected_worker_never_claims_no_commit_or_success() {
    let (tx, rx) = mpsc::channel();
    drop(tx);
    let mut state = State::default();
    state.job = Some(Job {
        target: None,
        receiver: rx,
    });
    state.poll();
    assert!(state.message.contains("unknown"));
    assert!(state.message.contains("History"));
    assert!(!state.changed);
}
#[test]
fn stale_database_worker_result_is_not_presented_as_current() {
    let (tx, rx) = mpsc::channel();
    tx.send(Ok(Value::Loaded(PathBuf::from("old.sqlite"), Vec::new())))
        .unwrap();
    let mut state = State::default();
    state.requested = Some(PathBuf::from("new.sqlite"));
    state.job = Some(Job {
        target: Some(PathBuf::from("old.sqlite")),
        receiver: rx,
    });
    state.poll();
    assert!(state.database.is_none());
    assert!(state.message.contains("selected database changed"));
}
#[test]
fn undone_message_reports_backend_exact_counts_and_reloads_after_commit() {
    let (tx, rx) = mpsc::channel();
    tx.send(Ok(Value::Undone(DiscoveryImportUndoResult {
        operation_id: "batch".into(),
        removed_records: 3,
        removed_assignments: 3,
        already_undone: false,
    })))
    .unwrap();
    let mut s = State::default();
    s.job = Some(Job {
        target: None,
        receiver: rx,
    });
    s.poll();
    assert!(s.changed);
    assert!(s.loaded.is_none());
    assert!(s.message.contains("3 created archive"));
}
#[test]
fn repeated_undo_reports_no_changes_and_no_reload_claim() {
    let (tx, rx) = mpsc::channel();
    tx.send(Ok(Value::Undone(DiscoveryImportUndoResult {
        operation_id: "batch".into(),
        removed_records: 0,
        removed_assignments: 0,
        already_undone: true,
    })))
    .unwrap();
    let mut s = State::default();
    s.job = Some(Job {
        target: None,
        receiver: rx,
    });
    s.poll();
    assert!(!s.changed);
    assert!(s.message.contains("already undone"));
}
#[test]
fn blocked_worker_result_clears_confirmation() {
    let (tx, rx) = mpsc::channel();
    tx.send(Err("Entire batch refused; unchanged".into()))
        .unwrap();
    let mut s = State::default();
    s.confirm = true;
    s.job = Some(Job {
        target: None,
        receiver: rx,
    });
    s.poll();
    assert!(!s.confirm);
    assert!(s.preview.is_none());
    assert!(!s.changed);
}

fn frame(
    s: &mut State,
    ctx: &egui::Context,
    db: &Path,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 600.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                s.show(ui, Some(db), false, None);
            });
        },
    )
}
fn label_rect(output: &egui::FullOutput, label: &str) -> Option<egui::Rect> {
    fn find(shape: &egui::Shape, label: &str) -> Option<egui::Rect> {
        match shape {
            egui::Shape::Text(t) if t.galley.text() == label => {
                Some(egui::Rect::from_min_size(t.pos, t.galley.size()))
            }
            egui::Shape::Vec(v) => v.iter().find_map(|s| find(s, label)),
            _ => None,
        }
    }
    output.shapes.iter().find_map(|s| {
        find(&s.shape, label).filter(|r| {
            s.clip_rect.contains(r.center()) && r.center().y < 600.0 && r.center().y > 0.0
        })
    })
}
fn click(s: &mut State, ctx: &egui::Context, db: &Path, label: &str) {
    for _ in 0..30 {
        let out = frame(s, ctx, db, Vec::new());
        if let Some(r) = label_rect(&out, label) {
            for pressed in [true, false] {
                frame(
                    s,
                    ctx,
                    db,
                    vec![
                        egui::Event::PointerMoved(r.center()),
                        egui::Event::PointerButton {
                            pos: r.center(),
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
            return;
        }
        frame(
            s,
            ctx,
            db,
            vec![
                egui::Event::PointerMoved(egui::pos2(400.0, 380.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    phase: egui::TouchPhase::Move,
                    delta: egui::vec2(0.0, -180.0),
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    panic!("Visible control missing: {label}");
}
fn poll_ready(s: &mut State, ctx: &egui::Context, db: &Path) {
    for _ in 0..400 {
        frame(s, ctx, db, Vec::new());
        if s.job.is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("Worker timeout");
}
#[test]
fn actual_import_results_preview_cancel_confirm_and_result_journey_preserves_roms() {
    use archivefs_core::ingestion::{
        bounded_import::{apply_bounded_import, begin_bounded_preview, prepare_bounded_import},
        preview::{SpectrumCatalogue, preview_media},
    };
    use std::sync::atomic::AtomicBool;
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("zxs");
    std::fs::create_dir_all(&root).unwrap();
    let bytes = {
        let mut b = vec![0; 49182];
        b[7] = 128;
        b[27] = 255;
        b[28] = 255;
        b[29] = 1;
        b
    };
    std::fs::write(root.join("existing.z80"), &bytes).unwrap();
    let db = t.path().join("library.sqlite3");
    let cfg = t.path().join("config.toml");
    let config = archivefs_core::Config {
        source_folders: vec![root.clone()],
        mount_root: t.path().join("mount"),
        ratarmount_bin: String::new(),
        master_rom_root: None,
    };
    let mut d = Database::open_or_create(&db).unwrap();
    archivefs_core::scan_and_persist(&mut d, &config, "synthetic-seed").unwrap();
    archivefs_core::save_source_folder_configs_to(
        &cfg,
        &[archivefs_core::SourceFolderConfig {
            path: root.clone(),
            enabled: true,
            created_at: None,
        }],
        &config.mount_root,
        "",
        None,
    )
    .unwrap();
    drop(d);
    let mut new = bytes.clone();
    new[100] = 2;
    std::fs::write(root.join("new.z80"), &new).unwrap();
    let dat = t.path().join("fixture.dat");
    std::fs::write(&dat,r#"<datafile><header><name>Sinclair ZX Spectrum - Games - [Z80]</name></header><game name="Synthetic"><rom name="ref.z80" size="49182" crc="12345678"/></game></datafile>"#).unwrap();
    let cat = SpectrumCatalogue::from_parsed(
        &archivefs_core::dat::parsers::parse_dat_file(&dat, Default::default())
            .unwrap()
            .dat,
    )
    .unwrap();
    let d = Database::open_read_only(&db).unwrap();
    let context = begin_bounded_preview(&d, &cfg).unwrap();
    let p = preview_media(
        &config,
        &[],
        &d.load_archives().unwrap(),
        &[cat],
        &AtomicBool::new(false),
        &mut |_, _| {},
    )
    .unwrap();
    let plan = prepare_bounded_import(&p, context).unwrap();
    drop(d);
    let result = apply_bounded_import(&plan, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(result.inserted.len(), 1);
    let ctx = egui::Context::default();
    let mut state = State::default();
    frame(&mut state, &ctx, &db, Vec::new());
    poll_ready(&mut state, &ctx, &db);
    assert_eq!(
        state.operations[0].operation_id,
        result.operation_id.unwrap()
    );
    click(&mut state, &ctx, &db, "Preview Undo");
    poll_ready(&mut state, &ctx, &db);
    assert!(state.preview.as_ref().unwrap().can_undo());
    click(&mut state, &ctx, &db, "Review Undo confirmation…");
    assert!(state.confirm);
    click(&mut state, &ctx, &db, "Cancel Undo");
    assert!(!state.confirm);
    assert_eq!(
        Database::open_read_only(&db)
            .unwrap()
            .load_archives()
            .unwrap()
            .len(),
        2
    );
    click(&mut state, &ctx, &db, "Review Undo confirmation…");
    click(&mut state, &ctx, &db, "Confirm Undo");
    poll_ready(&mut state, &ctx, &db);
    assert!(state.message.contains("Undo committed"));
    assert_eq!(
        Database::open_read_only(&db)
            .unwrap()
            .load_archives()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(std::fs::read(root.join("new.z80")).unwrap(), new);
    assert_eq!(std::fs::read(root.join("existing.z80")).unwrap(), bytes);
}
