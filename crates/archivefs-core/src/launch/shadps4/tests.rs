use super::super::{
    evidence_bridge::canonical_identity_from_game_report,
    installation::LaunchInstallation,
    installation_known::KnownInstallRoots,
    planning::{
        LaunchContainerKind, LaunchContentKind, LaunchContentRef, StandaloneProfileInput,
        build_launch_plan,
    },
    shadps4_input::{ShadPs4BootRepresentation, inspect_shadps4_game},
    shadps4_profile::{
        ShadPs4ConfigFormat, ShadPs4DiscoveryCandidate, ShadPs4ExecutableEvidence,
        discover_shadps4, inspect_shadps4_profile,
    },
};
use super::*;
use std::{
    ffi::OsString,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

struct Fixture {
    dir: tempfile::TempDir,
    exe: PathBuf,
    root: PathBuf,
    data: PathBuf,
}
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
fn self_image() -> Vec<u8> {
    let elf = ps4_elf();
    let mut bytes = vec![0u8; 64];
    bytes[..4].copy_from_slice(&0x1d3d154fu32.to_le_bytes());
    bytes[4..10].copy_from_slice(&[0, 1, 1, 0x12, 1, 1]);
    bytes[12..14].copy_from_slice(&128u16.to_le_bytes());
    bytes[16..20].copy_from_slice(&(64u32 + elf.len() as u32).to_le_bytes());
    bytes[24..26].copy_from_slice(&1u16.to_le_bytes());
    bytes[40..48].copy_from_slice(&64u64.to_le_bytes());
    bytes[48..56].copy_from_slice(&(elf.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&elf);
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
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("shadPS4");
        fs::write(&exe, host_elf(false)).unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        let root = dir.path().join("arbitrary title ; $(inert) Ω");
        fs::create_dir_all(root.join("sce_sys")).unwrap();
        fs::write(
            root.join("sce_sys/param.sfo"),
            sfo("CUSA00001", "UP0001-CUSA00001_00-LABEL00000000000"),
        )
        .unwrap();
        fs::write(root.join("eboot.bin"), ps4_elf()).unwrap();
        let data = dir.path().join("xdg");
        fs::create_dir_all(data.join("shadPS4")).unwrap();
        fs::write(
            data.join("shadPS4/config.json"),
            b"{\"General\":{},\"GPU\":{\"full_screen\":false}}",
        )
        .unwrap();
        Self {
            dir,
            exe,
            root,
            data,
        }
    }
    fn profile(&self) -> ShadPs4Profile {
        self.inspect(&self.exe, false).unwrap()
    }
    fn inspect(&self, path: &Path, selected: bool) -> Result<ShadPs4Profile, ShadPs4Refusal> {
        inspect_shadps4_profile(
            &ShadPs4DiscoveryCandidate {
                executable: path.into(),
                user_selected: selected,
            },
            self.dir.path(),
            &self.data,
        )
    }
    fn game(&self) -> ShadPs4GameInput {
        inspect_shadps4_game(&self.root).unwrap()
    }
    fn identity(&self) -> CanonicalIdentityStatus {
        canonical_identity_from_game_report(&crate::game_identity::inspect_game_identity(
            &self.root,
            Some("PS4"),
        ))
        .0
    }
    fn plan(&self) -> ShadPs4LaunchPlan {
        plan_shadps4_launch(
            &self.profile(),
            &self.game(),
            &self.identity(),
            Default::default(),
        )
        .unwrap()
    }
    fn roots(&self) -> KnownInstallRoots {
        KnownInstallRoots {
            home: self.dir.path().into(),
            user_data: self.data.clone(),
            system_data: self.dir.path().join("system"),
            path_dirs: vec![self.dir.path().into()],
        }
    }
}

#[test]
fn symlinked_override_directory_is_refused_even_without_a_title_override() {
    let f = Fixture::new();
    let plan = f.plan();
    let outside = f.dir.path().join("other-configs");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, f.data.join("shadPS4/custom_configs")).unwrap();
    assert_eq!(
        preflight_shadps4_launch(&plan).unwrap_err().kind,
        Kind::ChangedAfterPreview
    );
    assert_eq!(
        plan_shadps4_launch(&f.profile(), &f.game(), &f.identity(), Default::default())
            .unwrap_err()
            .kind,
        Kind::UnsafePath
    );
}

