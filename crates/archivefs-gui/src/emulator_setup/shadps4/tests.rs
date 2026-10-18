use super::*;
use crate::launch_readiness_page::{self as page, StandaloneLaunchRequest, StandaloneProcess};
use archivefs_core::launch::planning::{CanonicalIdentityStatus, ResolvedIdentity};
use std::{
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};
fn host_elf(appimage: bool) -> Vec<u8> {
    // Tiny synthetic x86-64 executable: one load segment, exit(7), CLI marker.
    let mut bytes = vec![0u8; 180];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    if appimage {
        bytes[8..11].copy_from_slice(b"AI\x02");
    }
    bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
    bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
    bytes[24..32].copy_from_slice(&0x400078u64.to_le_bytes());
    bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
    bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
    bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
    bytes[56..58].copy_from_slice(&1u16.to_le_bytes());
    bytes[64..68].copy_from_slice(&1u32.to_le_bytes());
    bytes[68..72].copy_from_slice(&5u32.to_le_bytes());
    bytes[80..88].copy_from_slice(&0x400000u64.to_le_bytes());
    bytes[96..104].copy_from_slice(&180u64.to_le_bytes());
    bytes[104..112].copy_from_slice(&180u64.to_le_bytes());
    bytes[112..120].copy_from_slice(&4096u64.to_le_bytes());
    bytes[120..132].copy_from_slice(&[0xb8, 60, 0, 0, 0, 0xbf, 7, 0, 0, 0, 0x0f, 0x05]);
    bytes[140..160].copy_from_slice(b"shadPS4 Emulator CLI");
    bytes
}
fn ps4_elf() -> Vec<u8> {
    let mut bytes = host_elf(false);
    bytes[7] = 9;
    bytes[16..18].copy_from_slice(&0xfe10u16.to_le_bytes());
    bytes
}
fn sfo(title: &str, content: &str) -> Vec<u8> {
    let pairs = [
        ("TITLE_ID", title),
        ("CONTENT_ID", content),
        ("CATEGORY", "gd"),
    ];
    let mut keys = Vec::new();
    let mut values = Vec::new();
    let mut entries = Vec::new();
    for (key, value) in pairs {
        let k = keys.len() as u16;
        let d = values.len() as u32;
        keys.extend_from_slice(key.as_bytes());
        keys.push(0);
        values.extend_from_slice(value.as_bytes());
        values.push(0);
        let len = value.len() as u32 + 1;
        entries.extend_from_slice(&k.to_le_bytes());
        entries.extend_from_slice(&0x0204u16.to_le_bytes());
        entries.extend_from_slice(&len.to_le_bytes());
        entries.extend_from_slice(&len.to_le_bytes());
        entries.extend_from_slice(&d.to_le_bytes());
    }
    let key_at = 20 + entries.len();
    let data_at = key_at + keys.len();
    let mut bytes = b"\0PSF".to_vec();
    bytes.extend_from_slice(&0x101u32.to_le_bytes());
    bytes.extend_from_slice(&(key_at as u32).to_le_bytes());
    bytes.extend_from_slice(&(data_at as u32).to_le_bytes());
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&entries);
    bytes.extend_from_slice(&keys);
    bytes.extend_from_slice(&values);
    bytes
}

