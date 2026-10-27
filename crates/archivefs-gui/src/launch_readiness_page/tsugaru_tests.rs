use super::*;
use archivefs_core::launch::planning::ResolvedIdentity;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};
struct Fixture {
    root: tempfile::TempDir,
    path: PathBuf,
    roots: TsugaruProfileDiscoveryRoots,
    identity: CanonicalIdentityStatus,
    profile: String,
}
fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let exe = root.path().join("Tsugaru_CUI");
    std::fs::write(
        &exe,
        b"#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.argv\"\necho 'synthetic failure' >&2\nexit 7\n",
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let rom = root.path().join("roms");
    std::fs::create_dir(&rom).unwrap();
    for (name, length) in [
        ("FMT_SYS.ROM", 262144),
        ("FMT_DOS.ROM", 524288),
        ("FMT_FNT.ROM", 262144),
        ("FMT_DIC.ROM", 524288),
    ] {
        std::fs::write(rom.join(name), vec![0u8; length]).unwrap();
    }
    let path = root.path().join("disc with spaces; literal.iso");
    std::fs::write(&path, b"synthetic ISO fixture, no commercial content").unwrap();
    let profile = format!("tsugaru:{}", exe.display());
    Fixture {
        root,
        path,
        roots: TsugaruProfileDiscoveryRoots {
            explicit_profiles: vec![(
                exe,
                rom,
                archivefs_core::patch_manager::TsugaruFirmwareState::PresentUnverified,
            )],
            ..Default::default()
        },
        identity: CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "FM Towns".into(),
            game_key: "synthetic".into(),
        }),
        profile,
    }
}
fn request(f: &Fixture) -> Request {
    Request::inspect(&f.path, &f.identity, &f.roots, &f.profile).unwrap()
}
#[test]
fn iso_launch_uses_core_argv_and_watched_nonzero_exit_without_modifying_sources() {
    let f = fixture();
    let r = request(&f);
    let before: Vec<_> = r
        .firmware
        .iter()
        .map(|b| (b.path.clone(), std::fs::read(&b.path).unwrap()))
        .collect();
    let mut process = r.execute_verified(&f.identity).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while process.poll().is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let report = process.poll().unwrap();
    assert_eq!(report.status.as_ref().unwrap().code(), Some(7));
    let argv = std::fs::read_to_string(f.root.path().join("Tsugaru_CUI.argv")).unwrap();
    assert!(argv.contains("-DONTAUTOSAVECMOS\n-CD\n"));
    assert!(argv.contains(f.path.to_str().unwrap()));
    for (path, bytes) in before {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}
#[test]
fn replaced_iso_executable_firmware_and_identity_fail_closed() {
    for target in ["iso", "exe", "firmware", "identity"] {
        let f = fixture();
        let r = request(&f);
        match target {
            "iso" => std::fs::write(&f.path, b"changed").unwrap(),
            "exe" => std::fs::write(&r.executable.path, b"#!/bin/sh\nexit 0\n").unwrap(),
            "firmware" => std::fs::remove_file(&r.firmware[0].path).unwrap(),
            _ => {}
        }
        let current = if target == "identity" {
            CanonicalIdentityStatus::Unknown
        } else {
            f.identity.clone()
        };
        assert!(r.execute_verified(&current).is_err(), "{target}");
        assert!(!f.root.path().join("Tsugaru_CUI.argv").exists());
    }
}
#[test]
fn missing_firmware_empty_directory_unsupported_media_and_links_refuse() {
    let f = fixture();
    let cue = f.path.with_extension("cue");
    std::fs::write(&cue, b"FILE \"missing.bin\" BINARY").unwrap();
    assert!(
        Request::inspect(&cue, &f.identity, &f.roots, &f.profile)
            .unwrap_err()
            .contains("direct ISO")
    );
    let linked = f.root.path().join("linked.iso");
    std::os::unix::fs::symlink(&f.path, &linked).unwrap();
    assert!(Request::inspect(&linked, &f.identity, &f.roots, &f.profile).is_err());
    std::fs::remove_file(f.roots.explicit_profiles[0].1.join("FMT_DOS.ROM")).unwrap();
    assert!(
        Request::inspect(&f.path, &f.identity, &f.roots, &f.profile)
            .unwrap_err()
            .contains("FMT_DOS.ROM")
    );
}
#[test]
fn combined_rom_override_added_after_review_is_refused() {
    let f = fixture();
    let r = request(&f);
    std::fs::write(
        r.core.expected_rom_directory.join("FMT_ALL.ROM"),
        b"override",
    )
    .unwrap();
    assert!(r.execute_verified(&f.identity).is_err());
}

#[test]
fn blue_watches_the_process_to_exit_and_keeps_its_report() {
    let f = fixture();
    let process = request(&f).execute_verified(&f.identity).unwrap();
    let mut state = StandaloneLaunchState::default();
    state.tracked = Some((
        f.path.clone(),
        "tsugaru".into(),
        super::super::StandaloneLaunchStage::Running(super::super::StandaloneProcess::Tsugaru(
            process,
        )),
    ));
    assert!(state.is_active());
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.is_active() {
        state.poll();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    match state.tracked.as_mut().unwrap().2 {
        super::super::StandaloneLaunchStage::Exited(super::super::StandaloneProcess::Tsugaru(
            ref mut process,
        )) => assert_eq!(
            process.poll().unwrap().status.as_ref().unwrap().code(),
            Some(7)
        ),
        _ => panic!("expected a retained watched Tsugaru exit"),
    }
}

#[test]
fn reviewed_action_fits_small_viewport_and_opening_does_not_launch() {
    let f = fixture();
    let request = request(&f);
    let mut plan = LaunchPlan {
        platform_id: Some("FM Towns".into()),
        game_key: Some("synthetic".into()),
        candidates: Vec::new(),
        summary: Default::default(),
        media_topology: None,
    };
    // Supply the already-discovered native candidate just as the controller does.
    plan.candidates
        .push(archivefs_core::launch::LaunchCandidate {
            target: super::super::LaunchTarget::Standalone {
                adapter_id: "tsugaru",
                profile_id: f.profile.clone(),
                profile_path: Some(request.core.expected_executable.clone()),
            },
            content: archivefs_core::launch::LaunchContentRef {
                kind: Some(archivefs_core::launch::LaunchContentKind::OpticalDisc),
                container: Some(archivefs_core::launch::LaunchContainerKind::PlainFile),
                resolved_path: Some(f.path.clone()),
                requires_mount: false,
                provenance: "synthetic".into(),
            },
            firmware: archivefs_core::launch::FirmwareReadiness::PresentUnverified,
            blockers: Vec::new(),
            warnings: Vec::new(),
            readiness: archivefs_core::launch::LaunchReadiness::ReadyWithWarnings,
            preference: archivefs_core::launch::CandidatePreference::SoleEligible,
        });
    let mut state = StandaloneLaunchState::default();
    state.tsugaru = Panel {
        key: Some((Some(f.path.clone()), f.identity.clone())),
        results: vec![(f.profile.clone(), Ok(request))],
        chosen: Some(f.profile.clone()),
        ..Default::default()
    };
    let context = egui::Context::default();
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024., 600.),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, &plan, &mut state));
        },
    );
    fn action(shape: &egui::Shape) -> Option<egui::Rect> {
        match shape {
            egui::Shape::Text(t) if t.galley.text() == "Launch tsugaru" => {
                Some(egui::Rect::from_min_size(t.pos, t.galley.size()))
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(action),
            _ => None,
        }
    }
    let rect = output
        .shapes
        .iter()
        .find_map(|s| action(&s.shape))
        .expect("launch action visible");
    assert!(rect.right() <= 1024. && rect.bottom() <= 600.);
    assert!(!state.is_active());
    assert!(!f.root.path().join("Tsugaru_CUI.argv").exists());
    let (_tx, rx) = mpsc::channel();
    state.tracked = Some((
        PathBuf::from("/different/game"),
        "other".into(),
        super::super::StandaloneLaunchStage::Starting(rx),
    ));
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024., 600.),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, &plan, &mut state));
        },
    );
    assert!(output.shapes.iter().all(|s| action(&s.shape).is_none()));
    assert_eq!(state.tracked.as_ref().unwrap().1, "other");
}

