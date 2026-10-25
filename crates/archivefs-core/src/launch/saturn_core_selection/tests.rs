use super::*;
use crate::emulator_environment::{
    HostReadOnlyFilesystem,
    retroarch::{DiscoveryEnvironment, ProfileRef, ProfileScope, discover_retroarch_environment},
};
use crate::launch::{
    CanonicalIdentityStatus, LaunchContainerKind, LaunchContentRef, ResolvedIdentity,
    RetroArchLaunchRequest, build_launch_plan, preflight_retroarch_launch,
};

struct Fixture {
    _temp: tempfile::TempDir,
    env: DiscoveryEnvironment,
    content: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for dir in ["bin", "cores", "info", "config/retroarch"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        let program = root.join("bin/retroarch");
        crate::verified_game_fixture::compile_native_test_executable(&program, "exit 0\n").unwrap();
        std::fs::write(
            root.join("config/retroarch/retroarch.cfg"),
            format!(
                "libretro_directory = \"{}\"\nlibretro_info_path = \"{}\"\n",
                root.join("cores").display(),
                root.join("info").display()
            ),
        )
        .unwrap();
        for stem in ["mednafen_saturn", "kronos", "yabause", "yabasanshiro"] {
            std::fs::write(
                root.join(format!("cores/{stem}_libretro.so")),
                b"synthetic core",
            )
            .unwrap();
            std::fs::write(root.join(format!("info/{stem}_libretro.info")), format!(
                "display_name = \"Real metadata {stem}\"\nsystemname = \"Saturn\"\nsupported_extensions = \"cue|chd|iso\"\n")).unwrap();
        }
        let mut bytes = vec![0u8; 24 * 2048];
        bytes[..256].fill(b' ');
        bytes[..16].copy_from_slice(b"SEGA SEGASATURN ");
        bytes[16..32].copy_from_slice(b"SEGA ENTERPRISES");
        bytes[32..39].copy_from_slice(b"T-7101G");
        bytes[42..48].copy_from_slice(b"V1.004");
        bytes[48..56].copy_from_slice(b"19961117");
        bytes[56..64].copy_from_slice(b"CD-1/1  ");
        let content = root.join("test.iso");
        std::fs::write(&content, bytes).unwrap();
        let env = DiscoveryEnvironment {
            home: Some(root.as_os_str().into()),
            xdg_config_home: Some(root.join("config").into()),
            path: Some(root.join("bin").into()),
            explicit_executables: vec![program],
            user_flatpak_root: root.join("flatpak-user"),
            system_flatpak_root: root.join("flatpak-system"),
            app_image_search_roots: vec![],
            desktop_file_roots: vec![],
        };
        Self {
            _temp: temp,
            env,
            content,
        }
    }
    fn report(&self) -> RetroArchEnvironmentReport {
        discover_retroarch_environment(&HostReadOnlyFilesystem, &self.env).unwrap()
    }
    fn selection(&self) -> SaturnCoreSelection {
        let report = self.report();
        let p = &report.profiles[0];
        SaturnCoreSelection::capture(
            p,
            p.cores.iter().find(|c| c.core_stem == "yabause").unwrap(),
        )
        .unwrap()
    }
    fn plan(&self) -> LaunchPlan {
        build_launch_plan(
            &CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: "Saturn".into(),
                game_key: "T-7101G".into(),
            }),
            &LaunchContentRef {
                kind: None,
                container: Some(LaunchContainerKind::PlainFile),
                resolved_path: Some(self.content.clone()),
                requires_mount: false,
                provenance: "test".into(),
            },
            &[],
            &self.report(),
            &[],
        )
    }
    fn request(&self) -> RetroArchLaunchRequest {
        RetroArchLaunchRequest {
            saturn_core_selection: Some(self.selection()),
            expected_snapshot_evidence: None,
            selected_content_path: self.content.clone(),
            expected_platform_id: "Saturn".into(),
            expected_game_key: "T-7101G".into(),
            profile: ProfileRef {
                profile_kind: ProfileKind::Native,
                scope: ProfileScope::User,
            },
            core_stem: "yabause".into(),
            expected_appimage_executable: None,
            expected_executable_binding: None,
            expected_configuration_file: None,
        }
    }
}

#[test]
fn ambiguous_saturn_requires_selection_and_never_guesses_even_one_core() {
    let f = Fixture::new();
    let mut p = f.plan();
    assert_eq!(p.summary.blocked, 4);
    apply(&mut p, &f.report(), Ok(None));
    assert_eq!(p.summary.ready, 0);
    let s = f.selection();
    apply(&mut p, &f.report(), Ok(Some(&s)));
    assert_eq!(p.summary.ready, 1);
    assert_eq!(
        p.candidates
            .iter()
            .find(|c| c.readiness == LaunchReadiness::Ready)
            .unwrap()
            .preference,
        CandidatePreference::Remembered
    );
    let mut p = f.plan();
    p.candidates.retain(
        |c| matches!(&c.target,LaunchTarget::RetroArchCore{core_stem,..} if core_stem=="yabause"),
    );
    apply(&mut p, &f.report(), Ok(None));
    assert_eq!(p.summary.ready, 0);
}

