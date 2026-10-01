use super::*;
use crate::launch::planning::ResolvedIdentity;
use crate::launch::safe_launch_sandbox::CleanupOutcome;
use sha2::{Digest, Sha256};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct Fixture {
    temp: TempDir,
    manager: SandboxManager,
    profile: Atari800Profile,
    media: Atari800Media,
}
impl Fixture {
    fn new(format: Atari800MediaFormat) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let exe = temp.path().join("atari800");
        // Synthetic executable stand-in ONLY; production uses typed argv.
        fs::write(&exe, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
        let rom = temp.path().join("synthetic-os.rom");
        fs::write(&rom, vec![0x31; 16384]).unwrap();
        let config = temp.path().join("isolated-seed.cfg");
        fs::write(
            &config,
            isolated_config_seed(Atari800Machine::Atari800Xl, false),
        )
        .unwrap();
        // Deliberately wrong extension/title: neither is platform evidence.
        let path = temp.path().join("Not an Atari 5200 title.dat");
        let bytes = if format == Atari800MediaFormat::Atr {
            atr_bytes()
        } else {
            vec![0; 92160]
        };
        fs::write(&path, &bytes).unwrap();
        let media = Atari800Media {
            path,
            format,
            identity: CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: PLATFORM_ID.into(),
                game_key: "synthetic-verified-evidence".into(),
            }),
            platform_evidence: LocalEvidenceStrength::Verified,
            verified_source_sha256: Sha256::digest(&bytes).into(),
        };
        let profile = Atari800Profile {
            id: "synthetic-xl".into(),
            executable: exe,
            machine: Atari800Machine::Atari800Xl,
            video: Atari800Video::Pal,
            os: Atari800Rom {
                path: rom,
                kind: Atari800RomKind::OsXlXe,
            },
            basic: None,
            isolated_seed: config,
            accept_present_unverified_roms: true,
            disposable_session_acknowledged: true,
        };
        let (manager, _) = SandboxManager::open(temp.path()).unwrap();
        Self {
            temp,
            manager,
            profile,
            media,
        }
    }
    fn plan(&self) -> Atari800Plan {
        plan_atari800(&self.profile, &self.media).unwrap()
    }
    fn prepare(&self) -> PreparedAtari800 {
        self.plan()
            .prepare(&self.manager, &self.profile, &self.media)
            .unwrap()
    }
    fn set_bytes(&mut self, bytes: &[u8]) {
        fs::write(&self.media.path, bytes).unwrap();
        self.media.verified_source_sha256 = Sha256::digest(bytes).into();
    }
    fn script(&self, text: &str) {
        fs::write(&self.profile.executable, format!("#!/bin/sh\n{text}\n")).unwrap();
    }
    fn transactions(&self) -> usize {
        fs::read_dir(self.manager.root())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().unwrap().is_dir())
            .count()
    }
}
fn atr_bytes() -> Vec<u8> {
    let mut bytes = vec![0; 92160 + 16];
    bytes[..2].copy_from_slice(&[0x96, 0x02]);
    bytes[2..4].copy_from_slice(&(5760u16).to_le_bytes());
    bytes[4..6].copy_from_slice(&(128u16).to_le_bytes());
    bytes
}
fn wait(process: &mut SandboxedProcess) -> &super::super::process_spawn::ProcessExitReport {
    let until = Instant::now() + Duration::from_secs(10);
    while process.process.poll().is_none() {
        assert!(Instant::now() < until, "stand-in process timeout");
        std::thread::sleep(Duration::from_millis(5));
    }
    process.process.poll().unwrap()
}

#[test]
fn atr_requests_explicit_scratch_policy_and_preserves_provenance() {
    let f = Fixture::new(Atari800MediaFormat::Atr);
    let plan = f.plan();
    assert_eq!(
        plan.sandbox_plan().declaration(),
        Atari800MediaFormat::Atr.safety()
    );
    assert_eq!(
        plan.sandbox_plan().declaration().safety,
        LaunchMediaSafety::ScratchCopyWithIsolatedConfig
    );
    assert_eq!(
        plan.sandbox_plan().sources().next().unwrap().original_path,
        f.media.path
    );
    assert_eq!(plan.media(), &f.media);
    assert_eq!(
        plan.firmware_readiness(),
        FirmwareReadiness::PresentUnverified
    );
    assert_eq!(plan.readiness(), LaunchReadiness::ReadyWithWarnings);
    assert_eq!(
        plan.preparation_state(),
        Atari800PreparationState::NeedsScratchPreparation
    );
}