struct Fixture {
    dir: tempfile::TempDir,
    exe: PathBuf,
    game: PathBuf,
    binding: Binding,
    selection: Selection,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("shadPS4");
        fs::write(&exe, host_elf(false)).unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        let game = dir.path().join("game ; $(inert) Ω");
        fs::create_dir_all(game.join("sce_sys")).unwrap();
        fs::write(game.join("eboot.bin"), ps4_elf()).unwrap();
        fs::write(
            game.join("sce_sys/param.sfo"),
            sfo("CUSA00001", "UP0001-CUSA00001_00-LABEL00000000000"),
        )
        .unwrap();
        let data = dir.path().join("data");
        fs::create_dir_all(data.join("shadPS4")).unwrap();
        fs::write(data.join("shadPS4/config.json"), "{}").unwrap();
        let selection_file = dir.path().join("selection.toml");
        let override_file = dir.path().join("override.txt");
        fs::write(&override_file, exe.to_str().unwrap()).unwrap();
        let binding = Binding {
            roots: KnownInstallRoots {
                home: dir.path().into(),
                user_data: data,
                system_data: dir.path().join("system"),
                path_dirs: vec![],
            },
            configured: vec![exe.clone()],
            path: None,
            mode: EmulatorSelectionMode::Auto,
            selection_file: Some(selection_file),
            override_file: Some(override_file),
        };
        let selection = Selection {
            path: game.clone(),
            identity: CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: "PS4".into(),
                game_key: "CUSA00001".into(),
            }),
        };
        Self {
            dir,
            exe,
            game,
            binding,
            selection,
        }
    }
    fn inspection(&self) -> Inspection {
        gather(&self.selection, self.binding.clone())
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            inspection: Some(Arc::new(self.inspection())),
            error: None,
            handle: Arc::new(Mutex::new(Controller {
                selection: Some(self.selection.clone()),
                ..Default::default()
            })),
            selection: self.selection.clone(),
        }
    }
    fn launch_plan(&self, snapshot: &Snapshot) -> launch::LaunchPlan {
        let content = launch::LaunchContentRef {
            kind: None,
            container: None,
            resolved_path: None,
            requires_mount: false,
            provenance: "fixture".into(),
        };
        let mut plan = launch::LaunchPlan {
            platform_id: Some("PS4".into()),
            game_key: Some("CUSA00001".into()),
            candidates: vec![],
            summary: Default::default(),
            media_topology: None,
        };
        snapshot.apply(&mut plan, &content);
        plan
    }
}
#[test]
fn ready_ps4_uses_resolver_profile_and_retains_verified_ids_and_exact_argv() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    let plan = f.launch_plan(&snapshot);
    let request = snapshot.request(&plan.candidates[0]).unwrap();
    assert_eq!(
        plan.candidates[0].readiness,
        launch::LaunchReadiness::ReadyWithWarnings
    );
    assert_eq!(request.plan.identity().game_key, "CUSA00001");
    assert_eq!(
        request.plan.game().content_id(),
        Some("UP0001-CUSA00001_00-LABEL00000000000")
    );
    assert_eq!(request.plan.game().root(), f.game);
    assert_eq!(request.plan.profile().executable, f.exe);
    assert_eq!(
        request.plan.command_preview().arguments,
        vec![
            OsString::from("-g"),
            f.game.join("eboot.bin").into_os_string()
        ]
    );
    assert!(!plan.candidates[0].warnings.is_empty());
    assert!(
        plan.candidates[0]
            .warnings
            .iter()
            .any(|w| w.detail.contains("compatibility") && w.detail.contains("not verified"))
    );
    assert_eq!(
        snapshot
            .inspection
            .as_ref()
            .unwrap()
            .resolution
            .resolved()
            .unwrap()
            .profile
            .executable,
        f.exe
    );
}
#[test]
fn missing_or_unusable_executable_is_a_typed_refusal_not_a_launch_option() {
    for runnable in [false, true] {
        let f = Fixture::new();
        if runnable {
            fs::remove_file(&f.exe).unwrap();
        } else {
            fs::set_permissions(&f.exe, fs::Permissions::from_mode(0o644)).unwrap();
        }
        let snapshot = f.snapshot();
        let plan = f.launch_plan(&snapshot);
        let kind = snapshot
            .inspection
            .as_ref()
            .unwrap()
            .request
            .as_ref()
            .err()
            .unwrap()
            .kind;
        assert_eq!(
            kind,
            if runnable {
                Kind::ExecutableMissing
            } else {
                Kind::ExecutableNotRunnable
            }
        );
        assert_eq!(
            plan.candidates[0].readiness,
            launch::LaunchReadiness::Blocked
        );
        assert!(snapshot.request(&plan.candidates[0]).is_none());
        assert!(
            plan.candidates[0].blockers[0]
                .detail
                .starts_with(&format!("{kind:?}:"))
        );
    }
}
#[test]
fn packages_archives_images_are_refused_even_with_a_valid_installation() {
    for extension in ["pkg", "zip", "iso"] {
        let mut f = Fixture::new();
        f.selection.path = f.dir.path().join(format!("CUSA00001.{extension}"));
        fs::write(&f.selection.path, "fixture").unwrap();
        assert_eq!(
            f.inspection().request.err().unwrap().kind,
            Kind::UnsupportedGameLayout
        );
    }
}
#[test]
fn no_filename_identity_exception_or_mismatched_title_id_is_introduced() {
    let mut f = Fixture::new();
    f.selection.identity = CanonicalIdentityStatus::Unknown;
    assert_eq!(
        f.inspection().request.err().unwrap().kind,
        Kind::IdentityUnverified
    );
    f.selection.identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "PS4".into(),
        game_key: "CUSA00002".into(),
    });
    assert_eq!(
        f.inspection().request.err().unwrap().kind,
        Kind::IdentityUnverified
    );
}
#[test]
fn forced_resolver_profile_uses_the_exact_second_executable_and_persists_mode() {
    let mut f = Fixture::new();
    let second = f.dir.path().join("renamed-core");
    fs::write(&second, host_elf(false)).unwrap();
    fs::set_permissions(&second, fs::Permissions::from_mode(0o755)).unwrap();
    let identity = CandidateIdentity {
        emulator: InventoryEmulator::ShadPs4,
        executable: Some(second.clone()),
        profile_root: f.binding.roots.user_data.join("shadPS4"),
    };
    f.binding.mode = EmulatorSelectionMode::Forced(identity.clone());
    remember_selection_to(
        f.binding.selection_file.as_ref().unwrap(),
        InventoryEmulator::ShadPs4,
        &f.binding.mode,
    )
    .unwrap();
    let inspection = f.inspection();
    let request = inspection.request.unwrap();
    assert_eq!(request.plan.command_preview().executable, second);
    assert_eq!(
        inspection.resolution.resolved().unwrap().profile.identity,
        identity
    );
    request.validate().unwrap();
}
#[test]
fn changed_executable_input_config_or_resolver_mode_refuses_the_preview_request() {
    for what in ["executable", "input", "config", "mode", "override"] {
        let f = Fixture::new();
        let request = f.inspection().request.unwrap();
        match what {
            "executable" => fs::write(&f.exe, b"changed").unwrap(),
            "input" => fs::write(f.game.join("eboot.bin"), b"changed").unwrap(),
            "config" => fs::write(
                f.binding.roots.user_data.join("shadPS4/config.json"),
                "{\"GPU\":{}}",
            )
            .unwrap(),
            "mode" => remember_selection_to(
                f.binding.selection_file.as_ref().unwrap(),
                InventoryEmulator::ShadPs4,
                &EmulatorSelectionMode::Forced(CandidateIdentity {
                    emulator: InventoryEmulator::ShadPs4,
                    executable: Some(f.exe.clone()),
                    profile_root: f.dir.path().join("missing"),
                }),
            )
            .unwrap(),
            "override" => fs::write(
                f.binding.override_file.as_ref().unwrap(),
                "/missing/shadPS4",
            )
            .unwrap(),
            _ => unreachable!(),
        }
        assert!(request.validate().is_err(), "{what}");
    }
}
#[test]
fn ordinary_metadata_changes_invalidate_the_visible_snapshot() {
    let f = Fixture::new();
    let inspection = f.inspection();
    assert!(inspection.metadata_current());
    fs::write(f.game.join("eboot.bin"), b"changed").unwrap();
    assert!(!inspection.metadata_current());
}
#[test]
fn stale_background_generation_or_selection_never_publishes_a_preview() {
    let f = Fixture::new();
    let mut controller = Controller {
        selection: Some(f.selection.clone()),
        generation: 2,
        ..Default::default()
    };
    controller.accept(1, &f.selection, Ok(f.inspection()));
    assert!(controller.inspection.is_none());
    let mut different = f.selection.clone();
    different.path = f.dir.path().join("different");
    controller.accept(2, &different, Ok(f.inspection()));
    assert!(controller.inspection.is_none());
    controller.accept(2, &f.selection, Ok(f.inspection()));
    assert!(controller.inspection.is_some());
}
#[test]
fn resolver_selection_controls_use_existing_atomic_persistence() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    let identity = snapshot
        .inspection
        .as_ref()
        .unwrap()
        .resolution
        .resolved()
        .unwrap()
        .profile
        .identity
        .clone();
    snapshot
        .choose_mode(EmulatorSelectionMode::Preferred(identity.clone()))
        .unwrap();
    assert_eq!(
        load_selection_from(
            f.binding.selection_file.as_ref().unwrap(),
            InventoryEmulator::ShadPs4
        )
        .unwrap(),
        EmulatorSelectionMode::Preferred(identity)
    );
    assert!(snapshot.handle.lock().unwrap().needs_check);
}
#[test]
#[cfg(target_arch = "x86_64")]
fn gui_typed_request_reaches_watched_process_supervision_and_exit() {
    let f = Fixture::new();
    let config = f.binding.roots.user_data.join("shadPS4/config.json");
    let before = fs::read(&config).unwrap();
    let request = StandaloneLaunchRequest::ShadPs4(f.inspection().request.unwrap());
    let StandaloneProcess::ShadPs4(mut process) = request.execute().unwrap() else {
        panic!("wrong process route");
    };
    assert!(process.pid > 0);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(result) = process.poll() {
            assert_eq!(result.status.as_ref().unwrap().code(), Some(7));
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(fs::read(config).unwrap(), before);
}
#[test]
fn gamer_action_carries_the_same_sealed_plan_with_warnings() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    let plan = f.launch_plan(&snapshot);
    let input = page::LaunchReadinessInput::Plan {
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
        cemu: None,
        amiga_whdload: None,
        shadps4: Some(snapshot),
    };
    assert!(
        matches!(page::gamer_play_action(&input), page::GamerPlayAction::Launch(request) if matches!(*request, page::TypedLaunchRequest::Standalone(StandaloneLaunchRequest::ShadPs4(_))))
    );
}
#[test]
fn page_renders_ready_and_refused_profiles_without_modifying_source_files() {
    for corrupt in [false, true] {
        let f = Fixture::new();
        if corrupt {
            fs::remove_file(&f.exe).unwrap();
        }
        let snapshot = f.snapshot();
        let plan = f.launch_plan(&snapshot);
        let bytes = fs::read(f.game.join("eboot.bin")).unwrap();
        let context = egui::Context::default();
        let _ = context.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let request = show(ui, &snapshot, &plan.candidates[0]);
                assert_eq!(request.is_some(), !corrupt);
            });
        });
        assert_eq!(fs::read(f.game.join("eboot.bin")).unwrap(), bytes);
    }
}
#[test]
fn inventory_never_executes_a_shadps4_version_probe() {
    let f = Fixture::new();
    let marker = f.dir.path().join("probe-was-executed");
    fs::write(
        &f.exe,
        format!(
            "#!/bin/sh\nprintf probe > '{}'\nprintf '2.0\\n'\n",
            marker.display()
        ),
    )
    .unwrap();
    assert!(
        archivefs_core::emulator_inventory::probe_version_output(
            &f.exe,
            InventoryEmulator::ShadPs4
        )
        .is_none()
    );
    assert!(
        !marker.exists(),
        "inventory must not execute even a version probe"
    );
}

