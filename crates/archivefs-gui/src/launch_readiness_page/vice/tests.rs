use super::*;
use crate::gui_v2::vice_evidence::tests::Lab;
use archivefs_core::emulator_environment::retroarch::RetroArchEnvironmentReport;
use archivefs_core::launch::build_launch_plan;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};

struct Fixture {
    lab: Lab,
    context: Context,
}
fn fixture(exit: i32) -> Fixture {
    let lab = Lab::new();
    let exe = lab.path.with_file_name("x64sc");
    let marker = lab.path.with_file_name("spawned");
    fs::write(
        &exe,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit {exit}\n",
            marker.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    let digest = crate::gui_v2::vice_evidence::hex(&sha2::Sha256::digest(fs::read(&exe).unwrap()));
    let roots = ViceProfileDiscoveryRoots {
        explicit_executables: vec![exe],
        path_env: None,
        ..Default::default()
    };
    let context =
        Context::inspect_with(lab.path.clone(), lab.evidence().unwrap(), roots, &digest).unwrap();
    assert_eq!(context.profiles.len(), 1);
    Fixture { lab, context }
}
fn input(context: Context) -> LaunchReadinessInput {
    let (identity, content) = context.inputs(&CanonicalIdentityStatus::Unknown).unwrap();
    let environment = RetroArchEnvironmentReport {
        format_version: 2,
        profiles: vec![],
        diagnostics: vec![],
    };
    let mut plan = build_launch_plan(
        &identity,
        &content,
        &context.profiles().collect::<Vec<_>>(),
        &environment,
        &[],
    );
    context.apply(&mut plan);
    LaunchReadinessInput::Plan {
        plan,
        retroarch: None,
        retroarch_scanned: false,
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
        vice: Some(context),
        amiga_whdload: None,
        #[cfg(target_os = "linux")]
        shadps4: None,
    }
}
fn request(f: &Fixture) -> Request {
    match gamer_play_action(&input(f.context.clone())) {
        GamerPlayAction::Launch(r) => match *r {
            TypedLaunchRequest::Standalone(StandaloneLaunchRequest::Vice(r)) => r,
            _ => panic!("expected VICE request"),
        },
        _ => panic!("expected VICE Play"),
    }
}

#[test]
fn exact_dat_through_real_planner_offers_explicit_play_and_tracks_process_exit() {
    let f = fixture(0);
    let req = request(&f);
    let data = input(f.context.clone());
    let LaunchReadinessInput::Plan { plan, .. } = &data else {
        unreachable!()
    };
    let candidate = plan
        .candidates
        .iter()
        .find(|c| {
            matches!(
                c.target,
                LaunchTarget::Standalone {
                    adapter_id: "vice",
                    ..
                }
            )
        })
        .unwrap();
    let profile = discover_vice_profiles(&f.context.roots).profiles.remove(0);
    let planned = build_vice_command_plan(
        &identity(&req.evidence),
        candidate,
        &resolve_vice_native_launch_binding(&profile),
    )
    .command
    .unwrap();
    let checked = preflight_vice_launch(&req.launch, &req.roots, &identity(&req.evidence)).unwrap();
    assert_eq!(planned.executable, checked.executable);
    assert_eq!(planned.arguments, checked.arguments);
    assert_eq!(
        checked.arguments,
        vec![
            std::ffi::OsString::from("+saveres"),
            "+cart".into(),
            "-cartcrt".into(),
            f.lab.path.clone().into_os_string()
        ]
    );
    assert!(!checked.arguments.iter().any(|a| a == "-moncommands"));
    let before = fs::read(&f.lab.path).unwrap();
    let process = req
        .execute_checked(|path| {
            assert_eq!(path, f.lab.path);
            f.lab.evidence()
        })
        .unwrap();
    let mut state = StandaloneLaunchState::default();
    state.tracked = Some((
        f.lab.path.clone(),
        "vice".into(),
        StandaloneLaunchStage::Running(StandaloneProcess::Vice(process)),
    ));
    let deadline = Instant::now() + Duration::from_secs(3);
    while state.is_active() && Instant::now() < deadline {
        state.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    let Some((_, _, StandaloneLaunchStage::Exited(StandaloneProcess::Vice(mut process)))) =
        state.tracked
    else {
        panic!("expected tracked VICE exit");
    };
    assert_eq!(
        exit_line("VICE", process.poll().unwrap()),
        "VICE exited normally; this does not prove the game ran."
    );
    assert_eq!(fs::read(&f.lab.path).unwrap(), before);
    assert_eq!(
        fs::read_to_string(f.lab.path.with_file_name("spawned")).unwrap(),
        format!("+saveres\n+cart\n-cartcrt\n{}\n", f.lab.path.display())
    );
}

#[test]
fn play_reloads_sources_and_refuses_removed_dat_without_starting_process() {
    let f = fixture(0);
    let req = request(&f);
    fs::remove_file(&f.lab.dat).unwrap();
    assert!(req.execute_checked(|_| f.lab.evidence()).is_err());
    assert!(!f.lab.path.with_file_name("spawned").exists());
}

#[test]
fn a_lost_execute_bit_is_refused_by_gold_preflight() {
    let f = fixture(0);
    let req = request(&f);
    fs::set_permissions(
        &req.launch.expected_executable,
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(req.execute_checked(|_| f.lab.evidence()).is_err());
    assert!(!f.lab.path.with_file_name("spawned").exists());
}

#[test]
fn stale_dat_cartridge_or_executable_cannot_spawn() {
    let f = fixture(0);
    let req = request(&f);
    let mut fresh = f.lab.evidence().unwrap();
    fresh.catalogues[0].1 = Some("changed".into());
    assert!(
        req.clone()
            .execute_with(fresh)
            .err()
            .unwrap()
            .contains("evidence changed")
    );
    fs::write(&req.launch.expected_executable, b"#!/bin/sh\nexit 99\n").unwrap();
    assert!(
        req.clone()
            .execute_with(f.lab.evidence().unwrap())
            .err()
            .unwrap()
            .contains("executable changed")
    );
    fs::remove_file(&f.lab.path).unwrap();
    assert!(
        req.clone()
            .execute_with(req.evidence.clone())
            .err()
            .unwrap()
            .contains("executable changed")
    );

    let f = fixture(0);
    let req = request(&f);
    fs::remove_file(&f.lab.path).unwrap();
    assert!(
        req.clone()
            .execute_with(req.evidence.clone())
            .err()
            .unwrap()
            .contains("not started")
    );
}

#[test]
fn wrong_identity_or_candidate_binding_never_offers_a_request() {
    let f = fixture(0);
    assert!(
        f.context
            .inputs(&CanonicalIdentityStatus::Conflicting)
            .is_none()
    );
    assert!(
        f.context
            .inputs(&CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: "VIC-20".into(),
                game_key: "other".into()
            }))
            .is_none()
    );
    let LaunchReadinessInput::Plan { mut plan, .. } = input(f.context.clone()) else {
        unreachable!()
    };
    let candidate = plan
        .candidates
        .iter()
        .find(|c| {
            matches!(
                c.target,
                LaunchTarget::Standalone {
                    adapter_id: "vice",
                    ..
                }
            )
        })
        .unwrap()
        .clone();
    let mut blocked = candidate.clone();
    blocked.readiness = LaunchReadiness::Blocked;
    assert!(f.context.request(&plan, &blocked).is_none());
    let mut mounted = candidate.clone();
    mounted.content.requires_mount = true;
    assert!(f.context.request(&plan, &mounted).is_none());
    let mut wrong_path = candidate.clone();
    wrong_path.content.resolved_path = Some(PathBuf::from("/other.crt"));
    assert!(f.context.request(&plan, &wrong_path).is_none());
    plan.game_key = Some("other".into());
    assert!(f.context.request(&plan, &candidate).is_none());
}

#[test]
fn unverified_binary_or_x64_never_becomes_a_play_candidate() {
    let f = fixture(0);
    let context = Context::inspect_with(
        f.lab.path.clone(),
        f.lab.evidence().unwrap(),
        f.context.roots.clone(),
        VERIFIED_X64SC_SHA256,
    )
    .unwrap();
    assert!(context.profiles.is_empty());
    let exe = f.lab.path.with_file_name("x64");
    fs::copy(&f.context.roots.explicit_executables[0], &exe).unwrap();
    let roots = ViceProfileDiscoveryRoots {
        explicit_executables: vec![exe],
        path_env: None,
        ..Default::default()
    };
    let context = Context::inspect_with(
        f.lab.path.clone(),
        f.lab.evidence().unwrap(),
        roots,
        &f.context.profiles[0].1.executable_sha256,
    )
    .unwrap();
    assert!(context.profiles.is_empty());
}

#[test]
fn preview_shows_exact_argv_without_starting_a_process() {
    let f = fixture(0);
    let LaunchReadinessInput::Plan { plan, .. } = input(f.context.clone()) else {
        unreachable!()
    };
    let candidate = plan
        .candidates
        .iter()
        .find(|c| {
            matches!(
                c.target,
                LaunchTarget::Standalone {
                    adapter_id: "vice",
                    ..
                }
            )
        })
        .unwrap();
    let mut state = StandaloneLaunchState::default();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default()
            .show(ctx, |ui| f.context.show(ui, &plan, candidate, &mut state));
    });
    let texts = output
        .shapes
        .iter()
        .map(|s| format!("{:?}", s.shape))
        .collect::<String>();
    assert!(texts.contains("+saveres"));
    assert!(texts.contains("-cartcrt"));
    assert!(texts.contains("Play in VICE"));
    assert!(state.tracked.is_none());
    fn center(shape: &egui::epaint::Shape) -> Option<egui::Pos2> {
        match shape {
            egui::epaint::Shape::Text(t) if t.galley.text() == "Play in VICE" => {
                Some(t.pos + t.galley.size() / 2.0)
            }
            egui::epaint::Shape::Vec(shapes) => shapes.iter().find_map(center),
            _ => None,
        }
    }
    let button = output.shapes.iter().find_map(|s| center(&s.shape)).unwrap();
    for pressed in [true, false] {
        let _ = ctx.run(
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(button),
                    egui::Event::PointerButton {
                        pos: button,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| f.context.show(ui, &plan, candidate, &mut state));
            },
        );
    }
    assert!(
        state.tracked.is_some(),
        "an explicit click must reach the existing launch worker"
    );
    // The process-wide HOME is private in this suite, with no production DB.
    // The real executor must re-read it and refuse, rather than trust preview.
    let deadline = Instant::now() + Duration::from_secs(3);
    while state.is_active() && Instant::now() < deadline {
        state.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(matches!(
        state.tracked,
        Some((_, _, StandaloneLaunchStage::Failed(_)))
    ));
    assert!(!f.lab.path.with_file_name("spawned").exists());
}