#[test]
fn xfd_requests_scratch_and_never_receives_original_argv() {
    let f = Fixture::new(Atari800MediaFormat::Xfd);
    let prepared = f.prepare();
    let command = prepared.command_preview();
    assert_eq!(
        prepared.preparation_state(),
        Atari800PreparationState::Prepared
    );
    assert_eq!(
        prepared.original_plan().sandbox_plan().declaration(),
        Atari800MediaFormat::Xfd.safety()
    );
    assert_eq!(
        command.arguments.last().unwrap(),
        &prepared
            .workspace_path()
            .join("media/primary-00.xfd")
            .into_os_string()
    );
    for source in prepared.original_plan().sandbox_plan().sources() {
        assert!(
            !command
                .arguments
                .contains(&source.original_path.clone().into_os_string())
        );
    }
}

#[test]
fn exact_xl_argv_uses_only_private_config_and_rom_media_copies() {
    let f = Fixture::new(Atari800MediaFormat::Atr);
    let p = f.prepare();
    let root = p.workspace_path();
    let expected = vec![
        OsString::from("-config"),
        root.join("config/atari800.cfg").into_os_string(),
        "-no-autosave-config".into(),
        "-xl".into(),
        "-pal".into(),
        "-xl-rev".into(),
        "custom".into(),
        "-hreadonly".into(),
        "-xlxe_rom".into(),
        root.join("media/secondary-01.rom").into_os_string(),
        "-nobasic".into(),
        root.join("media/primary-00.atr").into_os_string(),
    ];
    assert_eq!(p.command_preview().arguments, expected);
    assert_eq!(p.command_preview().executable, f.profile.executable);
    assert_eq!(p.command_preview().working_directory.as_deref(), Some(root));
    assert_eq!(fs::metadata(root).unwrap().mode() & 0o777, 0o700);
}

#[test]
fn atx_explicitly_refused_despite_byte_preserving_copy_possibility() {
    let f = Fixture::new(Atari800MediaFormat::Atx);
    assert_eq!(
        f.media.format.safety().safety,
        LaunchMediaSafety::UnsafeUnsupported
    );
    assert!(matches!(
        plan_atari800(&f.profile, &f.media),
        Err(Atari800Error::UnsupportedMedia(_))
    ));
    assert_eq!(f.transactions(), 0);
}

#[test]
fn explicit_400800_and_xe_profiles_and_basic_rom_binding() {
    for machine in [Atari800Machine::Atari400800, Atari800Machine::Atari130Xe] {
        let mut f = Fixture::new(Atari800MediaFormat::Atr);
        f.profile.machine = machine;
        f.profile.video = Atari800Video::Ntsc;
        if machine == Atari800Machine::Atari400800 {
            f.profile.os.kind = Atari800RomKind::Os400800;
            fs::write(&f.profile.os.path, vec![0x43; 10240]).unwrap();
        }
        let basic = f.temp.path().join("synthetic-basic.rom");
        fs::write(&basic, vec![0x12; 8192]).unwrap();
        f.profile.basic = Some(Atari800Rom {
            path: basic,
            kind: Atari800RomKind::Basic,
        });
        fs::write(
            &f.profile.isolated_seed,
            isolated_config_seed(machine, true),
        )
        .unwrap();
        let p = f.prepare();
        let argv = &p.command_preview().arguments;
        assert!(argv.contains(&OsString::from(
            if machine == Atari800Machine::Atari400800 {
                "-atari"
            } else {
                "-xe"
            }
        )));
        assert!(argv.contains(&OsString::from("-ntsc")));
        assert!(argv.contains(&OsString::from("-basic")));
        assert!(
            argv.contains(
                &p.workspace_path()
                    .join("media/secondary-02.rom")
                    .into_os_string()
            )
        );
    }
}