#[test]
fn discovered_appimage_requires_explicit_resolver_selection() {
    let mut f = Fixture::new();
    let directory = f.dir.path().join("Applications");
    fs::create_dir(&directory).unwrap();
    let image = directory.join("shadps4.AppImage");
    fs::write(&image, host_elf(true)).unwrap();
    fs::set_permissions(&image, fs::Permissions::from_mode(0o755)).unwrap();
    f.binding.configured.clear();
    f.binding.override_file = None;
    assert_eq!(
        f.inspection().request.err().unwrap().kind,
        Kind::ExecutableIdentityUnconfirmed
    );
    f.binding.mode = EmulatorSelectionMode::Forced(CandidateIdentity {
        emulator: InventoryEmulator::ShadPs4,
        executable: Some(image.clone()),
        profile_root: f.binding.roots.user_data.join("shadPS4"),
    });
    remember_selection_to(
        f.binding.selection_file.as_ref().unwrap(),
        InventoryEmulator::ShadPs4,
        &f.binding.mode,
    )
    .unwrap();
    let request = f.inspection().request.unwrap();
    assert_eq!(request.plan.command_preview().executable, image);
    assert_eq!(request.plan.command_preview().arguments[0], "-g");
    request.validate().unwrap();
}

#[test]
fn preferred_bypass_is_visible_and_forced_missing_pair_never_substitutes() {
    let mut f = Fixture::new();
    let pinned = CandidateIdentity {
        emulator: InventoryEmulator::ShadPs4,
        executable: Some(f.dir.path().join("missing")),
        profile_root: f.binding.roots.user_data.join("shadPS4"),
    };
    f.binding.mode = EmulatorSelectionMode::Preferred(pinned.clone());
    let result = f.inspection();
    assert!(result.request.is_ok());
    assert!(
        result
            .resolution
            .resolved()
            .unwrap()
            .profile
            .warnings
            .iter()
            .any(|w| matches!(w, ResolutionWarning::PreferredProfileBypassed { .. }))
    );
    f.binding.mode = EmulatorSelectionMode::Forced(pinned);
    let result = f.inspection();
    assert!(result.request.is_err());
    assert!(matches!(result.resolution, ResolutionOutcome::Refused(_)));
}

