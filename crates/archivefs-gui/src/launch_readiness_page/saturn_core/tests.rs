use super::*;
use archivefs_core::emulator_environment::{
    HostReadOnlyFilesystem,
    retroarch::{DiscoveryEnvironment, discover_retroarch_environment},
};

fn fixture() -> (tempfile::TempDir, RetroArchEnvironmentReport) {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    for d in ["bin", "cores", "info", "config/retroarch"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::copy("/bin/true", root.join("bin/retroarch")).unwrap();
    std::fs::write(root.join("cores/yabause_libretro.so"), b"synthetic").unwrap();
    std::fs::write(root.join("info/yabause_libretro.info"),"display_name = \"Sega - Saturn (Yabause)\"\nsystemname = \"Saturn\"\nsupported_extensions = \"cue|chd\"\n").unwrap();
    std::fs::write(
        root.join("config/retroarch/retroarch.cfg"),
        format!(
            "libretro_directory = \"{}\"\nlibretro_info_path = \"{}\"\n",
            root.join("cores").display(),
            root.join("info").display()
        ),
    )
    .unwrap();
    let e = DiscoveryEnvironment {
        home: Some(root.as_os_str().into()),
        xdg_config_home: Some(root.join("config").into()),
        path: Some(root.join("bin").into()),
        explicit_executables: vec![root.join("bin/retroarch")],
        user_flatpak_root: root.join("uf"),
        system_flatpak_root: root.join("sf"),
        app_image_search_roots: vec![],
        desktop_file_roots: vec![],
    };
    let report = discover_retroarch_environment(&HostReadOnlyFilesystem, &e).unwrap();
    (t, report)
}

#[test]
fn preference_round_trips_and_invalid_or_linked_files_fail_closed() {
    let (temp, report) = fixture();
    let p = &report.profiles[0];
    let selected = SaturnCoreSelection::capture(p, &p.cores[0]).unwrap();
    let path = temp.path().join("emuwiz/saturn_core_selection.json");
    assert!(load_at(&path).unwrap().is_none());
    save_at(&path, &selected).unwrap();
    assert_eq!(load_at(&path).unwrap(), Some(selected.clone()));
    let target = temp.path().join("do-not-change");
    std::fs::write(&target, b"keep").unwrap();
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(load_at(&path).is_err());
    assert!(save_at(&path, &selected).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, b"invalid json").unwrap();
    assert!(load_at(&path).is_err());
}

fn text(shapes: &[egui::epaint::ClippedShape]) -> String {
    fn collect(shape: &egui::epaint::Shape, out: &mut String) {
        match shape {
            egui::epaint::Shape::Text(t) => out.push_str(&t.galley.job.text),
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
        collect(&s.shape, &mut out);
    }
    out
}

#[test]
fn chooser_shows_real_identity_availability_and_never_selects_by_rendering() {
    let (temp, report) = fixture();
    let path = temp.path().join("selection.json");
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| show_at(ui, &report, &path));
    });
    let rendered = text(&output.shapes);
    assert!(rendered.contains("Sega - Saturn (Yabause)"));
    assert!(rendered.contains("Available"));
    assert!(rendered.contains("No Saturn core selected"));
    assert!(!path.exists());
    let p = &report.profiles[0];
    save_at(
        &path,
        &SaturnCoreSelection::capture(p, &p.cores[0]).unwrap(),
    )
    .unwrap();
    std::fs::remove_file(temp.path().join("cores/yabause_libretro.so")).unwrap();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| show_at(ui, &report, &path));
    });
    assert!(!text(&output.shapes).contains("Available — native core discovered"));
}

#[test]
fn chosen_saturn_core_bios_blocker_is_not_hidden_by_other_cores() {
    use super::super::{
        GamerBlockerKind, GamerPlayAction, LaunchReadinessInput, gamer_play_action,
    };
    use archivefs_core::launch::*;
    let (_temp, report) = fixture();
    let p = &report.profiles[0];
    let selected = SaturnCoreSelection::capture(p, &p.cores[0]).unwrap();
    let content = LaunchContentRef {
        kind: None,
        container: Some(LaunchContainerKind::PlainFile),
        resolved_path: Some(std::path::PathBuf::from("/test.cue")),
        requires_mount: false,
        provenance: "test".into(),
    };
    let mut plan = build_launch_plan(
        &CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Saturn".into(),
            game_key: "TEST".into(),
        }),
        &content,
        &[],
        &report,
        &[],
    );
    let mut alternative = plan.candidates[0].clone();
    if let LaunchTarget::RetroArchCore { core_stem, .. } = &mut alternative.target {
        *core_stem = "another_saturn_core".into();
    }
    alternative.blockers.push(LaunchBlocker::new(
        LaunchBlockerKind::AmbiguousCore,
        "multiple installed cores",
    ));
    plan.candidates[0].blockers.push(LaunchBlocker::new(
        LaunchBlockerKind::AmbiguousCore,
        "multiple installed cores",
    ));
    alternative.warnings.push(LaunchWarning {
        kind: LaunchWarningKind::MultipleEligibleProfiles,
        detail: "multiple profiles".into(),
    });
    plan.candidates.insert(0, alternative);
    plan.candidates[1].blockers.push(LaunchBlocker::new(
        LaunchBlockerKind::RequiredFirmwareMissing,
        "selected BIOS missing",
    ));
    saturn_core_selection::apply(&mut plan, &report, Ok(Some(&selected)));
    let input = LaunchReadinessInput::Plan {
        plan,
        retroarch: None,
        retroarch_scanned: true,
        standalone_scans_complete: true,
        dolphin: None,
        pcsx2: None,
        duckstation: None,
        ppsspp: None,
        rpcs3: None,
        xemu: None,
        xenia: None,
        melonds: None,
        cemu: None,
        flycast: None,
        xroar: None,
        vice: None,
        amiga_whdload: None,
        #[cfg(target_os = "linux")]
        shadps4: None,
    };
    let GamerPlayAction::BlockedTyped(blocker) = gamer_play_action(&input) else {
        panic!("BIOS must block Play");
    };
    assert_eq!(blocker.kind, GamerBlockerKind::EmulatorSetupIncomplete);
    assert!(blocker.detail.contains("selected BIOS missing"));
}