#[test]
fn source_changed_after_plan_refuses_without_recopy_or_fallback() {
    let f = Fixture::new(Atari800MediaFormat::Atr);
    let plan = f.plan();
    let mut changed = fs::read(&f.media.path).unwrap();
    changed[100] ^= 1;
    fs::write(&f.media.path, &changed).unwrap();
    assert!(plan.prepare(&f.manager, &f.profile, &f.media).is_err());
    assert_eq!(f.transactions(), 0);
    assert_eq!(fs::read(&f.media.path).unwrap(), changed);
}

#[test]
fn changed_source_after_preparation_still_blocks_spawn() {
    let f = Fixture::new(Atari800MediaFormat::Atr);
    let prepared = f.prepare();
    let root = prepared.workspace_path().to_owned();
    fs::write(&f.media.path, b"changed deliberately by test, not emulator").unwrap();
    assert!(prepared.spawn(&f.profile, &f.media).is_err());
    assert!(!root.exists());
}

#[test]
fn wrong_verified_hash_refuses_even_when_atr_magic_is_valid() {
    let mut f = Fixture::new(Atari800MediaFormat::Atr);
    f.media.verified_source_sha256[0] ^= 1;
    assert!(matches!(
        plan_atari800(&f.profile, &f.media),
        Err(Atari800Error::IdentityNotVerified)
    ));
}

#[test]
fn insufficient_whole_set_space_refuses_before_transaction_or_copy() {
    let f = Fixture::new(Atari800MediaFormat::Atr);
    let plan = f.plan();
    assert!(plan.sandbox_plan().total_bytes() > fs::metadata(&f.media.path).unwrap().len());
    let result = f.manager.prepare_with_test_capacity(plan.sandbox_plan(), 0);
    assert!(matches!(
        result,
        Err(SafeLaunchSandboxError::InsufficientTemporarySpace { .. })
    ));
    assert_eq!(f.transactions(), 0);
    assert_eq!(fs::read(&f.media.path).unwrap(), atr_bytes());
}

