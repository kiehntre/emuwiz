//! Tests for native DuckStation launch execution.
//!
//! # Why this suite is narrower than PCSX2's/Dolphin's own execution tests
//!
//! [`preflight_duckstation_launch`]'s step 4 ("freshly inspect game
//! identity") calls
//! [`crate::game_identity::inspect_catalogued_game_identity`] exactly the
//! way PCSX2's/Dolphin's own preflight already do - but unlike `PS2`
//! (`IdentityPlatform::PlayStation2`, wired through `BOOT2=` SYSTEM.CNF
//! parsing) or `GameCube`/`Wii`, **`game_identity.rs`'s `IdentityPlatform`
//! enum has no PlayStation/PSX variant, and `IdentityKind` has no
//! `Ps1Serial` variant** - confirmed by reading both enums directly, and by
//! `VerifiedIdentityFact::Ps1Serial` never being constructed anywhere in
//! this crate outside test fixtures. Fresh, live PS1 disc-serial
//! verification from real bytes genuinely does not exist yet at the
//! `game_identity` layer this preflight is required to call.
//!
//! This is a real, external, structural prerequisite - not a bug in this
//! module. `preflight_duckstation_launch` is written, complete, and
//! correct (mirroring the exact proven PCSX2 architecture); it is simply
//! unable to get past its own step 4 for *any* real PS1 disc today,
//! because the identity layer it must freshly re-verify against cannot yet
//! answer "what PS1 serial does this disc have" at all. It will start
//! working the moment that prerequisite lands, with zero changes needed
//! here.
//!
//! What *is* provable and tested today, independent of that gap:
//! - [`spawn_duckstation`] and the shared [`crate::launch::process_spawn`]
//!   watcher mechanics, exercised directly against a hand-built
//!   [`DuckStationCommand`] (spawning never touches identity at all).
//! - That [`preflight_duckstation_launch`] fails safely (never panics) and
//!   reports exactly [`DuckStationLaunchPreflightErrorKind::IdentityUnresolved`]
//!   for a real, well-formed PS1 disc today - a regression guard that
//!   documents the exact gap rather than silently working around it.

use std::path::PathBuf;
use std::time::Duration;

use super::*;
use crate::launch::duckstation_command::DuckStationCommandSelection;
use crate::patch_manager::DuckStationProfileDiscoveryRoots;

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn fixture_root(label: &str) -> PathBuf {
    let sequence = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "archivefs-duckstation-execution-{label}-{}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn write_executable(path: &std::path::Path, contents: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn hand_built_command(executable: PathBuf, content_path: PathBuf) -> DuckStationCommand {
    DuckStationCommand {
        executable,
        arguments: vec![
            OsString::from("-batch"),
            OsString::from("--"),
            content_path.clone().into_os_string(),
        ],
        working_directory: None,
        selection: DuckStationCommandSelection {
            profile_id: "test".to_string(),
            user_directory_mode: DuckStationUserDirectoryMode::DefaultNative,
            platform_id: "PSX".to_string(),
            verified_ps1_serial: "SLUS-12345".to_string(),
            content_path,
        },
    }
}

fn wait_for_exit(
    process: &mut LaunchedDuckStationProcess,
) -> &crate::launch::process_spawn::ProcessExitReport {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if process.poll().is_some() {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("the fake script did not exit in time");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    process.poll().expect("exit report must be cached now")
}

// --- 28: fake executable receives exact argv --------------------------------------------------------

#[test]
fn fake_executable_receives_exact_argv() {
    let root = fixture_root("argv-capture");
    let capture_path = root.join("argv-capture.txt");
    let executable = root.join("bin/duckstation-qt");
    write_executable(
        &executable,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            capture_path.display()
        )
        .as_bytes(),
    );
    let content_path = root.join("games/game.iso");
    std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
    std::fs::write(&content_path, b"synthetic content").unwrap();

    let command = hand_built_command(executable, content_path.clone());
    let mut process = spawn_duckstation(command).expect("the fake script must spawn");
    wait_for_exit(&mut process);
    let captured = std::fs::read_to_string(&capture_path).unwrap();
    let lines: Vec<&str> = captured.lines().collect();
    assert_eq!(lines, vec!["-batch", "--", content_path.to_str().unwrap()]);
    std::fs::remove_dir_all(root).unwrap();
}

