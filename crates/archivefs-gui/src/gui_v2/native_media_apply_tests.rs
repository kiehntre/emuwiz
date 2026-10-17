//! Synthetic end-to-end confirmation boundary; the test never applies to the
//! default database and never launches the application or a scanner on user data.
use super::*;
use archivefs_core::ingestion::{
    bounded_import::{begin_bounded_preview, prepare_bounded_import},
    preview::{SpectrumCatalogue, preview_media},
};
fn frame(
    state: &mut State,
    ctx: &egui::Context,
    db: &Path,
    views: &[SourceFolderView],
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
                ui.set_max_width(760.0);
                state.show(ui, db, 1, views, false)
            });
        },
    )
}
fn text_rect(out: &egui::FullOutput, label: &str) -> Option<egui::Rect> {
    fn find(s: &egui::Shape, l: &str) -> Option<egui::Rect> {
        match s {
            egui::Shape::Text(t) if t.galley.text().contains(l) => {
                Some(egui::Rect::from_min_size(t.pos, t.galley.size()))
            }
            egui::Shape::Vec(v) => v.iter().find_map(|s| find(s, l)),
            _ => None,
        }
    }
    out.shapes.iter().find_map(|s| find(&s.shape, label))
}
fn click(
    state: &mut State,
    ctx: &egui::Context,
    db: &Path,
    views: &[SourceFolderView],
    label: &str,
) {
    let _ = frame(state, ctx, db, views, vec![]);
    let out = frame(state, ctx, db, views, vec![]);
    let rect = text_rect(&out, label).unwrap();
    assert!(
        rect.min.y >= 0.0 && rect.max.y < 600.0 && rect.max.x < 1024.0,
        "primary control {label} must fit 1024x600: {rect:?}"
    );
    for pressed in [true, false] {
        frame(
            state,
            ctx,
            db,
            views,
            vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
}
#[test]
fn bounded_confirmation_requires_review_backup_ack_and_explicit_apply_at_small_resolution() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("zxs");
    std::fs::create_dir(&root).unwrap();
    let dbpath = t.path().join("copy.sqlite3");
    let cfg = t.path().join("config.toml");
    let config = archivefs_core::Config {
        source_folders: vec![root.clone()],
        mount_root: PathBuf::new(),
        ratarmount_bin: String::new(),
        master_rom_root: None,
    };
    let mut bytes = vec![0; 49182];
    bytes[7] = 128;
    bytes[29] = 1;
    std::fs::write(root.join("existing.z80"), &bytes).unwrap();
    let mut db = Database::open_or_create(&dbpath).unwrap();
    archivefs_core::scan_and_persist(&mut db, &config, "synthetic-seed").unwrap();
    archivefs_core::save_source_folder_configs_to(
        &cfg,
        &[archivefs_core::SourceFolderConfig {
            path: root.clone(),
            enabled: true,
            created_at: None,
        }],
        &t.path().join("mount"),
        "",
        None,
    )
    .unwrap();
    bytes[100] = 7;
    let new = root.join("candidate.z80");
    std::fs::write(&new, &bytes).unwrap();
    let hashes = archivefs_core::identity_source::hashing::hash_file(
        &new,
        &archivefs_core::safe_read::TrustedRoots::none(),
        None,
    )
    .unwrap();
    let dat = t.path().join("trusted.dat");
    std::fs::write(&dat,format!("<datafile><header><name>Sinclair ZX Spectrum - Games - [Z80]</name></header><game name=\"Candidate\"><rom name=\"anything\" size=\"{}\" sha1=\"{}\"/></game></datafile>",hashes.bytes_hashed,hashes.sha1)).unwrap();
    let cat = SpectrumCatalogue::from_parsed(
        &archivefs_core::dat::parsers::parse_dat_file(&dat, Default::default())
            .unwrap()
            .dat,
    )
    .unwrap();
    let context = begin_bounded_preview(&db, &cfg).unwrap();
    let mut report = preview_media(
        &config,
        &[],
        &db.load_archives().unwrap(),
        &[cat],
        &AtomicBool::new(false),
        &mut |_, _| {},
    )
    .unwrap();
    report.bounded_import = Some(prepare_bounded_import(&report, context).unwrap());
    let views = archivefs_core::build_source_folder_views(
        &archivefs_core::load_source_folder_configs_from(&cfg).unwrap(),
        &db.list_source_folders().unwrap(),
    );
    let before = db.load_archives().unwrap();
    // The ordinary path chooses and prepares the batch without manual selection.
    let mut automatic = State::default();
    automatic.database = dbpath.clone();
    automatic.generation = 1;
    automatic.sources = views.clone();
    automatic.root = root.display().to_string();
    automatic.reviewed_root = automatic.root.clone();
    let mut journey = super::super::native_import_journey::Journey::new(
        &report,
        report.bounded_import.as_ref().unwrap(),
    )
    .unwrap();
    let prepared = journey
        .prepare_first(&dbpath, &cfg, &AtomicBool::new(false), &mut |_, _| {})
        .unwrap()
        .unwrap();
    automatic.report = Some(super::super::native_media_subset::subset_report(prepared));
    automatic.journey = Some(journey);
    let automatic_ctx = egui::Context::default();
    click(
        &mut automatic,
        &automatic_ctx,
        &dbpath,
        &views,
        "Review bounded import…",
    );
    assert!(automatic.take_apply_request().is_none());
    click(
        &mut automatic,
        &automatic_ctx,
        &dbpath,
        &views,
        "Confirm and apply reviewed allowlist",
    );
    assert!(
        automatic.take_apply_request().is_none(),
        "automatic scheduling cannot approve a write"
    );
    click(
        &mut automatic,
        &automatic_ctx,
        &dbpath,
        &views,
        "I reviewed this exact allowlist",
    );
    click(
        &mut automatic,
        &automatic_ctx,
        &dbpath,
        &views,
        "Confirm and apply reviewed allowlist",
    );
    let (automatic_plan, _) = automatic.take_apply_request().unwrap();
    assert_eq!(automatic_plan.projected_insertions(), 1);
    assert_eq!(db.load_archives().unwrap(), before);
    let mut state = State::default();
    state.database = dbpath.clone();
    state.generation = 1;
    state.sources = views.clone();
    state.root = root.display().to_string();
    state.reviewed_root = state.root.clone();
    state.report = Some(report);
    let ctx = egui::Context::default();
    egui::collapsing_header::CollapsingState::load_with_default_open(
        &ctx,
        egui::Id::new("blue_media_preview"),
        true,
    )
    .store(&ctx);
    // Set the header's egui ID through a click if its scoped ID differs.
    let out = frame(&mut state, &ctx, &dbpath, &views, vec![]);
    if text_rect(&out, "Review bounded import…").is_none() {
        click(&mut state, &ctx, &dbpath, &views, "Read-only media preview");
    }
    for _ in 0..200 {
        frame(&mut state, &ctx, &dbpath, &views, vec![]);
        if state.subset.ready_for_test() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(state.subset.ready_for_test());
    assert!(state.take_apply_request().is_none());
    click(&mut state, &ctx, &dbpath, &views, "Review bounded import…");
    assert!(state.confirm_apply);
    assert!(!state.backup_acknowledged);
    click(
        &mut state,
        &ctx,
        &dbpath,
        &views,
        "Confirm and apply reviewed allowlist",
    );
    assert!(
        state.take_apply_request().is_none(),
        "disabled button must not start a write"
    );
    click(&mut state, &ctx, &dbpath, &views, "Cancel import review");
    assert!(!state.confirm_apply);
    assert!(state.take_apply_request().is_none());
    click(&mut state, &ctx, &dbpath, &views, "Review bounded import…");
    click(
        &mut state,
        &ctx,
        &dbpath,
        &views,
        "I reviewed this exact allowlist",
    );
    click(
        &mut state,
        &ctx,
        &dbpath,
        &views,
        "Confirm and apply reviewed allowlist",
    );
    let (plan, _) = state.take_apply_request().unwrap();
    assert_eq!(plan.projected_insertions(), 1);
    assert!(state.report.is_none());
    assert_eq!(
        db.load_archives().unwrap(),
        before,
        "confirmation only hands the sealed plan to the existing worker"
    );
    let saved_path = t.path().join("reviewed.json");
    let pin = plan.save_evidence(&saved_path).unwrap();
    let mut fresh = State::default();
    fresh.root = root.display().to_string();
    frame(&mut fresh, &ctx, &dbpath, &views, vec![]);
    fresh.subset.load_for_test(ctx.clone(), saved_path, pin);
    for _ in 0..200 {
        frame(&mut fresh, &ctx, &dbpath, &views, vec![]);
        if fresh.subset.ready_for_test() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        fresh.subset.ready_for_test(),
        "fresh Blue state can retain loaded evidence without preparing a root preview"
    );
    assert!(fresh.report.is_none());
    assert!(fresh.take_apply_request().is_none());
    let out = frame(&mut fresh, &ctx, &dbpath, &views, vec![]);
    if text_rect(&out, "Select first bounded batch").is_none() {
        click(
            &mut fresh,
            &ctx,
            &dbpath,
            &views,
            "Select an exact bounded batch",
        );
    }
    click(
        &mut fresh,
        &ctx,
        &dbpath,
        &views,
        "Select first bounded batch",
    );
    assert_eq!(fresh.subset.selected_for_test(), 1);
    assert!(fresh.report.is_none());
    assert!(fresh.take_apply_request().is_none());
    click(&mut fresh, &ctx, &dbpath, &views, "Clear selection");
    assert_eq!(fresh.subset.selected_for_test(), 0);

    // Any app generation change revokes the loaded evidence and pending confirmation.
    fresh.generation = 0;
    frame(&mut fresh, &ctx, &dbpath, &views, vec![]);
    assert!(!fresh.subset.ready_for_test());
}
#[test]
fn generic_source_dispatch_cannot_substitute_a_scan_for_bounded_apply() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("zxs");
    let error = crate::platform_source_actions::run_source_action(
        &crate::SourceAction::BoundedImport(path.clone()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("sealed plan"));
    assert!(!path.exists());
}
#[test]
fn committed_skip_reasons_are_visible_and_paged() {
    let details = (0..125)
        .map(|n| format!("/synthetic/zxs/{n:03}.z80: changed or protected since preview"))
        .collect::<Vec<_>>()
        .join("\n");
    let ctx = egui::Context::default();
    let mut page = 1;
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 600.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default()
                .show(ctx, |ui| show_apply_details(ui, &details, &mut page));
        },
    );
    assert!(text_rect(&output, "050.z80: changed or protected").is_some());
    assert!(text_rect(&output, "000.z80").is_none());
    assert!(text_rect(&output, "100.z80").is_none());
    let next = text_rect(&output, "Next reasons").unwrap();
    assert!(next.max.y < 600.0);
}