#[test]
fn watched_process_writes_only_scratch_disk_config_state_and_roms() {
    let f = Fixture::new(Atari800MediaFormat::Atr);
    let user_config = f.temp.path().join("user-atari800.cfg");
    fs::write(&user_config, b"private user settings").unwrap();
    let originals = [
        &f.media.path,
        &f.profile.os.path,
        &f.profile.isolated_seed,
        &user_config,
    ]
    .map(|path| (path.clone(), fs::read(path).unwrap()));
    f.script(
        r#"
test "$PWD" = "${HOME%/}" || exit 11
test "$XDG_CONFIG_HOME" = "$PWD/config" || exit 12
test "$XDG_STATE_HOME" = "$PWD/state" || exit 13
test "$TMPDIR" = "$PWD/cache" || exit 14
test "$1" = '-config' || exit 15
test -f "$2" || exit 16
printf config-write > "$2"
for arg do
    case "$arg" in
        */media/primary-00.atr|*/media/secondary-01.rom) printf disk-write > "$arg";;
    esac
done
printf new-save > "$XDG_STATE_HOME/session.state"
printf home-config > "$HOME/.atari800.cfg"
printf 'isolated-writes-ok' >&2
exit 0
"#,
    );
    let prepared = f.prepare();
    let root = prepared.workspace_path().to_owned();
    let mut process = prepared.spawn(&f.profile, &f.media).unwrap();
    let report = wait(&mut process);
    assert!(
        report.status.as_ref().unwrap().success(),
        "status={:?}; stderr={:?}",
        report.status,
        report.stderr
    );
    assert_eq!(report.stderr, b"isolated-writes-ok");
    assert_eq!(process.cleanup_outcome(), CleanupOutcome::Removed);
    assert!(!root.exists());
    for (path, bytes) in originals {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn live_config_or_extra_absolute_output_path_refused_not_imported() {
    let f = Fixture::new(Atari800MediaFormat::Atr);
    let seed = format!(
        "{}H1_DIR=/real/preservation/library\n",
        isolated_config_seed(f.profile.machine, false)
    );
    fs::write(&f.profile.isolated_seed, &seed).unwrap();
    assert!(matches!(
        plan_atari800(&f.profile, &f.media),
        Err(Atari800Error::IsolatedProfileRequired)
    ));
    assert_eq!(fs::read_to_string(&f.profile.isolated_seed).unwrap(), seed);
    assert_eq!(f.transactions(), 0);
}

#[test]
fn missing_seed_cannot_reach_upstream_system_config_fallback() {
    let mut f = Fixture::new(Atari800MediaFormat::Atr);
    f.profile.isolated_seed = f.temp.path().join("absent.cfg");
    assert!(plan_atari800(&f.profile, &f.media).is_err());
    assert_eq!(f.transactions(), 0);
}

#[test]
fn missing_executable_has_distinct_setup_refusal() {
    let mut f = Fixture::new(Atari800MediaFormat::Atr);
    f.profile.executable = f.temp.path().join("missing-atari800");
    assert!(matches!(
        plan_atari800(&f.profile, &f.media),
        Err(Atari800Error::ExecutableMissing)
    ));
}

#[test]
fn missing_system_rom_has_distinct_readiness_refusal() {
    let mut f = Fixture::new(Atari800MediaFormat::Atr);
    f.profile.os.path = f.temp.path().join("absent.rom");
    assert!(matches!(
        plan_atari800(&f.profile, &f.media),
        Err(Atari800Error::FirmwareMissing)
    ));
}

#[test]
fn wrong_machine_rom_cannot_satisfy_readiness() {
    let mut f = Fixture::new(Atari800MediaFormat::Atr);
    f.profile.os.kind = Atari800RomKind::Os400800;
    assert!(matches!(
        plan_atari800(&f.profile, &f.media),
        Err(Atari800Error::FirmwareIncompatible)
    ));
    f.profile.os.kind = Atari800RomKind::OsXlXe;
    fs::write(&f.profile.os.path, vec![0; 10240]).unwrap();
    assert!(matches!(
        plan_atari800(&f.profile, &f.media),
        Err(Atari800Error::FirmwareIncompatible)
    ));
}

#[test]
fn no_unverified_firmware_or_disposable_session_silent_opt_in() {
    let mut f = Fixture::new(Atari800MediaFormat::Atr);
    f.profile.accept_present_unverified_roms = false;
    assert!(matches!(
        plan_atari800(&f.profile, &f.media),
        Err(Atari800Error::FirmwareNeedsAcknowledgement)
    ));
    f.profile.accept_present_unverified_roms = true;
    f.profile.disposable_session_acknowledged = false;
    assert!(matches!(
        plan_atari800(&f.profile, &f.media),
        Err(Atari800Error::DisposableSessionRequired)
    ));
}

#[test]
fn unsupported_tape_program_cartridge_hdd_manifest_and_unknown_refused() {
    for format in [
        Atari800MediaFormat::Cassette,
        Atari800MediaFormat::Program,
        Atari800MediaFormat::Cartridge,
        Atari800MediaFormat::HardDisk,
        Atari800MediaFormat::ReferenceManifest,
        Atari800MediaFormat::Unknown,
    ] {
        let f = Fixture::new(format);
        assert!(matches!(
            plan_atari800(&f.profile, &f.media),
            Err(Atari800Error::UnsupportedMedia(_))
        ));
        assert_eq!(f.transactions(), 0);
    }
}

#[test]
fn large_media_refused_without_copy_even_if_declared_floppy() {
    let mut f = Fixture::new(Atari800MediaFormat::Xfd);
    f.set_bytes(&vec![0; 1024 * 1024]);
    assert!(matches!(
        plan_atari800(&f.profile, &f.media),
        Err(Atari800Error::UnsupportedMedia(_))
    ));
    assert_eq!(f.transactions(), 0);
}

#[test]
fn unresolved_conflicting_5200_and_extension_only_identity_refused() {
    let mut f = Fixture::new(Atari800MediaFormat::Atr);
    let original = f.media.identity.clone();
    for identity in [
        CanonicalIdentityStatus::Unknown,
        CanonicalIdentityStatus::Conflicting,
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Atari5200".into(),
            game_key: "title".into(),
        }),
    ] {
        f.media.identity = identity;
        assert!(matches!(
            plan_atari800(&f.profile, &f.media),
            Err(Atari800Error::IdentityNotVerified)
        ));
    }
    f.media.identity = original;
    for strength in [LocalEvidenceStrength::None, LocalEvidenceStrength::Weak] {
        f.media.platform_evidence = strength;
        assert!(matches!(
            plan_atari800(&f.profile, &f.media),
            Err(Atari800Error::IdentityNotVerified)
        ));
    }
}

