use super::*;
use crate::launch::planning::ResolvedIdentity;
use crate::launch::safe_launch_sandbox::{
    CleanupOutcome, LaunchMediaSafety, SafeLaunchSandboxError,
};
use sha2::{Digest, Sha256};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct Fixture {
    temp: TempDir,
    manager: SandboxManager,
    profile: Caprice32Profile,
    media: Caprice32Media,
}

fn dsk_bytes() -> Vec<u8> {
    let mut v = vec![0; 512];
    v[..21].copy_from_slice(b"MV - CPCEMU Disk-File");
    v
}

impl Fixture {
    fn new(format: Caprice32MediaFormat) -> Self {
        Self::named(format, "ambiguous-name.bin")
    }

    fn named(format: Caprice32MediaFormat, file_name: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        // The install layout Caprice32 expects: executable with rom/ beside it.
        let install = temp.path().join("install");
        std::fs::create_dir_all(install.join("rom")).unwrap();
        let executable = install.join("cap32");
        std::fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        for name in ["cpc464.rom", "cpc664.rom", "cpc6128.rom", "amsdos.rom"] {
            std::fs::write(install.join("rom").join(name), vec![0x42; 16384]).unwrap();
        }
        let bytes = match format {
            Caprice32MediaFormat::Dsk => dsk_bytes(),
            Caprice32MediaFormat::Cdt => {
                let mut v = vec![0; 512];
                v[..8].copy_from_slice(b"ZXTape!\x1a");
                v
            }
            _ => vec![0; 512],
        };
        let path = temp.path().join(file_name);
        std::fs::write(&path, &bytes).unwrap();
        let seed = temp.path().join("seed.cfg");
        std::fs::write(&seed, isolated_config_seed(Caprice32Machine::Cpc6128)).unwrap();
        let media = Caprice32Media {
            path,
            format,
            identity: CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: PLATFORM_ID.into(),
                game_key: "verified-cpc-fixture".into(),
            }),
            platform_evidence: LocalEvidenceStrength::Verified,
            verified_source_sha256: Sha256::digest(&bytes).into(),
        };
        let profile = Caprice32Profile {
            id: "cpc6128".into(),
            executable,
            machine: Caprice32Machine::Cpc6128,
            isolated_seed: seed,
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

    fn rom(&self, name: &str) -> PathBuf {
        self.profile
            .executable
            .parent()
            .unwrap()
            .join("rom")
            .join(name)
    }

    fn plan(&self) -> Caprice32Plan {
        plan_caprice32(&self.profile, &self.media).unwrap()
    }

    fn prepare(&self) -> PreparedCaprice32 {
        self.plan()
            .prepare(&self.manager, &self.profile, &self.media)
            .unwrap()
    }
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
fn the_seed_is_valid_ini_with_an_integer_model_and_every_writable_path_redirected() {
    let seed = isolated_config_seed(Caprice32Machine::Cpc6128);
    assert!(seed.contains("[system]\nmodel=2\n"));
    assert!(isolated_config_seed(Caprice32Machine::Cpc464).contains("model=0\n"));
    assert!(isolated_config_seed(Caprice32Machine::Cpc664).contains("model=1\n"));
    // Every location Caprice32 writes by default (all under the executable
    // directory) is named, and none is absolute.
    for key in [
        "snap_path",
        "cart_path",
        "dsk_path",
        "tape_path",
        "printer_file",
        "sdump_dir",
    ] {
        let line = seed
            .lines()
            .find(|l| l.starts_with(&format!("{key}=")))
            .unwrap_or_else(|| panic!("{key} missing"));
        assert!(line.contains("=state"), "{line}");
    }
    assert!(!seed.contains("=/"));
    // Not the invented dotted/string syntax of the earlier prototype.
    assert!(!seed.contains("system.model"));
}

#[test]
fn verified_dsk_uses_scratch_and_the_exact_argv() {
    let f = Fixture::new(Caprice32MediaFormat::Dsk);
    let plan = f.plan();
    assert_eq!(
        plan.sandbox_plan().declaration().safety,
        LaunchMediaSafety::ScratchCopyWithIsolatedConfig
    );
    assert_eq!(
        plan.sandbox_plan().declaration().config_isolation,
        ConfigIsolation::Combined { flag: "--cfg_file" }
    );
    let prepared = plan.prepare(&f.manager, &f.profile, &f.media).unwrap();
    let root = prepared.workspace_path();
    assert_eq!(
        prepared.command_preview().arguments,
        vec![
            OsString::from("--cfg_file"),
            root.join("config/caprice32.cfg").into_os_string(),
            root.join("media/primary-00.dsk").into_os_string(),
        ]
    );
    assert_eq!(
        prepared.command_preview().working_directory.as_deref(),
        Some(root)
    );
    assert!(
        !prepared
            .command_preview()
            .arguments
            .iter()
            .any(|arg| arg == f.media.path.as_os_str())
    );
}

use std::ffi::OsString;

#[test]
fn verified_cdt_uses_scratch_copy() {
    let f = Fixture::new(Caprice32MediaFormat::Cdt);
    let prepared = f.prepare();
    assert!(
        prepared
            .command_preview()
            .arguments
            .last()
            .unwrap()
            .to_string_lossy()
            .ends_with("primary-00.cdt")
    );
}

#[test]
fn spaces_unicode_and_shell_metacharacters_stay_inert_single_argv_tokens() {
    for name in [
        "Disk with spaces.dsk",
        "Dísk ünïcode – 日本.dsk",
        "$(touch pwned);`id`; --cfg_file=etc-passwd.dsk",
        "-O system.model=3.dsk",
    ] {
        let f = Fixture::named(Caprice32MediaFormat::Dsk, name);
        let prepared = f.prepare();
        let arguments = &prepared.command_preview().arguments;
        // The scratch name is generated; the hostile source name never reaches
        // argv and no token is split or interpreted.
        assert_eq!(arguments.len(), 3, "{name}");
        assert!(
            arguments
                .iter()
                .all(|a| !a.to_string_lossy().contains("pwned"))
        );
        assert!(
            arguments
                .iter()
                .all(|a| !a.to_string_lossy().contains("etc-passwd"))
        );
        assert!(
            arguments[2]
                .to_string_lossy()
                .ends_with("media/primary-00.dsk")
        );
        assert!(!f.temp.path().join("pwned").exists());
    }
}

#[test]
fn extension_only_or_unverified_identity_is_refused() {
    let mut f = Fixture::new(Caprice32MediaFormat::Dsk);
    f.media.platform_evidence = LocalEvidenceStrength::Weak;
    assert!(matches!(
        plan_caprice32(&f.profile, &f.media),
        Err(Caprice32Error::IdentityNotVerified)
    ));
    let mut f = Fixture::new(Caprice32MediaFormat::Dsk);
    f.media.verified_source_sha256 = [9; 32];
    assert!(matches!(
        plan_caprice32(&f.profile, &f.media),
        Err(Caprice32Error::IdentityNotVerified)
    ));
}

#[test]
fn source_drift_fails_before_spawn_and_cleans_up() {
    let f = Fixture::new(Caprice32MediaFormat::Dsk);
    let prepared = f.prepare();
    let workspace = prepared.workspace_path().to_owned();
    std::fs::write(&f.media.path, b"replacement").unwrap();
    assert!(matches!(
        prepared.spawn(&f.profile, &f.media),
        Err(Caprice32Error::Sandbox(_))
    ));
    assert!(!workspace.exists());
}

#[test]
fn source_replacement_at_same_path_fails_closed() {
    let f = Fixture::new(Caprice32MediaFormat::Dsk);
    let plan = f.plan();
    std::fs::write(&f.media.path, vec![0x55; 512]).unwrap();
    assert!(matches!(
        plan.prepare(&f.manager, &f.profile, &f.media),
        Err(Caprice32Error::IdentityNotVerified | Caprice32Error::Sandbox(_))
    ));
}

#[test]
fn media_removed_after_planning_fails_closed() {
    let f = Fixture::new(Caprice32MediaFormat::Dsk);
    let plan = f.plan();
    std::fs::remove_file(&f.media.path).unwrap();
    assert!(plan.prepare(&f.manager, &f.profile, &f.media).is_err());
}

#[test]
fn missing_executable_blocks() {
    let mut f = Fixture::new(Caprice32MediaFormat::Dsk);
    f.profile.executable = f.temp.path().join("install/missing-cap32");
    assert!(matches!(
        plan_caprice32(&f.profile, &f.media),
        Err(Caprice32Error::ExecutableMissing)
    ));
}

#[test]
fn a_symlinked_executable_is_refused_like_every_other_adapter() {
    let mut f = Fixture::new(Caprice32MediaFormat::Dsk);
    let real = f.profile.executable.clone();
    let bin = f.temp.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let link = bin.join("caprice32");
    symlink(&real, &link).unwrap();
    f.profile.executable = link.clone();
    assert!(matches!(
        plan_caprice32(&f.profile, &f.media),
        Err(Caprice32Error::ExecutableUnsafe)
    ));
    // Discovery reports the link and the target a person may select instead.
    let path = std::env::join_paths([&bin]).unwrap();
    let found = discover_executables(&[], Some(&path));
    assert!(found.executables.is_empty());
    assert_eq!(found.symlink_refusals.len(), 1);
    assert_eq!(
        found.symlink_refusals[0].eligible_target.as_deref(),
        Some(std::fs::canonicalize(&real).unwrap().as_path())
    );
    // Selecting the resolved regular file works.
    f.profile.executable = found.symlink_refusals[0].eligible_target.clone().unwrap();
    assert!(plan_caprice32(&f.profile, &f.media).is_ok());
}

#[test]
fn firmware_is_derived_hash_bound_and_unverified() {
    let f = Fixture::new(Caprice32MediaFormat::Dsk);
    let plan = f.plan();
    assert_eq!(
        plan.firmware_readiness(),
        FirmwareReadiness::PresentUnverified
    );
    assert_eq!(plan.readiness(), LaunchReadiness::ReadyWithWarnings);
    assert_eq!(plan.firmware().paths().count(), 2);
    assert!(plan.warnings()[0].contains("discarded"));
}

#[test]
fn missing_firmware_blocks_readiness() {
    for rom in ["cpc6128.rom", "amsdos.rom"] {
        let f = Fixture::new(Caprice32MediaFormat::Dsk);
        std::fs::remove_file(f.rom(rom)).unwrap();
        assert!(matches!(
            plan_caprice32(&f.profile, &f.media),
            Err(Caprice32Error::FirmwareMissing)
        ));
    }
    // A symlinked ROM is not accepted as firmware.
    let f = Fixture::new(Caprice32MediaFormat::Dsk);
    let real = f.temp.path().join("elsewhere.rom");
    std::fs::write(&real, vec![0x42; 16384]).unwrap();
    std::fs::remove_file(f.rom("cpc6128.rom")).unwrap();
    symlink(&real, f.rom("cpc6128.rom")).unwrap();
    assert!(matches!(
        plan_caprice32(&f.profile, &f.media),
        Err(Caprice32Error::FirmwareMissing)
    ));
}

#[test]
fn unverified_firmware_needs_explicit_acknowledgement() {
    let mut f = Fixture::new(Caprice32MediaFormat::Dsk);
    f.profile.accept_present_unverified_roms = false;
    assert!(matches!(
        plan_caprice32(&f.profile, &f.media),
        Err(Caprice32Error::FirmwareNeedsAcknowledgement)
    ));
    f.profile.accept_present_unverified_roms = true;
    f.profile.disposable_session_acknowledged = false;
    assert!(matches!(
        plan_caprice32(&f.profile, &f.media),
        Err(Caprice32Error::DisposableSessionRequired)
    ));
}

#[test]
fn changed_removed_or_replaced_firmware_blocks_spawn() {
    for change in 0..3 {
        let f = Fixture::new(Caprice32MediaFormat::Dsk);
        let prepared = f.prepare();
        let rom = f.rom("cpc6128.rom");
        match change {
            // Same size, different content.
            0 => std::fs::write(&rom, vec![0x43; 16384]).unwrap(),
            1 => std::fs::remove_file(&rom).unwrap(),
            _ => {
                std::fs::remove_file(&rom).unwrap();
                symlink(f.rom("amsdos.rom"), &rom).unwrap();
            }
        }
        assert!(prepared.spawn(&f.profile, &f.media).is_err(), "{change}");
    }
}

#[test]
fn oversized_and_unsupported_media_fail_closed() {
    let mut f = Fixture::new(Caprice32MediaFormat::Dsk);
    let bytes = vec![0u8; 32 * 1024 * 1024 + 1];
    std::fs::write(&f.media.path, &bytes).unwrap();
    f.media.verified_source_sha256 = Sha256::digest(&bytes).into();
    assert!(matches!(
        plan_caprice32(&f.profile, &f.media),
        Err(Caprice32Error::Sandbox(_))
    ));
    for format in [
        Caprice32MediaFormat::Cpr,
        Caprice32MediaFormat::Ipf,
        Caprice32MediaFormat::Sna,
        Caprice32MediaFormat::Unknown,
    ] {
        let f = Fixture::new(format);
        assert!(matches!(
            plan_caprice32(&f.profile, &f.media),
            Err(Caprice32Error::UnsupportedMedia(_))
        ));
    }
}

#[test]
fn profile_or_machine_change_fails_before_spawn() {
    let f = Fixture::new(Caprice32MediaFormat::Dsk);
    let prepared = f.prepare();
    let mut changed = f.profile.clone();
    changed.machine = Caprice32Machine::Cpc464;
    assert!(matches!(
        prepared.spawn(&changed, &f.media),
        Err(Caprice32Error::ProfileChanged)
    ));
}

#[test]
fn insufficient_space_fails_before_copy() {
    let f = Fixture::new(Caprice32MediaFormat::Dsk);
    let plan = f.plan();
    assert!(matches!(
        f.manager.prepare_with_test_capacity(&plan.sandbox, 1),
        Err(SafeLaunchSandboxError::InsufficientTemporarySpace { .. })
    ));
}

#[test]
fn executable_replacement_even_at_the_same_size_fails_before_spawn() {
    for same_size in [false, true] {
        let f = Fixture::new(Caprice32MediaFormat::Dsk);
        let prepared = f.prepare();
        let modified = std::fs::metadata(&f.profile.executable)
            .unwrap()
            .modified()
            .unwrap();
        let body: &[u8] = if same_size {
            b"#!/bin/sh\nexit 1\n"
        } else {
            b"replacement"
        };
        std::fs::write(&f.profile.executable, body).unwrap();
        if same_size {
            std::fs::File::options()
                .write(true)
                .open(&f.profile.executable)
                .unwrap()
                .set_modified(modified)
                .unwrap();
        }
        assert!(matches!(
            prepared.spawn(&f.profile, &f.media),
            Err(Caprice32Error::ProfileChanged)
        ));
    }
}

#[test]
fn seed_mismatch_even_at_the_same_length_is_refused() {
    let f = Fixture::new(Caprice32MediaFormat::Dsk);
    let mut bytes = std::fs::read(&f.profile.isolated_seed).unwrap();
    let at = bytes.len() - 3;
    bytes[at] ^= 1;
    std::fs::write(&f.profile.isolated_seed, &bytes).unwrap();
    assert!(matches!(
        plan_caprice32(&f.profile, &f.media),
        Err(Caprice32Error::IsolatedProfileRequired)
    ));
    std::fs::write(&f.profile.isolated_seed, b"user config").unwrap();
    assert!(matches!(
        plan_caprice32(&f.profile, &f.media),
        Err(Caprice32Error::IsolatedProfileRequired)
    ));
}

#[test]
fn a_launch_writes_only_scratch_and_cleans_up_and_the_source_never_changes() {
    let f = Fixture::new(Caprice32MediaFormat::Dsk);
    // Stand-in emulator: scribbles on the disk, the config and every state
    // location the real emulator would use, then exits.
    std::fs::write(
        &f.profile.executable,
        "#!/bin/sh\nprintf x > \"$3\"\nprintf y > state/snapshot.sna\nprintf z > \"$HOME/.cap32.cfg\"\nprintf d > debug.txt\n",
    )
    .unwrap();
    let source_before = std::fs::read(&f.media.path).unwrap();
    let seed_before = std::fs::read(&f.profile.isolated_seed).unwrap();
    let plan = plan_caprice32(&f.profile, &f.media).unwrap();
    let prepared = plan.prepare(&f.manager, &f.profile, &f.media).unwrap();
    let workspace = prepared.workspace_path().to_owned();
    let mut process = prepared.spawn(&f.profile, &f.media).unwrap();
    assert!(wait(&mut process).status.as_ref().unwrap().success());
    assert_eq!(process.cleanup_outcome(), CleanupOutcome::Removed);
    assert!(!workspace.exists());
    assert_eq!(std::fs::read(&f.media.path).unwrap(), source_before);
    assert_eq!(
        std::fs::read(&f.profile.isolated_seed).unwrap(),
        seed_before
    );
    // Nothing leaked beside the install or into the library directory.
    let leaked: Vec<_> = std::fs::read_dir(f.temp.path())
        .unwrap()
        .chain(std::fs::read_dir(f.temp.path().join("install")).unwrap())
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    for name in &leaked {
        assert!(
            ["install", "seed.cfg", "emuwiz", "cap32", "rom"].contains(&name.as_str())
                || name.starts_with("ambiguous-name"),
            "unexpected entry {name}: {leaked:?}"
        );
    }
}

/// Runs the REAL Caprice32 on the adapter's prepared command, headless. Skipped
/// unless a regular `cap32` executable with a `rom/` directory beside it is at
/// the known install location. Only synthetic media is used; the ROMs are the
/// ones that ship with the emulator, read-only.
#[test]
fn real_caprice32_reads_scratch_media_and_the_explicit_config_and_writes_nothing_outside() {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let executable = home.join("Applications/emulators/caprice32/cap32");
    if executable_binding(&executable).is_err()
        || !executable
            .parent()
            .unwrap()
            .join("rom/cpc6128.rom")
            .is_file()
    {
        return;
    }
    let mut f = Fixture::new(Caprice32MediaFormat::Dsk);
    f.profile.executable = executable.clone();
    let user_cfg = home.join(".cap32.cfg");
    let user_cfg_existed = user_cfg.exists();
    let install_before = {
        let dir = executable.parent().unwrap();
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name())
            .collect();
        names.sort();
        names
    };
    let plan = plan_caprice32(&f.profile, &f.media).unwrap();
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
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(3000));
    let running = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    let mut stdout = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut stdout);
    }
    let _ = child.wait();
    if !running {
        return; // no usable headless video on this host
    }
    // The emulator announced the SCRATCH config, not a user/system fallback.
    assert!(
        stdout.contains(&format!(
            "Using configuration file: {}",
            workspace.join("config/caprice32.cfg").display()
        )),
        "{stdout}"
    );
    assert_eq!(user_cfg.exists(), user_cfg_existed, "user config created");
    let install_after = {
        let dir = executable.parent().unwrap();
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name())
            .collect();
        names.sort();
        names
    };
    assert_eq!(
        install_before, install_after,
        "files appeared in the install dir"
    );
    drop(prepared);
    assert!(!workspace.exists());
}