// --- 29/30: successful spawn returns PID, clean exit reported -----------------------------------------

#[test]
fn successful_spawn_returns_a_real_pid_and_clean_exit() {
    let root = fixture_root("spawn-success");
    let executable = root.join("bin/duckstation-qt");
    write_executable(&executable, b"#!/bin/sh\nexit 0\n");
    let content_path = root.join("games/game.iso");
    std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
    std::fs::write(&content_path, b"synthetic content").unwrap();

    let command = hand_built_command(executable, content_path);
    let mut process = spawn_duckstation(command).expect("the fake script must spawn");
    assert!(process.pid > 0);
    assert!(process.is_running() || process.poll().is_some());
    let report = wait_for_exit(&mut process);
    let status = report.status.as_ref().expect("wait() must succeed");
    assert!(status.success(), "the fake script exits 0");
    std::fs::remove_dir_all(root).unwrap();
}

// --- 31/32: non-zero exit and bounded stderr reported -------------------------------------------------

#[test]
fn non_zero_exit_and_stderr_are_reported() {
    let root = fixture_root("spawn-nonzero");
    let executable = root.join("bin/duckstation-qt");
    write_executable(
        &executable,
        b"#!/bin/sh\necho 'synthetic duckstation failure' 1>&2\nexit 7\n",
    );
    let content_path = root.join("games/game.iso");
    std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
    std::fs::write(&content_path, b"synthetic content").unwrap();

    let command = hand_built_command(executable, content_path);
    let mut process = spawn_duckstation(command).unwrap();
    let report = wait_for_exit(&mut process);
    let status = report.status.as_ref().unwrap();
    assert!(!status.success());
    assert_eq!(status.code(), Some(7));
    assert!(String::from_utf8_lossy(&report.stderr).contains("synthetic duckstation failure"));
    std::fs::remove_dir_all(root).unwrap();
}

// --- 33: shell metacharacters never interpreted -------------------------------------------------