#[test]
fn xfd_cannot_smuggle_state_cassette_cartridge_program_or_compression() {
    let mut f = Fixture::new(Atari800MediaFormat::Xfd);
    for magic in [
        b"ATAR".as_slice(),
        b"AT8X",
        b"CART",
        b"FUJI",
        &[0xff, 0xff, 0, 0],
        &[0x1f, 0x8b, 0, 0],
        &[0xf9, 0, 0, 0],
        &[0xfa, 0, 0, 0],
        &[0, 0, 1, 0],
        b"10 X",
        &[0x96, 0x02, 0, 0],
    ] {
        let mut bytes = vec![0; 92160];
        bytes[..magic.len()].copy_from_slice(magic);
        f.set_bytes(&bytes);
        assert!(
            matches!(
                plan_atari800(&f.profile, &f.media),
                Err(Atari800Error::UnsupportedMedia(_))
            ),
            "{magic:?}"
        );
    }
}

#[test]
fn malformed_and_truncated_atr_fail_soft() {
    let mut f = Fixture::new(Atari800MediaFormat::Atr);
    for bytes in [vec![0x96, 0x02], vec![0; 92176], {
        let mut b = atr_bytes();
        b[2] ^= 1;
        b
    }] {
        f.set_bytes(&bytes);
        assert!(plan_atari800(&f.profile, &f.media).is_err());
    }
}

#[test]
fn source_and_parent_symlinks_refused() {
    let mut f = Fixture::new(Atari800MediaFormat::Atr);
    let link = f.temp.path().join("link.atr");
    symlink(&f.media.path, &link).unwrap();
    f.media.path = link;
    assert!(plan_atari800(&f.profile, &f.media).is_err());
    let parent = f.temp.path().join("parent-link");
    symlink(f.temp.path(), &parent).unwrap();
    f.media.path = parent.join("Not an Atari 5200 title.dat");
    assert!(plan_atari800(&f.profile, &f.media).is_err());
}

#[test]
fn executable_rom_config_and_profile_drift_block_before_spawn() {
    for target in 0..5 {
        let mut f = Fixture::new(Atari800MediaFormat::Atr);
        let p = f.prepare();
        match target {
            0 => f.script("exit 12"),
            // Same size, mtime restored: only the content hash can see it.
            4 => {
                let modified = fs::metadata(&f.profile.executable)
                    .unwrap()
                    .modified()
                    .unwrap();
                f.script("exit 1");
                assert_eq!(
                    fs::metadata(&f.profile.executable).unwrap().len(),
                    b"#!/bin/sh\nexit 0\n".len() as u64
                );
                fs::File::options()
                    .write(true)
                    .open(&f.profile.executable)
                    .unwrap()
                    .set_modified(modified)
                    .unwrap();
            }
            1 => fs::write(&f.profile.os.path, vec![0x32; 16384]).unwrap(),
            2 => fs::write(&f.profile.isolated_seed, b"changed configuration").unwrap(),
            _ => f.profile.video = Atari800Video::Ntsc,
        }
        assert!(p.spawn(&f.profile, &f.media).is_err());
    }
}

#[test]
fn selected_game_switch_does_not_spawn_stale_request() {
    let mut f = Fixture::new(Atari800MediaFormat::Atr);
    let p = f.prepare();
    f.media.identity = CanonicalIdentityStatus::Unknown;
    assert!(matches!(
        p.spawn(&f.profile, &f.media),
        Err(Atari800Error::ProfileChanged)
    ));
}

