use super::*;
use archivefs_core::launch::{CanonicalIdentityStatus, LaunchPlanSummary};
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};
struct Fixture {
    root: tempfile::TempDir,
    media: PathBuf,
    exe: PathBuf,
    profile: PathBuf,
    identity: CanonicalIdentityStatus,
}
fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let media = root.path().join("synthetic game.iso");
    let exe = root.path().join("amiberry");
    let profile = root.path().join("test.uae");
    std::fs::write(&media, b"synthetic CD32 ISO fixture").unwrap();
    std::fs::write(&exe, b"#!/bin/sh\necho synthetic >&2\nexit 9\n").unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(root.path().join("main.rom"), b"synthetic main").unwrap();
    std::fs::write(root.path().join("extended.rom"), b"synthetic ext").unwrap();
    std::fs::write(&profile,format!("chipset_compatible=CD32\ncdimage0={}\nkickstart_rom_file={}\nkickstart_ext_rom_file={}\n",media.display(),root.path().join("main.rom").display(),root.path().join("extended.rom").display())).unwrap();
    let dat = root.path().join("registered.dat");
    super::super::native_cd_identity::tests::dat(&dat, &media, "Commodore - Amiga CD32");
    let identity = super::super::native_cd_identity::verify_from_paths(&media, &[dat]).unwrap();
    Fixture {
        root,
        media,
        exe,
        profile,
        identity,
    }
}
fn ready(f: &Fixture) -> Request {
    Request {
        reviewed: review_amiberry_cd_profile(&f.exe, &f.profile, &f.media, &f.identity).unwrap(),
    }
}
#[test]
fn missing_setup_has_guidance_and_cannot_create_a_launch_request() {
    let results = inspect(None, CanonicalIdentityStatus::Unknown, None);
    assert!(results.iter().all(|(_, r)| r.is_err()));
    assert!(results[0].1.as_ref().unwrap_err().contains("Configure"));
}
#[test]
fn blue_retains_amiberry_exit_and_preserves_source_profile() {
    let f = fixture();
    let before = std::fs::read(&f.profile).unwrap();
    let process = ready(&f).reviewed.launch(&f.identity).unwrap();
    let mut state = StandaloneLaunchState {
        tracked: Some((
            f.media.clone(),
            "amiberry_cd".into(),
            super::super::StandaloneLaunchStage::Running(
                super::super::StandaloneProcess::AmiberryCd(process),
            ),
        )),
        ..Default::default()
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.is_active() {
        state.poll();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    match state.tracked.as_mut().unwrap().2 {
        super::super::StandaloneLaunchStage::Exited(
            super::super::StandaloneProcess::AmiberryCd(ref mut p),
        ) => assert_eq!(p.poll().unwrap().status.as_ref().unwrap().code(), Some(9)),
        _ => panic!("expected retained Amiberry exit"),
    }
    assert_eq!(std::fs::read(&f.profile).unwrap(), before);
}
#[test]
fn small_viewport_keeps_action_visible_and_blocks_another_active_launch() {
    let f = fixture();
    let request = ready(&f);
    let plan = LaunchPlan {
        platform_id: Some("AmigaCD32".into()),
        game_key: match &f.identity {
            CanonicalIdentityStatus::Resolved(i) => Some(i.game_key.clone()),
            _ => None,
        },
        candidates: Vec::new(),
        summary: LaunchPlanSummary::default(),
        media_topology: None,
    };
    let mut state = StandaloneLaunchState {
        amiberry_cd: Panel {
            focused: Some(f.media.clone()),
            key: Some((Some(f.media.clone()), f.identity.clone())),
            results: (0..12)
                .map(|i| (format!("configured {i}"), Ok(request.clone())))
                .collect(),
            chosen: Some("configured 0".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let context = egui::Context::default();
    fn action(shape: &egui::Shape) -> Option<egui::Rect> {
        match shape {
            egui::Shape::Text(t) if t.galley.text() == "Launch Amiberry CD" => {
                Some(egui::Rect::from_min_size(t.pos, t.galley.size()))
            }
            egui::Shape::Vec(v) => v.iter().find_map(action),
            _ => None,
        }
    }
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
    let rect = output.shapes.iter().find_map(|s| action(&s.shape)).unwrap();
    assert!(rect.bottom() <= 600. && rect.right() <= 1024.);
    assert!(!state.is_active());
    let (_tx, rx) = mpsc::channel();
    state.tracked = Some((
        PathBuf::from("/another/game"),
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
fn complete_blue_launch_uses_actual_registered_dat_and_configured_profile() {
    const MARKER: &str = "EMUWIZ_AMIBERRY_SYNTHETIC_CHILD";
    if let Some(directory) = std::env::var_os(MARKER) {
        let directory = PathBuf::from(directory);
        let media = directory.join("synthetic game.iso");
        let config = archivefs_core::dat::sources::default_dat_sources_config_path().unwrap();
        assert!(config.starts_with(PathBuf::from(std::env::var_os("HOME").unwrap())));
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config,format!("[[sources]]\nid='fixture'\ndisplay_name='Synthetic'\npath='{}'\nkind='file'\nenabled=true\n",directory.join("registered.dat").display())).unwrap();
        let roots = AmigaProfileDiscoveryRoots {
            home: directory.clone(),
            xdg_config_home: directory.clone(),
            xdg_data_home: directory.join("data"),
            explicit_configuration_roots: vec![(AmigaEmulatorKind::Amiberry, directory.clone())],
            portable_configuration_roots: Vec::new(),
            explicit_executables: vec![(AmigaEmulatorKind::Amiberry, directory.join("amiberry"))],
            known_version_outputs: Default::default(),
        };
        let results = inspect(
            Some(media.clone()),
            CanonicalIdentityStatus::Unknown,
            Some(roots),
        );
        let request = results
            .into_iter()
            .find_map(|(_, r)| r.ok())
            .expect("a real discovered configured profile should be reviewable");
        let mut state = StandaloneLaunchState::default();
        state.start(StandaloneLaunchRequest::AmiberryCd(request));
        let deadline = Instant::now() + Duration::from_secs(5);
        while state.is_active() {
            state.poll();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            matches!(
                state.tracked.as_ref().unwrap().2,
                super::super::StandaloneLaunchStage::Exited(
                    super::super::StandaloneProcess::AmiberryCd(_)
                )
            ),
            "{:?}",
            state.failure_detail()
        );
        return;
    }
    let f = fixture();
    std::fs::copy(&f.profile, f.root.path().join("amiberry.uae")).unwrap();
    let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","launch_readiness_page::amiberry_cd::tests::complete_blue_launch_uses_actual_registered_dat_and_configured_profile","--nocapture"]).env(MARKER,f.root.path()).output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn returning_to_amiberry_game_retains_status_without_reviewed_selection() {
    let f = fixture();
    let process = ready(&f).reviewed.launch(&f.identity).unwrap();
    let mut state = StandaloneLaunchState::default();
    state.focus_dosbox_game(Some(&f.media));
    state.tracked = Some((
        f.media.clone(),
        "amiberry_cd".into(),
        super::super::StandaloneLaunchStage::Running(super::super::StandaloneProcess::AmiberryCd(
            process,
        )),
    ));
    let plan = LaunchPlan {
        platform_id: Some("AmigaCD32".into()),
        game_key: match &f.identity {
            CanonicalIdentityStatus::Resolved(i) => Some(i.game_key.clone()),
            _ => None,
        },
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
            .contains("Amiberry CD running · PID")
    );
    assert!(state.amiberry_cd.results.is_empty());
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
    assert!(rendered.contains('9'));
}

#[test]
fn full_unresolved_page_keeps_native_action_visible_at_1024_by_600() {
    let f = fixture();
    let request = ready(&f);
    let mut state = StandaloneLaunchState::default();
    state.focus_dosbox_game(Some(&f.media));
    state.amiberry_cd = Panel {
        focused: Some(f.media.clone()),
        key: Some((Some(f.media.clone()), CanonicalIdentityStatus::Unknown)),
        results: (0..12)
            .map(|i| (format!("configured {i}"), Ok(request.clone())))
            .collect(),
        chosen: Some("configured 0".into()),
        ..Default::default()
    };
    let mut retroarch = super::super::RetroArchLaunchState::default();
    let mut dolphin = super::super::DolphinLaunchState::default();
    let mut pcsx2 = super::super::Pcsx2LaunchState::default();
    let mut whdload = super::super::AmigaWHDLoadLaunchState::default();
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
            egui::CentralPanel::default().show(ctx, |ui| {
                super::super::show_launch_readiness_panel(
                    ui,
                    &super::super::LaunchReadinessInput::IdentityUnknown,
                    &mut retroarch,
                    &mut dolphin,
                    &mut pcsx2,
                    &mut state,
                    &mut whdload,
                );
            });
        },
    );
    fn action(shape: &egui::Shape) -> Option<egui::Rect> {
        match shape {
            egui::Shape::Text(t) if t.galley.text() == "Launch Amiberry CD" => {
                Some(egui::Rect::from_min_size(t.pos, t.galley.size()))
            }
            egui::Shape::Vec(v) => v.iter().find_map(action),
            _ => None,
        }
    }
    let rect = output
        .shapes
        .iter()
        .find_map(|s| action(&s.shape))
        .expect("configured CD launch action visible on combined optical page");
    assert!(rect.bottom() <= 600. && rect.right() <= 1024., "{rect:?}");
    assert!(!state.is_active());
}
