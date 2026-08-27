use std::path::PathBuf;
use std::time::Duration;

use super::*;
use crate::dreamcast_boot_evidence::IP_BIN_META_BYTES;
use crate::patch_manager::FlycastProfile;

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn fixture_root(label: &str) -> PathBuf {
    let sequence = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "archivefs-flycast-execution-{label}-{}-{sequence}",
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

/// A structurally valid, minimal Dreamcast ISO9660 image with a real
/// IP.BIN boot header at logical sector 0 - mirrors `game_identity.rs`'s
/// own `dreamcast_iso` test fixture (private to that module's own test
/// mod, so re-derived here rather than duplicated by reference), proving
/// the *content* this preflight is given is genuinely well-formed.
fn dreamcast_iso_bytes(product_code: &str) -> Vec<u8> {
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
    let mut ip_bin = vec![b' '; IP_BIN_META_BYTES];
    ip_bin[..16].copy_from_slice(b"SEGA SEGAKATANA ");
    ip_bin[0x10..0x20].copy_from_slice(b"SEGA ENTERPRISES");
    let product_bytes = product_code.as_bytes();
    let product_len = product_bytes.len().min(10);
    ip_bin[0x40..0x40 + product_len].copy_from_slice(&product_bytes[..product_len]);
    ip_bin[0x4a..0x50].copy_from_slice(b"V1.000");
    ip_bin[0x50..0x60].copy_from_slice(b"20000915        ");
    ip_bin[0x60..0x70].copy_from_slice(b"1ST_READ.BIN    ");
    iso[..IP_BIN_META_BYTES].copy_from_slice(&ip_bin);

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
    iso
}

fn flycast_roots(root: &PathBuf, executable: PathBuf) -> FlycastProfileDiscoveryRoots {
    // Deliberately not under `xdg_config_home`/`flycast` - that path is
    // also the `Native` candidate's own config root, and a colliding path
    // would dedup away this `Explicit` candidate in favour of `Native`
    // (`discover_flycast_profiles` dedups by config path), silently
    // changing which `FlycastInstallationType` the discovered profile ends
    // up as.
    let config = root.join("explicit-flycast-config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("emu.cfg"), "").unwrap();
    FlycastProfileDiscoveryRoots {
        home: root.join("home"),
        xdg_config_home: root.join("config"),
        xdg_data_home: root.join("data"),
        explicit_configuration_roots: vec![config],
        portable_configuration_roots: Vec::new(),
        explicit_executables: vec![executable],
        known_version_outputs: std::collections::BTreeMap::new(),
        appimage_directory: None,
    }
}

fn discovered_profile(roots: &FlycastProfileDiscoveryRoots) -> FlycastProfile {
    discover_flycast_profiles(roots)
        .profiles
        .into_iter()
        .find(|profile| profile.eligible)
        .expect("a discovered, eligible native profile")
}

#[test]
fn fresh_identity_revalidates_a_real_dreamcast_iso() {
    // Isolates the fresh identity revalidation stage from later
    // binding/profile checks, exactly like DuckStation's equivalent test.
    let root = fixture_root("identity-revalidation");
    let executable = root.join("bin/flycast");
    write_executable(&executable, b"#!/bin/sh\nexit 0\n");
    let content_path = root.join("games/game.iso");
    std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
    std::fs::write(&content_path, dreamcast_iso_bytes("T-8109N")).unwrap();

    let roots = flycast_roots(&root, executable.clone());
    let profile = discovered_profile(&roots);
    let request = FlycastLaunchRequest {
        selected_content_path: content_path,
        expected_platform_id: "Dreamcast".to_string(),
        expected_game_key: "T-8109N".to_string(),
        expected_dreamcast_product_code: "T-8109N".to_string(),
        profile_id: profile.profile_id.clone(),
        expected_executable: executable,
    };
    let error = fresh_identity_status(&request.selected_content_path, &request).unwrap();
    assert!(matches!(error.0, CanonicalIdentityStatus::Resolved(_)));
    assert_eq!(error.2, "T-8109N");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fresh_identity_revalidates_a_real_dreamcast_gdi() {
    // The GDI-specific counterpart to `fresh_identity_revalidates_a_real_
    // dreamcast_iso` above - proves a real `.gdi` descriptor (with its own
    // low-density and high-density tracks) reaches this same fresh
    // identity revalidation stage through `resolve_gdi_data_track`, one
    // level below the BIOS-gated full preflight (see this file's own
    // notes on why a full "reaches strict Ready" test needs real BIOS
    // bytes this repo never commits).
    let root = fixture_root("gdi-identity-revalidation");
    let executable = root.join("bin/flycast");
    write_executable(&executable, b"#!/bin/sh\nexit 0\n");
    let games = root.join("games");
    std::fs::create_dir_all(&games).unwrap();
    std::fs::write(games.join("track01.bin"), vec![0_u8; 2352]).unwrap();
    std::fs::write(games.join("track02.raw"), vec![0_u8; 2352]).unwrap();
    std::fs::write(games.join("track03.iso"), dreamcast_iso_bytes("T-8109N")).unwrap();
    let content_path = games.join("game.gdi");
    std::fs::write(
        &content_path,
        "3\n\
         1 0 4 2352 track01.bin 0\n\
         2 600 0 2352 track02.raw 0\n\
         3 45000 4 2048 track03.iso 0\n",
    )
    .unwrap();

    let roots = flycast_roots(&root, executable.clone());
    let profile = discovered_profile(&roots);
    let request = FlycastLaunchRequest {
        selected_content_path: content_path,
        expected_platform_id: "Dreamcast".to_string(),
        expected_game_key: "T-8109N".to_string(),
        expected_dreamcast_product_code: "T-8109N".to_string(),
        profile_id: profile.profile_id.clone(),
        expected_executable: executable,
    };
    let result = fresh_identity_status(&request.selected_content_path, &request).unwrap();
    assert!(matches!(result.0, CanonicalIdentityStatus::Resolved(_)));
    assert_eq!(result.2, "T-8109N");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fresh_identity_rejects_a_mismatched_real_product_code() {
    let root = fixture_root("identity-mismatch");
    let content_path = root.join("games/game.iso");
    std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
    std::fs::write(&content_path, dreamcast_iso_bytes("T-8109N")).unwrap();
    let request = FlycastLaunchRequest {
        selected_content_path: content_path,
        expected_platform_id: "Dreamcast".to_string(),
        expected_game_key: "T-8109N".to_string(),
        expected_dreamcast_product_code: "MK-51053".to_string(),
        profile_id: "test".to_string(),
        expected_executable: root.join("flycast"),
    };

    let error = fresh_identity_status(&request.selected_content_path, &request).unwrap_err();
    assert_eq!(
        error.kind,
        FlycastLaunchPreflightErrorKind::DreamcastProductCodeMismatch
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn filename_disagreement_is_irrelevant() {
    let root = fixture_root("filename-disagreement");
    let content_path = root.join("games/Totally Unrelated Name.iso");
    std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
    std::fs::write(&content_path, dreamcast_iso_bytes("T-8109N")).unwrap();
    let request = FlycastLaunchRequest {
        selected_content_path: content_path,
        expected_platform_id: "Dreamcast".to_string(),
        expected_game_key: "T-8109N".to_string(),
        expected_dreamcast_product_code: "T-8109N".to_string(),
        profile_id: "test".to_string(),
        expected_executable: root.join("flycast"),
    };

    let (status, _facts, product_code) =
        fresh_identity_status(&request.selected_content_path, &request).unwrap();
    assert!(matches!(status, CanonicalIdentityStatus::Resolved(_)));
    assert_eq!(product_code, "T-8109N");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn malformed_dreamcast_media_fails_closed() {
    let root = fixture_root("malformed-media");
    let content_path = root.join("games/game.iso");
    std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
    // Valid ISO9660 volume descriptor, but no recognisable IP.BIN boot
    // signature at all.
    std::fs::write(&content_path, vec![0xAB_u8; 24 * 2048]).unwrap();
    let request = FlycastLaunchRequest {
        selected_content_path: content_path,
        expected_platform_id: "Dreamcast".to_string(),
        expected_game_key: "T-8109N".to_string(),
        expected_dreamcast_product_code: "T-8109N".to_string(),
        profile_id: "test".to_string(),
        expected_executable: root.join("flycast"),
    };

    let error = fresh_identity_status(&request.selected_content_path, &request).unwrap_err();
    assert_eq!(
        error.kind,
        FlycastLaunchPreflightErrorKind::IdentityUnresolved
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn cdi_is_refused_at_the_content_gate() {
    let root = fixture_root("cdi-refused");
    for extension in ["cdi"] {
        let content_path = root.join(format!("games/game.{extension}"));
        std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
        std::fs::write(&content_path, dreamcast_iso_bytes("T-8109N")).unwrap();
        let error = inspect_and_capture_content_identity(&content_path).unwrap_err();
        assert_eq!(
            error.kind,
            FlycastLaunchPreflightErrorKind::ContentFormatUnsupported,
            "{extension} must be refused"
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn gdi_passes_the_content_gate() {
    let root = fixture_root("gdi-content-gate");
    let content_path = root.join("games/game.gdi");
    std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
    std::fs::write(&content_path, b"1\n1 45000 4 2352 track01.bin 0\n").unwrap();
    // The content-format gate only inspects the extension/mount-input
    // classification, not the descriptor's own contents.
    assert!(inspect_and_capture_content_identity(&content_path).is_ok());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn no_profile_refuses_cleanly() {
    let root = fixture_root("no-profile");
    let content_path = root.join("games/game.iso");
    std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
    std::fs::write(&content_path, dreamcast_iso_bytes("T-8109N")).unwrap();
    let roots = FlycastProfileDiscoveryRoots {
        home: root.join("home"),
        xdg_config_home: root.join("config"),
        xdg_data_home: root.join("data"),
        explicit_configuration_roots: Vec::new(),
        portable_configuration_roots: Vec::new(),
        explicit_executables: Vec::new(),
        known_version_outputs: std::collections::BTreeMap::new(),
        appimage_directory: None,
    };
    let request = FlycastLaunchRequest {
        selected_content_path: content_path,
        expected_platform_id: "Dreamcast".to_string(),
        expected_game_key: "T-8109N".to_string(),
        expected_dreamcast_product_code: "T-8109N".to_string(),
        profile_id: "does-not-exist".to_string(),
        expected_executable: root.join("flycast"),
    };

    let error = preflight_flycast_launch(&request, &roots).unwrap_err();
    assert_eq!(error.kind, FlycastLaunchPreflightErrorKind::ProfileNotFound);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn missing_executable_refuses_via_binding_unavailable() {
    let root = fixture_root("missing-executable");
    let content_path = root.join("games/game.iso");
    std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
    std::fs::write(&content_path, dreamcast_iso_bytes("T-8109N")).unwrap();
    let executable = root.join("bin/flycast");
    // Deliberately never written to disk.
    let roots = flycast_roots(&root, executable.clone());
    let profile = discovered_profile(&roots);
    let request = FlycastLaunchRequest {
        selected_content_path: content_path,
        expected_platform_id: "Dreamcast".to_string(),
        expected_game_key: "T-8109N".to_string(),
        expected_dreamcast_product_code: "T-8109N".to_string(),
        profile_id: profile.profile_id,
        expected_executable: executable,
    };

    let error = preflight_flycast_launch(&request, &roots).unwrap_err();
    assert_eq!(
        error.kind,
        FlycastLaunchPreflightErrorKind::BindingUnavailable
    );
    let _ = std::fs::remove_dir_all(root);
}

/// `inspect_flycast_game`'s own `dc_boot.bin` check (`flycast_local.rs`'s
/// `state()`) only ever produces `PresentUnverified`/`Missing`/`Unreadable`
/// for the Dreamcast BIOS field - never `Unknown`/`NotConfigured` (those
/// two are reachable only for the separate `arcade_system_roms` field).
/// A directory at the expected BIOS path deterministically produces
/// `Unreadable` (`state()`'s `Ok(_) if !is_file() => Unreadable` arm) without
/// relying on filesystem permissions, which would not be portable across test
/// environments. It must not bypass the strict firmware gate.
fn make_bios_state_unreadable(profile: &FlycastProfile) {
    std::fs::create_dir_all(profile.system_path.join("dc_boot.bin")).unwrap();
}

#[test]
fn unreadable_bios_does_not_reach_a_real_command() {
    let root = fixture_root("successful-preflight");
    let executable = root.join("bin/flycast");
    write_executable(&executable, b"#!/bin/sh\nexit 0\n");
    let content_path = root.join("games/game.iso");
    std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
    std::fs::write(&content_path, dreamcast_iso_bytes("T-8109N")).unwrap();

    let roots = flycast_roots(&root, executable.clone());
    let profile = discovered_profile(&roots);
    make_bios_state_unreadable(&profile);
    let request = FlycastLaunchRequest {
        selected_content_path: content_path.clone(),
        expected_platform_id: "Dreamcast".to_string(),
        expected_game_key: "T-8109N".to_string(),
        expected_dreamcast_product_code: "T-8109N".to_string(),
        profile_id: profile.profile_id,
        expected_executable: executable.clone(),
    };

    let error = preflight_flycast_launch(&request, &roots).unwrap_err();
    assert_eq!(
        error.kind,
        FlycastLaunchPreflightErrorKind::CandidateNotReady
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Honest regression pin for a known limitation: because
/// A present but unknown Dreamcast BIOS remains unverified and keeps this
/// preflight's strict-`Ready` gate closed.
#[test]
fn present_but_unverified_bios_does_not_reach_strict_ready() {
    let root = fixture_root("unverified-bios");
    let executable = root.join("bin/flycast");
    write_executable(&executable, b"#!/bin/sh\nexit 0\n");
    let content_path = root.join("games/game.iso");
    std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
    std::fs::write(&content_path, dreamcast_iso_bytes("T-8109N")).unwrap();

    let roots = flycast_roots(&root, executable.clone());
    let profile = discovered_profile(&roots);
    std::fs::create_dir_all(&profile.system_path).unwrap();
    std::fs::write(profile.system_path.join("dc_boot.bin"), b"bios").unwrap();
    let request = FlycastLaunchRequest {
        selected_content_path: content_path,
        expected_platform_id: "Dreamcast".to_string(),
        expected_game_key: "T-8109N".to_string(),
        expected_dreamcast_product_code: "T-8109N".to_string(),
        profile_id: profile.profile_id,
        expected_executable: executable,
    };

    let error = preflight_flycast_launch(&request, &roots).unwrap_err();
    assert_eq!(
        error.kind,
        FlycastLaunchPreflightErrorKind::CandidateNotReady
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn spawn_produces_a_real_process_and_exact_argv() {
    let root = fixture_root("spawn");
    let capture_path = root.join("argv-capture.txt");
    let executable = root.join("bin/flycast");
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

    let command = FlycastCommand {
        executable,
        arguments: vec![content_path.clone().into_os_string()],
        working_directory: None,
        selection: crate::launch::flycast_command::FlycastCommandSelection {
            profile_id: "test".to_string(),
            platform_id: "Dreamcast".to_string(),
            verified_dreamcast_product_code: "T-8109N".to_string(),
            content_path: content_path.clone(),
        },
    };
    let mut process = spawn_flycast(command).expect("the fake script must spawn");
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
    let captured = std::fs::read_to_string(&capture_path).unwrap();
    assert_eq!(captured.trim(), content_path.to_str().unwrap());
    let _ = std::fs::remove_dir_all(root);
}