#[test]
fn scratch_tampering_fails_and_never_uses_source_as_fallback() {
    let f = Fixture::new(Atari800MediaFormat::Atr);
    let p = f.prepare();
    fs::write(
        p.workspace_path().join("media/primary-00.atr"),
        b"changed scratch",
    )
    .unwrap();
    assert!(p.spawn(&f.profile, &f.media).is_err());
    assert_eq!(fs::read(&f.media.path).unwrap(), atr_bytes());
}

#[test]
fn discovery_is_bounded_deduplicated_and_never_chooses_machine_or_runs_probe() {
    let f = Fixture::new(Atari800MediaFormat::Atr);
    f.script("exit 99");
    let path = std::env::join_paths([f.temp.path(), f.temp.path()]).unwrap();
    assert_eq!(
        discover_executables(&[f.profile.executable.clone()], Some(&path)).executables,
        vec![f.profile.executable.clone()]
    );
    fs::set_permissions(&f.profile.executable, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        discover_executables(&[], Some(&path))
            .executables
            .is_empty()
    );
    assert_eq!(
        parse_version("Atari 800 Emulator, Version 5.2.0"),
        Some("5.2.0".into())
    );
    assert_eq!(
        parse_version("Atari 800 Emulator, Version 7.1.2"),
        Some("7.1.2".into())
    );
    assert_eq!(parse_version("Not Atari800 5.2.0"), None);
}

#[test]
fn non_atari_process_spawn_remains_direct_and_does_not_inject_isolation() {
    use crate::launch::process_spawn::spawn_watched_process;
    let f = Fixture::new(Atari800MediaFormat::Atr);
    f.script("printf '%s' \"$1\" >&2");
    let command = PreparedProcessCommand {
        executable: f.profile.executable.clone(),
        arguments: vec![f.media.path.clone().into_os_string()],
        working_directory: Some(f.temp.path().to_owned()),
    };
    let mut child = spawn_watched_process(&command).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while child.poll().is_none() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        child.poll().unwrap().stderr,
        f.media.path.as_os_str().as_encoded_bytes()
    );
    assert_eq!(f.transactions(), 0);
    assert_eq!(fs::read(&f.media.path).unwrap(), atr_bytes());
}

#[test]
fn a_symlinked_executable_is_refused_and_discovery_reports_its_eligible_target() {
    let mut f = Fixture::new(Atari800MediaFormat::Atr);
    let real = f.profile.executable.clone();
    let bin = f.temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let link = bin.join("atari800");
    symlink(&real, &link).unwrap();
    f.profile.executable = link.clone();
    assert!(matches!(
        plan_atari800(&f.profile, &f.media),
        Err(Atari800Error::ExecutableUnsafe)
    ));
    let path = std::env::join_paths([&bin]).unwrap();
    let found = discover_executables(&[], Some(&path));
    assert!(found.executables.is_empty());
    assert_eq!(found.symlink_refusals.len(), 1);
    assert_eq!(found.symlink_refusals[0].link, link);
    assert_eq!(
        found.symlink_refusals[0].eligible_target.as_deref(),
        Some(fs::canonicalize(&real).unwrap().as_path())
    );
}

#[test]
fn a_seed_with_the_right_length_but_different_content_is_refused() {
    let f = Fixture::new(Atari800MediaFormat::Atr);
    let mut bytes = fs::read(&f.profile.isolated_seed).unwrap();
    // Flip one byte inside the config (same length, so a length-only check
    // would accept it).
    let at = bytes.len() - 3;
    bytes[at] ^= 0x01;
    fs::write(&f.profile.isolated_seed, &bytes).unwrap();
    assert!(matches!(
        plan_atari800(&f.profile, &f.media),
        Err(Atari800Error::IsolatedProfileRequired)
    ));
}