#[test]
fn ps4_non_load_metadata_segment_need_not_have_a_memory_extent() {
    let f = Fixture::new();
    let mut boot = ps4_elf();
    boot.splice(120..120, [0u8; 56]);
    let size = boot.len() as u64;
    boot[56..58].copy_from_slice(&2u16.to_le_bytes());
    boot[96..104].copy_from_slice(&size.to_le_bytes());
    boot[104..112].copy_from_slice(&size.to_le_bytes());
    boot[120..124].copy_from_slice(&0x61000000u32.to_le_bytes());
    boot[128..136].copy_from_slice(&232u64.to_le_bytes());
    boot[152..160].copy_from_slice(&4u64.to_le_bytes());
    fs::write(f.root.join("eboot.bin"), boot).unwrap();
    assert_eq!(f.game().representation(), ShadPs4BootRepresentation::Ps4Elf);
}
#[test]
fn native_and_path_discovery_are_bounded_read_only_and_do_not_probe() {
    let f = Fixture::new();
    let path = std::env::join_paths([f.dir.path()]).unwrap();
    let found = discover_shadps4(&f.roots(), &[], Some(&path));
    assert_eq!(
        found,
        vec![ShadPs4DiscoveryCandidate {
            executable: f.exe.clone(),
            user_selected: false
        }]
    );
    assert_eq!(
        f.profile().executable_evidence,
        ShadPs4ExecutableEvidence::NativeCliMarker
    );
    assert_eq!(f.profile().version, None);
    assert_eq!(
        discover_shadps4(&f.roots(), &[f.exe.clone()], None)[0].user_selected,
        true
    );
    let root_count = fs::read_dir(f.dir.path()).unwrap().count();
    let _ = f.profile();
    assert_eq!(root_count, fs::read_dir(f.dir.path()).unwrap().count());
}
#[test]
fn missing_non_runnable_symlink_and_similarly_named_executables_refused() {
    let f = Fixture::new();
    assert_eq!(
        f.inspect(&f.dir.path().join("missing"), false)
            .unwrap_err()
            .kind,
        Kind::ExecutableMissing
    );
    fs::set_permissions(&f.exe, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        f.inspect(&f.exe, false).unwrap_err().kind,
        Kind::ExecutableNotRunnable
    );
    fs::set_permissions(&f.exe, fs::Permissions::from_mode(0o755)).unwrap();
    let link = f.dir.path().join("alias");
    symlink(&f.exe, &link).unwrap();
    assert!(f.inspect(&link, true).is_err());
    let mut bytes = host_elf(false);
    bytes[140..160].fill(0);
    fs::write(&f.exe, bytes).unwrap();
    assert_eq!(
        f.inspect(&f.exe, true).unwrap_err().kind,
        Kind::ExecutableIdentityUnconfirmed
    );
}
#[test]
fn appimage_discovered_but_identity_requires_explicit_selection_and_direct_execution() {
    let f = Fixture::new();
    let apps = f.dir.path().join("Applications");
    fs::create_dir(&apps).unwrap();
    let exe = apps.join("shadPS4-core.AppImage");
    fs::write(&exe, host_elf(true)).unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    let found = discover_shadps4(&f.roots(), &[], None);
    assert_eq!(found.len(), 1);
    assert!(!found[0].user_selected);
    assert_eq!(
        f.inspect(&exe, false).unwrap_err().kind,
        Kind::ExecutableIdentityUnconfirmed
    );
    let profile = f.inspect(&exe, true).unwrap();
    assert_eq!(
        profile.installation,
        LaunchInstallation::AppImage {
            extract_and_run: false
        }
    );
    let plan = plan_shadps4_launch(&profile, &f.game(), &f.identity(), Default::default()).unwrap();
    assert_eq!(plan.command_preview().executable, exe);
    assert_eq!(plan.command_preview().arguments[0], "-g");
    assert!(
        !plan
            .command_preview()
            .arguments
            .iter()
            .any(|a| a == "--appimage-extract-and-run")
    );
}
#[test]
fn extracted_root_and_eboot_share_verified_identity_and_provenance() {
    let f = Fixture::new();
    let game = f.game();
    assert_eq!(
        game,
        inspect_shadps4_game(&f.root.join("eboot.bin")).unwrap()
    );
    assert_eq!(game.title_id(), "CUSA00001");
    assert_eq!(
        game.content_id(),
        Some("UP0001-CUSA00001_00-LABEL00000000000")
    );
    assert_eq!(game.identity_provenance(), f.root.join("sce_sys/param.sfo"));
    assert_eq!(game.representation(), ShadPs4BootRepresentation::Ps4Elf);
    assert_eq!(f.plan().identity().platform_id, "PS4");
    assert_eq!(f.plan().identity().game_key, "CUSA00001");
}
#[test]
fn package_iso_archive_arbitrary_elf_missing_boot_and_wrong_platform_are_refused() {
    let f = Fixture::new();
    for ext in ["pkg", "iso", "zip", "zar", "elf"] {
        let path = f.dir.path().join(format!("CUSA00001.{ext}"));
        fs::write(&path, ps4_elf()).unwrap();
        assert_eq!(
            inspect_shadps4_game(&path).unwrap_err().kind,
            Kind::UnsupportedGameLayout
        );
    }
    fs::remove_file(f.root.join("eboot.bin")).unwrap();
    assert!(inspect_shadps4_game(&f.root).is_err());
}
#[test]
fn self_header_is_supported_only_without_encryption_compression_or_truncation() {
    let f = Fixture::new();
    fs::write(f.root.join("eboot.bin"), self_image()).unwrap();
    assert_eq!(
        f.game().representation(),
        ShadPs4BootRepresentation::UnencryptedUncompressedSelf
    );
    for flag in [2u8, 8] {
        let mut bytes = self_image();
        bytes[32] = flag;
        fs::write(f.root.join("eboot.bin"), bytes).unwrap();
        assert!(inspect_shadps4_game(&f.root).is_err());
    }
    fs::write(f.root.join("eboot.bin"), &self_image()[..60]).unwrap();
    assert!(inspect_shadps4_game(&f.root).is_err());
}
#[test]
fn conflicting_sfo_ids_filename_only_and_conflicting_canonical_identity_never_launch() {
    let f = Fixture::new();
    let p = f.profile();
    let game = f.game();
    for identity in [
        CanonicalIdentityStatus::Unknown,
        CanonicalIdentityStatus::Conflicting,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "PS3".into(),
            game_key: "CUSA00001".into(),
        }),
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "PS4".into(),
            game_key: "CUSA99999".into(),
        }),
    ] {
        assert_eq!(
            plan_shadps4_launch(&p, &game, &identity, Default::default())
                .unwrap_err()
                .kind,
            Kind::IdentityUnverified
        );
    }
    fs::write(
        f.root.join("sce_sys/param.sfo"),
        sfo("CUSA00001", "UP0001-CUSA99999_00-LABEL00000000000"),
    )
    .unwrap();
    assert_eq!(
        inspect_shadps4_game(&f.root).unwrap_err().kind,
        Kind::IdentityUnverified
    );
}
#[test]
fn argv_is_exact_and_paths_are_inert_osstrings() {
    let f = Fixture::new();
    let plan = plan_shadps4_launch(
        &f.profile(),
        &f.game(),
        &f.identity(),
        ShadPs4LaunchOptions {
            fullscreen: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        plan.command_preview().arguments,
        vec![
            OsString::from("-g"),
            f.root.join("eboot.bin").into_os_string(),
            "--fullscreen".into(),
            "true".into()
        ]
    );
    assert_eq!(
        plan.command_preview().working_directory.as_deref(),
        Some(f.dir.path())
    );
    assert!(matches!(
        plan.target(),
        LaunchTarget::Standalone {
            adapter_id: "shadps4",
            ..
        }
    ));
    assert_eq!(plan.readiness(), LaunchReadiness::ReadyWithWarnings);
    assert!(!plan.warnings().is_empty());
}
#[test]
fn xdg_portable_and_legacy_config_resolution_are_read_only() {
    let f = Fixture::new();
    assert_eq!(f.profile().user_directory, f.data.join("shadPS4"));
    let user = f.dir.path().join("user");
    fs::create_dir(&user).unwrap();
    fs::write(
        user.join("config.toml"),
        b"[General]\nsysModulesPath = ''\n[GPU]\nFullscreen = true\n",
    )
    .unwrap();
    let before = fs::read(user.join("config.toml")).unwrap();
    let profile = f.profile();
    assert_eq!(profile.user_directory, user);
    assert_eq!(profile.config_format, ShadPs4ConfigFormat::LegacyToml);
    assert_eq!(profile.settings.fullscreen, Some(true));
    assert_eq!(fs::read(profile.config_path).unwrap(), before);
    assert!(!user.join("config.json").exists());
}
#[test]
fn malformed_missing_huge_and_unsafe_config_refused() {
    let f = Fixture::new();
    let path = f.data.join("shadPS4/config.json");
    for bytes in [
        b"{".as_slice(),
        b"[]",
        b"{\"General\":{\"sys_modules_dir\":\"relative\"}}",
        b"{\"GPU\":{\"full_screen\":123}}",
    ] {
        fs::write(&path, bytes).unwrap();
        assert!(f.inspect(&f.exe, false).is_err());
    }
    fs::write(&path, vec![b' '; 1024 * 1024 + 1]).unwrap();
    assert!(f.inspect(&f.exe, false).is_err());
    fs::remove_file(&path).unwrap();
    assert!(f.inspect(&f.exe, false).is_err());
    symlink(&f.exe, &path).unwrap();
    assert!(f.inspect(&f.exe, false).is_err());
}
#[test]
fn dependency_missing_is_typed_and_presence_is_unverified_not_compatibility() {
    let f = Fixture::new();
    let module = ShadPs4Sysmodule::LibcInternal;
    let options = ShadPs4LaunchOptions {
        required_sysmodules: vec![module],
        ..Default::default()
    };
    assert_eq!(
        plan_shadps4_launch(&f.profile(), &f.game(), &f.identity(), options.clone())
            .unwrap_err()
            .kind,
        Kind::RequiredSysmoduleMissing
    );
    let root = f.data.join("shadPS4/sys_modules");
    fs::create_dir(&root).unwrap();
    fs::write(root.join(module.filename()), b"synthetic unverified module").unwrap();
    let plan = plan_shadps4_launch(&f.profile(), &f.game(), &f.identity(), options).unwrap();
    assert_eq!(
        preflight_shadps4_launch(&plan).unwrap(),
        ShadPs4PreflightState::VerifiedReady
    );
    assert_eq!(plan.readiness(), LaunchReadiness::ReadyWithWarnings);
    fs::remove_file(root.join(module.filename())).unwrap();
    assert!(preflight_shadps4_launch(&plan).is_err());
}
#[test]
fn same_size_executable_swap_even_with_restored_mtime_refuses_launch() {
    let f = Fixture::new();
    let plan = f.plan();
    let modified = fs::metadata(&f.exe).unwrap().modified().unwrap();
    let mut bytes = fs::read(&f.exe).unwrap();
    bytes[179] ^= 1;
    fs::write(&f.exe, bytes).unwrap();
    fs::File::options()
        .write(true)
        .open(&f.exe)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert_eq!(
        launch_shadps4(&plan).err().unwrap().kind,
        Kind::ChangedAfterPreview
    );
}
#[test]
fn source_sfo_config_and_override_appearance_invalidate_preflight() {
    for relative in ["eboot.bin", "sce_sys/param.sfo"] {
        let f = Fixture::new();
        let plan = f.plan();
        fs::write(f.root.join(relative), b"changed").unwrap();
        assert!(preflight_shadps4_launch(&plan).is_err());
    }
    let f = Fixture::new();
    let plan = f.plan();
    fs::write(plan.profile.config_path.clone(), b"{}").unwrap();
    assert!(preflight_shadps4_launch(&plan).is_err());
    let f = Fixture::new();
    let plan = f.plan();
    let root = f.data.join("shadPS4/custom_configs");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("CUSA00001.json"), b"{}").unwrap();
    assert!(preflight_shadps4_launch(&plan).is_err());
}
#[test]
fn portable_mode_appearing_after_preview_refuses_spawn() {
    let f = Fixture::new();
    let plan = f.plan();
    fs::create_dir(f.dir.path().join("user")).unwrap();
    assert!(preflight_shadps4_launch(&plan).is_err());
}
#[test]
fn ps4_profile_projects_into_existing_standalone_planner_without_ps5() {
    let f = Fixture::new();
    let identity = f.identity();
    let profile = f.profile();
    let input: StandaloneProfileInput = profile.launch_profile_input();
    assert_eq!(input.adapter_id, "shadps4");
    assert_eq!(
        super::super::platform_map::launch_compatibility_for_platform("PS4")
            .unwrap()
            .standalone_adapters,
        &["shadps4"]
    );
    assert!(super::super::platform_map::launch_compatibility_for_platform("PS5").is_none());

    let content = LaunchContentRef {
        kind: Some(LaunchContentKind::Executable),
        container: Some(LaunchContainerKind::PlainFile),
        resolved_path: Some(f.root.join("eboot.bin")),
        requires_mount: false,
        provenance: "verified PS4 source".into(),
    };
    let retroarch = crate::emulator_environment::retroarch::RetroArchEnvironmentReport {
        format_version: 1,
        profiles: vec![],
        diagnostics: vec![],
    };
    let plan = build_launch_plan(&identity, &content, &[input], &retroarch, &[]);
    assert!(plan.candidates.iter().any(|c| matches!(
        c.target,
        LaunchTarget::Standalone {
            adapter_id: "shadps4",
            ..
        }
    )));
    let sources =
        [super::super::integration::DiscoveredStandaloneProfile::ShadPs4 { profile: &profile }];
    let results = super::super::integration::LaunchPlanResults {
        identity: &identity,
        verified_identity_facts: &[],
        content: &content,
        standalone_profiles: &sources,
        retroarch: &retroarch,
        remembered: &[],
    };
    let integrated = super::super::integration::build_launch_plan_from_results(&results);
    assert_eq!(integrated, plan);
}
#[test]
#[cfg(target_arch = "x86_64")]
fn real_spawn_reports_pid_and_nonzero_exit_without_source_or_config_writes() {
    let f = Fixture::new();
    let plan = f.plan();
    let config = fs::read(&plan.profile.config_path).unwrap();
    let boot = fs::read(f.root.join("eboot.bin")).unwrap();
    let mut process = launch_shadps4(&plan).unwrap();
    assert!(process.pid > 0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while process.poll().is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        process.poll().unwrap().status.as_ref().unwrap().code(),
        Some(7)
    );
    assert!(!process.is_running());
    assert_eq!(fs::read(&plan.profile.config_path).unwrap(), config);
    assert_eq!(fs::read(f.root.join("eboot.bin")).unwrap(), boot);
}