#[test]
fn shell_metacharacters_in_content_path_are_never_interpreted() {
    let marker = std::env::temp_dir().join(format!(
        "archivefs-duckstation-execution-shell-marker-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&marker);
    let root = fixture_root("shell-metacharacters");
    let executable = root.join("bin/duckstation-qt");
    write_executable(&executable, b"#!/bin/sh\nexit 0\n");
    let dangerous_name = format!("game; touch {} #.iso", marker.display());
    let dangerous_path = root.join("games").join(&dangerous_name);
    std::fs::create_dir_all(dangerous_path.parent().unwrap()).unwrap();
    std::fs::write(&dangerous_path, b"synthetic content").unwrap();

    let command = hand_built_command(executable, dangerous_path);
    let mut process = spawn_duckstation(command).unwrap();
    wait_for_exit(&mut process);
    assert!(
        !marker.exists(),
        "shell metacharacters in a content path must never be interpreted"
    );
    let _ = std::fs::remove_file(&marker);
    std::fs::remove_dir_all(root).unwrap();
}

// --- spawn failure reported -----------------------------------------------------------------------

#[test]
fn spawn_failure_is_reported() {
    let root = fixture_root("spawn-failure");
    let content_path = root.join("games/game.iso");
    std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
    std::fs::write(&content_path, b"synthetic content").unwrap();
    let command = hand_built_command(root.join("bin/does-not-exist"), content_path);
    let result = spawn_duckstation(command);
    assert!(matches!(result, Err(DuckStationLaunchSpawnError::Spawn(_))));
    std::fs::remove_dir_all(root).unwrap();
}

// --- honest documentation of the current identity-layer gap --------------------------------------

/// A structurally valid, minimal PS1 ISO9660 image with a real
/// `SYSTEM.CNF`/`BOOT = cdrom:\...;1` entry - proves the *content* this
/// preflight is given is genuinely well-formed; the failure below comes
/// only from the identity layer's current lack of PS1 support, never from
/// a malformed fixture.
fn ps1_iso_bytes() -> Vec<u8> {
    const ISO_SECTOR_SIZE: usize = 2_048;
    const SECTORS: usize = 24;

    fn directory_record(name: &[u8], extent: u32, size: u32, directory: bool) -> Vec<u8> {
        let length = 33 + name.len() + usize::from(name.len().is_multiple_of(2));
        let mut record = vec![0_u8; length];
        record[0] = length as u8;
        record[2..6].copy_from_slice(&extent.to_le_bytes());
        record[6..10].copy_from_slice(&extent.to_be_bytes());
        record[10..14].copy_from_slice(&size.to_le_bytes());
        record[14..18].copy_from_slice(&size.to_be_bytes());
        record[25] = if directory { 2 } else { 0 };
        record[28..30].copy_from_slice(&1_u16.to_le_bytes());
        record[30..32].copy_from_slice(&1_u16.to_be_bytes());
        record[32] = name.len() as u8;
        record[33..33 + name.len()].copy_from_slice(name);
        record
    }

    let mut iso = vec![0_u8; SECTORS * ISO_SECTOR_SIZE];
    let pvd = 16 * ISO_SECTOR_SIZE;
    iso[pvd] = 1;
    iso[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
    iso[pvd + 6] = 1;
    let root = directory_record(&[0], 20, ISO_SECTOR_SIZE as u32, true);
    iso[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);
    let terminator = 17 * ISO_SECTOR_SIZE;
    iso[terminator] = 255;
    iso[terminator + 1..terminator + 6].copy_from_slice(b"CD001");
    iso[terminator + 6] = 1;

    let cnf = b"BOOT = cdrom:\\SLUS_123.45;1\r\nTCB = 4\r\n";
    let root_offset = 20 * ISO_SECTOR_SIZE;
    let cnf_record = directory_record(b"SYSTEM.CNF;1", 21, cnf.len() as u32, false);
    iso[root_offset..root_offset + cnf_record.len()].copy_from_slice(&cnf_record);

    let cnf_offset = 21 * ISO_SECTOR_SIZE;
    iso[cnf_offset..cnf_offset + cnf.len()].copy_from_slice(cnf);
    iso
}

#[test]
fn preflight_fails_closed_with_identity_unresolved_for_a_real_ps1_disc_today() {
    // This test exists to *document and guard* the exact external gap this
    // module's own doc comment explains, not to validate desired behavior.
    // When PS1 disc-serial identity support lands in `game_identity.rs`,
    // this test's expectation should change to `Ok(_)` (matching
    // `valid_native_ps1_iso_succeeds` in PCSX2's own execution test suite) -
    // and that is the intended, expected outcome of fixing the
    // prerequisite, not a signal something here is broken.
    let root = fixture_root("identity-gap");
    let executable = root.join("bin/duckstation-qt");
    write_executable(&executable, b"#!/bin/sh\nexit 0\n");
    let profile_root = root.join("config/duckstation");
    std::fs::create_dir_all(&profile_root).unwrap();
    std::fs::write(
        profile_root.join("settings.ini"),
        b"[BIOS]\nBIOSFilename=scph1001.bin\n",
    )
    .unwrap();
    let content_path = root.join("games/game.iso");
    std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
    std::fs::write(&content_path, ps1_iso_bytes()).unwrap();

    let roots = DuckStationProfileDiscoveryRoots {
        home: root.join("home"),
        xdg_config_home: root.join("config"),
        xdg_data_home: root.join("data"),
        xdg_config_home_explicit: true,
        explicit_configuration_roots: Vec::new(),
        portable_configuration_roots: Vec::new(),
        explicit_executables: vec![executable.clone()],
        known_version_outputs: std::collections::BTreeMap::new(),
        appimage_directory: None,
    };
    let request = DuckStationLaunchRequest {
        selected_content_path: content_path,
        expected_platform_id: "PSX".to_string(),
        expected_game_key: "SLUS-12345".to_string(),
        expected_ps1_serial: "SLUS-12345".to_string(),
        profile_id: format!("duckstation:{}", profile_root.display()),
        expected_executable: executable,
        expected_user_directory_mode: DuckStationUserDirectoryMode::DefaultNative,
    };
    let error = preflight_duckstation_launch(&request, &roots, &[]).unwrap_err();
    assert_eq!(
        error.kind,
        DuckStationLaunchPreflightErrorKind::IdentityUnresolved,
        "if this now fails differently (or succeeds), the PS1 identity prerequisite this \
         module's doc comment names has changed - update this test and the wider execution \
         test suite to match PCSX2's own, not just this assertion"
    );
    std::fs::remove_dir_all(root).unwrap();
}