/// Runs the REAL emulator against the adapter's prepared command, headless
/// (SDL dummy drivers), if one is installed. Skipped otherwise. Synthetic
/// all-zero ROM-sized files stand in for firmware; no user ROM is touched.
#[test]
fn real_atari800_reads_scratch_media_and_config_and_writes_nothing_outside_the_workspace() {
    let real = Path::new("/usr/bin/atari800");
    if !real.is_file() {
        return;
    }
    let mut f = Fixture::new(Atari800MediaFormat::Atr);
    f.profile.executable = real.to_owned();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let user_config = home.as_ref().map(|h| h.join(".atari800.cfg"));
    let user_config_existed = user_config.as_ref().is_some_and(|p| p.exists());
    let source_before = fs::read(&f.media.path).unwrap();
    let plan = plan_atari800(&f.profile, &f.media).unwrap();
    let prepared = plan.prepare(&f.manager, &f.profile, &f.media).unwrap();
    let workspace = prepared.workspace_path().to_owned();
    let command = prepared.command_preview().clone();
    let mut child = std::process::Command::new(&command.executable)
        .args(&command.arguments)
        .envs(prepared.sandbox.environment())
        .env("SDL_VIDEODRIVER", "dummy")
        .env("SDL_AUDIODRIVER", "dummy")
        .current_dir(&workspace)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(2500));
    let running = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    let _ = child.wait();
    if !running {
        // No usable headless video here; the emulator never started.
        return;
    }
    assert_eq!(fs::read(&f.media.path).unwrap(), source_before);
    // Everything the emulator touched is inside its private workspace.
    let outside: Vec<_> = fs::read_dir(f.temp.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    assert!(
        outside.iter().all(|n| {
            let n = n.to_string_lossy();
            [
                "atari800",
                "synthetic-os.rom",
                "isolated-seed.cfg",
                "emuwiz",
            ]
            .contains(&&*n)
                || n.starts_with("Not an Atari")
        }),
        "{outside:?}"
    );
    if let Some(path) = &user_config {
        assert_eq!(
            path.exists(),
            user_config_existed,
            "user config was created"
        );
    }
    drop(prepared);
    assert!(!workspace.exists());
}

#[test]
fn a_missing_unreadable_or_replaced_scratch_config_refuses_before_spawn() {
    // Atari800 falls back to ~/.atari800.cfg and then /etc/atari800.cfg when its
    // -config file is absent, so a missing scratch config must never reach it.
    for attack in ["removed", "unreadable", "symlink", "edited"] {
        let f = Fixture::new(Atari800MediaFormat::Atr);
        f.script("touch ran");
        let prepared = f.prepare();
        let scratch = prepared.workspace_path().join("config/atari800.cfg");
        assert!(scratch.is_file());
        match attack {
            "removed" => fs::remove_file(&scratch).unwrap(),
            "unreadable" => fs::set_permissions(&scratch, fs::Permissions::from_mode(0)).unwrap(),
            "symlink" => {
                fs::remove_file(&scratch).unwrap();
                symlink(&f.profile.isolated_seed, &scratch).unwrap();
            }
            _ => fs::write(&scratch, "HD_READ_ONLY=0\n").unwrap(),
        }
        let workspace = prepared.workspace_path().to_owned();
        assert!(prepared.spawn(&f.profile, &f.media).is_err(), "{attack}");
        assert!(!workspace.join("ran").exists(), "{attack}");
        assert_eq!(
            fs::read_to_string(&f.profile.isolated_seed).unwrap(),
            isolated_config_seed(Atari800Machine::Atari800Xl, false)
        );
    }
}

#[test]
fn changed_removed_or_symlinked_firmware_refuses_before_spawn() {
    for change in 0..3 {
        let f = Fixture::new(Atari800MediaFormat::Atr);
        f.script("touch ran");
        let prepared = f.prepare();
        let workspace = prepared.workspace_path().to_owned();
        match change {
            0 => fs::write(&f.profile.os.path, vec![0x77; 16384]).unwrap(),
            1 => fs::remove_file(&f.profile.os.path).unwrap(),
            _ => {
                let other = f.temp.path().join("other.rom");
                fs::write(&other, vec![0x31; 16384]).unwrap();
                fs::remove_file(&f.profile.os.path).unwrap();
                symlink(&other, &f.profile.os.path).unwrap();
            }
        }
        assert!(prepared.spawn(&f.profile, &f.media).is_err(), "{change}");
        assert!(!workspace.join("ran").exists(), "{change}");
    }
}