#[test]
fn explicit_selection_survives_serialization_and_reaches_real_preflight() {
    let f = Fixture::new();
    let request = f.request();
    let saved = serde_json::to_vec(request.saturn_core_selection.as_ref().unwrap()).unwrap();
    let decoded: SaturnCoreSelection = serde_json::from_slice(&saved).unwrap();
    assert_eq!(Some(decoded), request.saturn_core_selection);
    let command = preflight_retroarch_launch(&request, &HostReadOnlyFilesystem, &f.env).unwrap();
    assert!(
        request
            .saturn_core_selection
            .as_ref()
            .unwrap()
            .matches_command(&command)
    );
    let mut unselected = request.clone();
    unselected.saturn_core_selection = None;
    assert!(preflight_retroarch_launch(&unselected, &HostReadOnlyFilesystem, &f.env).is_err());
}

#[test]
fn missing_selected_core_never_falls_back_to_the_other_three() {
    let f = Fixture::new();
    let request = f.request();
    std::fs::remove_file(&request.saturn_core_selection.as_ref().unwrap().core.path).unwrap();
    assert!(preflight_retroarch_launch(&request, &HostReadOnlyFilesystem, &f.env).is_err());
    let mut p = f.plan();
    apply(
        &mut p,
        &f.report(),
        Ok(request.saturn_core_selection.as_ref()),
    );
    assert_eq!(p.summary.ready, 0);
}

#[test]
fn changed_core_metadata_and_executable_each_require_fresh_selection() {
    for changed in ["core", "info", "executable"] {
        let f = Fixture::new();
        let request = f.request();
        let s = request.saturn_core_selection.as_ref().unwrap();
        let path = match changed {
            "core" => s.core.path.clone(),
            "info" => f._temp.path().join("info/yabause_libretro.info"),
            _ => s.executable.path.clone(),
        };
        std::fs::write(path, b"replacement bytes").unwrap();
        let discovered = discover_retroarch_environment(&HostReadOnlyFilesystem, &f.env);
        assert!(
            discovered
                .as_ref()
                .map_or(true, |report| s.validate(report).is_err()),
            "{changed}"
        );
        assert!(
            preflight_retroarch_launch(&request, &HostReadOnlyFilesystem, &f.env).is_err(),
            "{changed}"
        );
    }
}

#[test]
fn selection_preserves_firmware_and_disc_blockers() {
    let f = Fixture::new();
    let mut p = f.plan();
    let s = f.selection();
    for c in &mut p.candidates {
        c.blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::RequiredFirmwareMissing,
            "BIOS missing",
        ));
        c.blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::ContentNotResolved,
            "track missing",
        ));
    }
    apply(&mut p, &f.report(), Ok(Some(&s)));
    assert_eq!(p.summary.ready, 0);
    assert!(p.candidates.iter().all(|c| {
        c.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::RequiredFirmwareMissing)
    }));
    assert!(p.candidates.iter().all(|c| {
        c.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::ContentNotResolved)
    }));
}

#[test]
fn spawn_rejects_backend_replaced_after_preflight() {
    let f = Fixture::new();
    let command =
        preflight_retroarch_launch(&f.request(), &HostReadOnlyFilesystem, &f.env).unwrap();
    let core = command.selection.core_library.clone();
    let result = super::super::execution::spawn_retroarch_guarded(command, || {
        std::fs::write(core, b"changed after preflight").unwrap();
        Ok(())
    });
    assert!(result.is_err());
}

#[test]
fn same_size_rewrites_with_restored_mtime_are_still_refused() {
    for executable in [false, true] {
        let f = Fixture::new();
        let request = f.request();
        let selection = request.saturn_core_selection.as_ref().unwrap();
        let binding = if executable {
            &selection.executable
        } else {
            &selection.core
        };
        let mut bytes = std::fs::read(&binding.path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&binding.path, bytes).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&binding.path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(binding.modified.unwrap()))
            .unwrap();
        let now = FileBinding::capture(&binding.path).unwrap();
        assert_eq!(now.size, binding.size);
        assert_eq!(now.modified, binding.modified);
        assert!(selection.validate(&f.report()).is_err());
        assert!(preflight_retroarch_launch(&request, &HostReadOnlyFilesystem, &f.env).is_err());
    }
}

#[test]
fn explicit_native_choice_resolves_same_core_in_unsupported_flatpak_profile() {
    let f = Fixture::new();
    let selection = f.selection();
    let mut report = f.report();
    report
        .profiles
        .retain(|p| p.profile_kind == ProfileKind::Native);
    report.profiles[0]
        .cores
        .retain(|c| c.core_stem == "yabause");
    let mut flatpak = report.profiles[0].clone();
    flatpak.profile_kind = ProfileKind::Flatpak;
    report.profiles.push(flatpak);
    let content = f.plan().candidates[0].content.clone();
    let mut plan = build_launch_plan(
        &CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Saturn".into(),
            game_key: "T-7101G".into(),
        }),
        &content,
        &[],
        &report,
        &[],
    );
    assert!(plan.candidates.iter().any(|c| !c.warnings.is_empty()));
    apply(&mut plan, &report, Ok(Some(&selection)));
    assert_eq!(plan.summary.ready, 1);
    assert_eq!(plan.summary.blocked, 1);
    assert!(plan.candidates.iter().all(|c| {
        c.blockers
            .iter()
            .all(|b| b.kind != LaunchBlockerKind::AmbiguousCore)
            && c.warnings.iter().all(|w| {
                w.kind != super::super::readiness::LaunchWarningKind::MultipleEligibleProfiles
            })
    }));
    assert!(plan.candidates[0].warnings.is_empty());
}
