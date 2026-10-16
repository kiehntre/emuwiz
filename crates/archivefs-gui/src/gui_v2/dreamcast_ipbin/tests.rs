use super::*;

fn game() -> Game {
    let mut archive = super::super::tests::archive(1, "Synthetic", Some("Dreamcast"));
    archive.absolute_path = PathBuf::from("/synthetic-only/game.gdi");
    Game::from_archive(archive)
}
fn text(shapes: &[egui::epaint::ClippedShape]) -> String {
    fn collect(s: &egui::epaint::Shape, out: &mut String) {
        match s {
            egui::epaint::Shape::Text(t) => {
                out.push_str(&t.galley.job.text);
                out.push('\n');
            }
            egui::epaint::Shape::Vec(v) => {
                for s in v {
                    collect(s, out)
                }
            }
            _ => {}
        }
    }
    let mut out = String::new();
    for s in shapes {
        collect(&s.shape, &mut out)
    }
    out
}
#[test]
fn dreamcast_ipbin_panel_has_explicit_read_only_flow_and_disabled_apply() {
    let ctx = egui::Context::default();
    let game = game();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1600.0, 2000.0),
        )),
        ..Default::default()
    };
    let output = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| show(ui, &game));
    });
    let text = text(&output.shapes);
    assert!(text.contains("Inspect / refresh IP.BIN"));
    assert!(text.contains("Preview DCP contents & compatibility"));
    assert!(text.contains("Apply unavailable"));
    assert!(text.contains("Original images and launch settings stay untouched"));
}
#[test]
fn dreamcast_ipbin_other_platform_is_not_intercepted() {
    let mut game = game();
    game.platform = "Saturn".into();
    assert!(!applies(&game));
}

#[test]
fn dreamcast_ipbin_renders_actual_boot_values_and_contradictory_identity() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("synthetic.iso");
    let mut bytes = vec![0; 32768];
    bytes[..256].fill(b' ');
    for (offset, value) in [
        (0, b"SEGA SEGAKATANA ".as_slice()),
        (16, b"SEGA ENTERPRISES"),
        (32, b"0000 GD-ROM1/1  "),
        (48, b"JUE     "),
        (56, b"0000010 "),
        (64, b"T-1234M   "),
        (74, b"V1.000"),
        (80, b"20000229"),
        (96, b"1ST_READ.BIN"),
        (128, b"SYNTHETIC TITLE"),
    ] {
        bytes[offset..offset + value.len()].copy_from_slice(value);
    }
    std::fs::write(&path, &bytes).unwrap();
    let mut image = inspect_image(&path).unwrap();
    let meta = &mut image.inspection.ip_bin.as_mut().unwrap().metadata;
    add_stronger_evidence_conflicts(meta, Some("OTHER"), None);
    assert!(meta.conflicts.iter().any(|s| s.contains("conflicts")));
    let ctx = egui::Context::default();
    let game = game();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1600.0, 2000.0),
        )),
        ..Default::default()
    };
    let output = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| render_image(ui, &image, &game));
    });
    let text = text(&output.shapes);
    for expected in [
        "T-1234M",
        "JUE",
        "1ST_READ.BIN",
        "SYNTHETIC TITLE",
        "conflicts with stronger disc evidence OTHER",
        "not complete track contents",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn dreamcast_ipbin_worker_reports_unsupported_media_without_a_patch_or_apply() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("synthetic.chd");
    std::fs::write(&path, b"not a real CHD").unwrap();
    let state = SharedPanel::default();
    start(state.clone(), egui::Context::default(), path, None);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        {
            let state = state.lock().unwrap();
            if !state.busy {
                assert!(state.error.as_ref().unwrap().contains("unsupported"));
                assert!(state.image.is_none());
                assert!(state.patch.is_none());
                break;
            }
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn dreamcast_ipbin_is_reachable_from_actual_mods_cheats_route() {
    let ctx = egui::Context::default();
    let mut native = super::super::native_workflows::NativeWorkflows::new(ctx.clone());
    let mut activity = super::super::activity::Activity::default();
    let game = game();
    let output = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            assert!(native.show_cheats(ui, Some(&game), &mut activity).is_none());
        });
    });
    assert!(text(&output.shapes).contains("Dreamcast IP.BIN & DCP inspection"));
}

#[test]
fn dreamcast_ipbin_editing_package_path_clears_previous_snapshot() {
    use archivefs_core::dreamcast_patch_readiness as dcp;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("synthetic.ips");
    std::fs::write(&path, b"PATCHEOF").unwrap();
    let package = dcp::unsupported_dreamcast_patch_package(&path).unwrap();
    let patch =
        dcp::assess_dreamcast_patch_readiness(&package, &Default::default(), &Default::default());
    let state = Arc::new(Mutex::new(Panel {
        patch: Some(patch),
        patch_path: "old.dcp".into(),
        ..Default::default()
    }));
    let ctx = egui::Context::default();
    let game = game();
    ctx.data_mut(|data| {
        data.insert_temp(
            egui::Id::new(("dreamcast_read_only", &game.archive.absolute_path)),
            state.clone(),
        )
    });
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| show(ui, &game));
    });
    ctx.memory_mut(|m| {
        m.request_focus(egui::Id::new((
            "dc_patch_path",
            &game.archive.absolute_path,
        )))
    });
    let input = egui::RawInput {
        events: vec![egui::Event::Text("new".into())],
        ..Default::default()
    };
    let _ = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| show(ui, &game));
    });
    let state = state.lock().unwrap();
    assert!(state.patch_path.contains("new"));
    assert!(state.patch.is_none());
}