#[test]
fn production_identity_producer_rechecks_actual_dat_before_mock_launch() {
    let mut f = fixture();
    let dat = f.root.path().join("registered.dat");
    super::super::native_cd_identity::tests::dat(&dat, &f.path, "Fujitsu - FM Towns");
    f.identity =
        super::super::native_cd_identity::verify_from_paths(&f.path, &[dat.clone()]).unwrap();
    let request = request(&f);
    let mut process = request
        .clone()
        .execute_with_verifier(|path| {
            super::super::native_cd_identity::verify_from_paths(path, &[dat.clone()])
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while process.poll().is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    std::fs::write(
        &dat,
        b"<datafile><header><name>Fujitsu - FM Towns</name></header></datafile>",
    )
    .unwrap();
    assert!(
        request
            .execute_with_verifier(|path| super::super::native_cd_identity::verify_from_paths(
                path,
                &[dat]
            ))
            .is_err()
    );
}

#[test]
fn complete_blue_launch_uses_private_registry_and_real_production_entrypoint() {
    const MARKER: &str = "EMUWIZ_TSUGARU_SYNTHETIC_CHILD";
    if let Some(path) = std::env::var_os(MARKER) {
        let path = PathBuf::from(path);
        let roots = TsugaruProfileDiscoveryRoots::from_environment();
        let profile = discover_tsugaru_profiles(&roots)
            .profiles
            .into_iter()
            .next()
            .unwrap();
        let config_path = archivefs_core::dat::sources::default_dat_sources_config_path().unwrap();
        assert!(config_path.starts_with(PathBuf::from(std::env::var_os("HOME").unwrap())));
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        let dat = path.parent().unwrap().join("registered.dat");
        std::fs::write(&config_path,format!("[[sources]]\nid='fixture'\ndisplay_name='Synthetic optical catalogue'\npath='{}'\nkind='file'\nenabled=true\n",dat.display())).unwrap();
        let config = archivefs_core::dat::sources::load_dat_sources_config_default().unwrap();
        let (registry, problems) =
            archivefs_core::dat::sources::DatSourceRegistry::from_config(&config);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            registry.sorted_enabled().len(),
            1,
            "private registry must have one source"
        );
        let identity = super::super::native_cd_identity::verify(&path).unwrap();
        let request = Request::inspect(&path, &identity, &roots, &profile.profile_id).unwrap();
        let mut state = StandaloneLaunchState::default();
        state.start(StandaloneLaunchRequest::Tsugaru(request));
        let deadline = Instant::now() + Duration::from_secs(5);
        while state.is_active() {
            state.poll();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        match state.tracked.as_mut().unwrap().2 {
            super::super::StandaloneLaunchStage::Exited(
                super::super::StandaloneProcess::Tsugaru(ref mut process),
            ) => assert_eq!(
                process.poll().unwrap().status.as_ref().unwrap().code(),
                Some(7)
            ),
            _ => panic!("production launch failed: {:?}", state.failure_detail()),
        }
        return;
    }
    let f = fixture();
    let dat = f.root.path().join("registered.dat");
    super::super::native_cd_identity::tests::dat(&dat, &f.path, "Fujitsu - FM Towns");
    let config = f.root.path().join("private-config");
    std::fs::create_dir(&config).unwrap();
    std::fs::write(config.join("dat_sources.toml"),format!("[[sources]]\nid='fixture'\ndisplay_name='Synthetic optical catalogue'\npath='{}'\nkind='file'\nenabled=true\n",dat.display())).unwrap();
    let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","launch_readiness_page::tsugaru::tests::complete_blue_launch_uses_private_registry_and_real_production_entrypoint","--nocapture"])
        .env(MARKER,&f.path).env("HOME",f.root.path()).env("XDG_CONFIG_HOME",f.root.path().join("xdg-config")).env("XDG_DATA_HOME",f.root.path().join("xdg-data")).env("TMPDIR",f.root.path())
        .env("EMUWIZ_CONFIG_HOME",&config).env("EMUWIZ_DATA_HOME",f.root.path().join("data")).env("PATH",f.root.path()).env("EMUWIZ_TSUGARU_ROM_DIRECTORY",&f.roots.explicit_profiles[0].1).output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn returning_to_game_retains_process_feedback_without_a_reviewed_request() {
    let f = fixture();
    let process = request(&f).execute_verified(&f.identity).unwrap();
    let mut state = StandaloneLaunchState::default();
    state.focus_dosbox_game(Some(&f.path));
    state.tracked = Some((
        f.path.clone(),
        "tsugaru".into(),
        super::super::StandaloneLaunchStage::Running(super::super::StandaloneProcess::Tsugaru(
            process,
        )),
    ));
    let plan = LaunchPlan {
        platform_id: Some("FM Towns".into()),
        game_key: Some("synthetic".into()),
        candidates: Vec::new(),
        summary: Default::default(),
        media_topology: None,
    };
    fn text(shape: &egui::Shape) -> String {
        match shape {
            egui::Shape::Text(t) => t.galley.text().into(),
            egui::Shape::Vec(v) => v.iter().map(text).collect::<Vec<_>>().join(" "),
            _ => String::new(),
        }
    }
    let context = egui::Context::default();
    let output = context.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| show(ui, &plan, &mut state));
    });
    assert!(
        output
            .shapes
            .iter()
            .map(|s| text(&s.shape))
            .collect::<Vec<_>>()
            .join(" ")
            .contains("Tsugaru running · PID")
    );
    assert!(state.tsugaru.results.is_empty());
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.is_active() {
        state.poll();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = context.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| show(ui, &plan, &mut state));
    });
    let rendered = output
        .shapes
        .iter()
        .map(|s| text(&s.shape))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(rendered.contains("Process exited"));
    assert!(rendered.contains('7'));
}