#[test]
fn traversal_symlink_and_host_elf_with_ps4_metadata_are_not_game_inputs() {
    let f = Fixture::new();
    assert!(inspect_shadps4_game(&f.root.join("../").join(f.root.file_name().unwrap())).is_err());
    let alias = f.dir.path().join("game-alias");
    symlink(&f.root, &alias).unwrap();
    assert!(inspect_shadps4_game(&alias).is_err());
    fs::remove_file(f.root.join("eboot.bin")).unwrap();
    symlink(&f.exe, f.root.join("eboot.bin")).unwrap();
    assert!(inspect_shadps4_game(&f.root).is_err());
    fs::remove_file(f.root.join("eboot.bin")).unwrap();
    fs::write(f.root.join("eboot.bin"), host_elf(false)).unwrap();
    assert!(inspect_shadps4_game(&f.root).is_err());
}
#[test]
fn malformed_boot_tables_and_truncated_elf_fail_without_unbounded_allocation() {
    let f = Fixture::new();
    for bytes in [vec![], vec![0; 64], ps4_elf()[..63].to_vec()] {
        fs::write(f.root.join("eboot.bin"), bytes).unwrap();
        assert!(inspect_shadps4_game(&f.root).is_err());
    }
    let mut elf = ps4_elf();
    elf[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
    fs::write(f.root.join("eboot.bin"), elf).unwrap();
    assert!(inspect_shadps4_game(&f.root).is_err());
    let mut image = self_image();
    image[24..26].copy_from_slice(&u16::MAX.to_le_bytes());
    fs::write(f.root.join("eboot.bin"), image).unwrap();
    assert!(inspect_shadps4_game(&f.root).is_err());
}
#[test]
fn config_json_has_precedence_and_new_json_invalidates_legacy_preview() {
    let f = Fixture::new();
    let root = f.data.join("shadPS4");
    fs::write(root.join("config.toml"), b"[GPU]\nFullscreen = true\n").unwrap();
    assert_eq!(f.profile().config_format, ShadPs4ConfigFormat::Json);
    fs::remove_file(root.join("config.json")).unwrap();
    let plan = f.plan();
    fs::write(root.join("config.json"), b"{}").unwrap();
    assert!(preflight_shadps4_launch(&plan).is_err());
}
#[test]
fn new_adapter_never_probes_or_builds_shell_commands() {
    for source in [
        include_str!("../shadps4.rs"),
        include_str!("../shadps4_profile.rs"),
        include_str!("../shadps4_input.rs"),
    ] {
        let production = source.split("#[cfg(test)]").next().unwrap();
        for token in [
            "Command::new",
            "--version",
            "--help",
            "sh -c",
            "run_command",
            "fs::write",
        ] {
            assert!(
                !production.contains(token),
                "unexpected execution/write token {token}"
            );
        }
    }
}