#[test]
fn late_worker_results_do_not_cross_selections() {
    let f = fixture(0);
    let (tx, rx) = mpsc::channel();
    tx.send(Ok(f.context)).unwrap();
    let mut controller = Controller {
        path: Some(f.lab.path.clone()),
        receiver: Some((f.lab.path.clone(), rx)),
        ..Default::default()
    };
    let ctx = egui::Context::default();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default()
            .show(ctx, |ui| controller.show(ui, Some(Path::new("/other.iso"))));
    });
    assert!(controller.context.is_none());
    assert!(controller.receiver.is_none());
}

#[test]
fn removed_executable_does_not_hide_historical_exit_reporting() {
    let f = fixture(0);
    let req = request(&f);
    let executable = req.launch.expected_executable.clone();
    let mut process = req.execute_checked(|_| f.lab.evidence()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while process.poll().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(process.poll().is_some());
    fs::remove_file(executable).unwrap();
    let mut state = StandaloneLaunchState::default();
    state.tracked = Some((
        f.lab.path.clone(),
        "vice".into(),
        StandaloneLaunchStage::Exited(StandaloneProcess::Vice(process)),
    ));
    let LaunchReadinessInput::Plan { plan, .. } = input(f.context.clone()) else {
        unreachable!()
    };
    let candidate = plan
        .candidates
        .iter()
        .find(|c| {
            matches!(
                c.target,
                LaunchTarget::Standalone {
                    adapter_id: "vice",
                    ..
                }
            )
        })
        .unwrap();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default()
            .show(ctx, |ui| f.context.show(ui, &plan, candidate, &mut state));
    });
    let texts = output
        .shapes
        .iter()
        .map(|s| format!("{:?}", s.shape))
        .collect::<String>();
    assert!(texts.contains("VICE exited normally; this does not prove the game ran."));
    assert!(texts.contains("VICE command refused; refresh readiness."));
}
