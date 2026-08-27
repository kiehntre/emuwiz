//! Tests for native DuckStation launch execution.
//!
//! The execution tests cover the native command and its fresh, authoritative
//! identity revalidation without duplicating the identity reader.
//!
//! What is provable and tested here:
//! - [`spawn_duckstation`] and the shared [`crate::launch::process_spawn`]
//!   watcher mechanics, exercised directly against a hand-built
//!   [`DuckStationCommand`] (spawning never touches identity at all).
//! - [`preflight_duckstation_launch`] revalidates a real PS1 disc's serial
//!   before producing a command.

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
    let executable_name = b"SLUS_123.45;1";
    let executable_record = directory_record(executable_name, 22, 12, false);
    let executable_record_offset = root_offset + cnf_record.len();
    iso[executable_record_offset..executable_record_offset + executable_record.len()]
        .copy_from_slice(&executable_record);
    let executable_offset = 22 * ISO_SECTOR_SIZE;
    iso[executable_offset..executable_offset + 12].copy_from_slice(b"PS-X EXE\0\0\0\0");
    iso
}

/// Wraps `ps1_iso_bytes()`-shaped ISO9660 content into a genuine,
/// `open_chd_track_logical_media`-openable uncompressed CHD v5 file. Mirrors
/// `crate::game_identity::tests::ps1_chd` (which cannot be imported across
/// module boundaries, being private to that module's own test mod) - not a
/// second CHD reader, only a second CHD test-fixture writer.
fn ps1_chd_bytes(image: &[u8]) -> Vec<u8> {
    use crate::dat::archive::chd::CHD_MAGIC;
    use crate::raw_cd_sector::{LOGICAL_BLOCK_BYTES, MODE1_USER_DATA_OFFSET, RAW_SECTOR_BYTES};

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }
    fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
    }

    let mut image = image.to_vec();
    let pvd = 16 * LOGICAL_BLOCK_BYTES;
    image[pvd + 128..pvd + 130].copy_from_slice(&(LOGICAL_BLOCK_BYTES as u16).to_le_bytes());
    image[pvd + 130..pvd + 132].copy_from_slice(&(LOGICAL_BLOCK_BYTES as u16).to_be_bytes());

    let sectors: Vec<[u8; RAW_SECTOR_BYTES]> = image
        .chunks(LOGICAL_BLOCK_BYTES)
        .map(|block| {
            let mut sector = [0u8; RAW_SECTOR_BYTES];
            sector[MODE1_USER_DATA_OFFSET..MODE1_USER_DATA_OFFSET + block.len()]
                .copy_from_slice(block);
            sector
        })
        .collect();
    let frames = sectors.len() as u32;
    let frames_per_hunk = frames.max(1);
    let unit_bytes = RAW_SECTOR_BYTES as u32;
    let hunk_bytes = unit_bytes * frames_per_hunk;
    let logical_bytes = frames as u64 * unit_bytes as u64;

    let mut data = vec![0u8; 124];
    data[0..8].copy_from_slice(CHD_MAGIC);
    put_u32(&mut data, 8, 124);
    put_u32(&mut data, 12, 5);
    put_u64(&mut data, 32, logical_bytes);
    put_u32(&mut data, 56, hunk_bytes);
    put_u32(&mut data, 60, unit_bytes);

    let meta_offset = data.len() as u64;
    let payload = format!(
        "TRACK:1 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:{frames} PREGAP:0 PGTYPE:NONE PGSUB:NONE POSTGAP:0"
    )
    .into_bytes();
    data.extend_from_slice(&u32::from_be_bytes(*b"CHT2").to_be_bytes());
    data.push(0);
    let length = payload.len() as u32;
    data.extend_from_slice(&length.to_be_bytes()[1..]);
    data.extend_from_slice(&0u64.to_be_bytes());
    data.extend_from_slice(&payload);

    let hunk_count = logical_bytes.div_ceil(hunk_bytes as u64) as u32;
    let map_offset = data.len() as u64;
    let map_end = map_offset + hunk_count as u64 * 4;
    let hunk_data_start = map_end.div_ceil(hunk_bytes as u64).max(1) * hunk_bytes as u64;
    let base_index = hunk_data_start / hunk_bytes as u64;
    for index in 0..hunk_count {
        let value = (base_index + index as u64) as u32;
        data.extend_from_slice(&value.to_be_bytes());
    }

    data.resize(hunk_data_start as usize, 0);
    for sector in &sectors {
        data.extend_from_slice(sector);
    }

    put_u64(&mut data, 40, map_offset);
    put_u64(&mut data, 48, meta_offset);
    data
}