#[test]
fn reset_selection_to_auto_keeps_other_emulator_preferences() {
    let f = Fixture::new();
    let file = f.binding.selection_file.as_ref().unwrap();
    let duck = CandidateIdentity {
        emulator: InventoryEmulator::DuckStation,
        executable: Some(f.dir.path().join("duck")),
        profile_root: f.dir.path().join("duck-profile"),
    };
    remember_selection_to(
        file,
        InventoryEmulator::DuckStation,
        &EmulatorSelectionMode::Forced(duck.clone()),
    )
    .unwrap();
    f.snapshot()
        .choose_mode(EmulatorSelectionMode::Auto)
        .unwrap();
    assert_eq!(
        load_selection_from(file, InventoryEmulator::DuckStation).unwrap(),
        EmulatorSelectionMode::Forced(duck)
    );
}

#[test]
fn rapid_selection_changes_keep_one_worker_and_only_the_latest_key() {
    let f = Fixture::new();
    let (_sender, receiver) = mpsc::sync_channel(1);
    let handle = Arc::new(Mutex::new(Controller {
        selection: Some(f.selection.clone()),
        worker: Some(receiver),
        ..Default::default()
    }));
    for index in 0..100 {
        let mut key = f.selection.clone();
        key.path = f.dir.path().join(format!("selection-{index}"));
        let snapshot = Controller::snapshot(&handle, key.clone());
        assert!(snapshot.inspection.is_none());
        let state = handle.lock().unwrap();
        assert!(state.worker.is_some());
        assert!(state.needs_check);
        assert_eq!(state.selection.as_ref(), Some(&key));
    }
}
#[test]
fn a_launch_that_cannot_start_is_reported_as_a_failure_never_as_running() {
    let f = Fixture::new();
    let config = f.binding.roots.user_data.join("shadPS4/config.json");
    let before = fs::read(&config).unwrap();
    let request = StandaloneLaunchRequest::ShadPs4(f.inspection().request.unwrap());
    // The previewed executable disappears before Launch.
    fs::remove_file(&f.exe).unwrap();
    let error = match request.execute() {
        Ok(_) => panic!("a missing executable must never start a process"),
        Err(error) => error,
    };
    assert!(error.contains("ChangedAfterPreview"), "{error}");
    assert_eq!(fs::read(config).unwrap(), before);
}
#[test]
fn the_gamer_view_never_presents_a_shadps4_launch_as_plainly_ready() {
    let f = Fixture::new();
    let request = page::TypedLaunchRequest::Standalone(StandaloneLaunchRequest::ShadPs4(
        f.inspection().request.unwrap(),
    ));
    let warnings = request.unverified_warnings();
    assert!(!warnings.is_empty());
    let readiness = crate::gamer_view::GamerReadiness::Ready { request: &request };
    assert_eq!(
        crate::gamer_view::gamer_readiness_short_label(&readiness),
        "Ready, compatibility unverified"
    );
}