#[test]
fn fresh_identity_revalidates_a_real_ps1_chd() {
    let root = fixture_root("identity-revalidation-chd");
    let executable = root.join("bin/duckstation-qt");
    write_executable(&executable, b"#!/bin/sh\nexit 0\n");
    let profile_root = root.join("config/duckstation");
    std::fs::create_dir_all(&profile_root).unwrap();
    std::fs::write(
        profile_root.join("settings.ini"),
        b"[BIOS]\nBIOSFilename=scph1001.bin\n",
    )
    .unwrap();
    let content_path = root.join("games/game.chd");
    std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
    std::fs::write(&content_path, ps1_chd_bytes(&ps1_iso_bytes())).unwrap();

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
    // Identity itself must succeed for the CHD, exactly as it does for the
    // equivalent ISO above - reaching the later BindingUnavailable failure
    // (not IdentityUnresolved/Ps1SerialUnavailable) proves fresh CHD
    // identity revalidation, not just that the file was readable.
    let error = preflight_duckstation_launch(&request, &roots, &[]).unwrap_err();
    assert_eq!(
        error.kind,
        DuckStationLaunchPreflightErrorKind::BindingUnavailable
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn fresh_identity_rejects_a_mismatched_real_ps1_chd_serial() {
    let root = fixture_root("identity-mismatch-chd");
    let content_path = root.join("games/game.chd");
    std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
    std::fs::write(&content_path, ps1_chd_bytes(&ps1_iso_bytes())).unwrap();
    let request = DuckStationLaunchRequest {
        selected_content_path: content_path,
        expected_platform_id: "PSX".to_string(),
        expected_game_key: "SLUS-12345".to_string(),
        expected_ps1_serial: "SLES-23456".to_string(),
        profile_id: "duckstation:test".to_string(),
        expected_executable: root.join("duckstation-qt"),
        expected_user_directory_mode: DuckStationUserDirectoryMode::DefaultNative,
    };

    let error = fresh_identity_status(&request.selected_content_path, &request).unwrap_err();
    assert_eq!(
        error.kind,
        DuckStationLaunchPreflightErrorKind::Ps1SerialMismatch
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn fresh_identity_revalidates_a_real_ps1_disc() {
    // The fixture is a directly inspectable PS1 image; this test isolates the
    // fresh identity revalidation stage from later profile-binding checks.
    let root = fixture_root("identity-revalidation");
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
        DuckStationLaunchPreflightErrorKind::BindingUnavailable
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn fresh_identity_rejects_a_mismatched_real_ps1_serial() {
    let root = fixture_root("identity-mismatch");
    let content_path = root.join("games/game.iso");
    std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
    std::fs::write(&content_path, ps1_iso_bytes()).unwrap();
    let request = DuckStationLaunchRequest {
        selected_content_path: content_path,
        expected_platform_id: "PSX".to_string(),
        expected_game_key: "SLUS-12345".to_string(),
        expected_ps1_serial: "SLES-23456".to_string(),
        profile_id: "duckstation:test".to_string(),
        expected_executable: root.join("duckstation-qt"),
        expected_user_directory_mode: DuckStationUserDirectoryMode::DefaultNative,
    };

    let error = fresh_identity_status(&request.selected_content_path, &request).unwrap_err();
    assert_eq!(
        error.kind,
        DuckStationLaunchPreflightErrorKind::Ps1SerialMismatch
    );
    std::fs::remove_dir_all(root).unwrap();
}
