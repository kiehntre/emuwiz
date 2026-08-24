//! Tests for native PCSX2 launch preflight/execution.
//!
//! Every fixture is a real temp directory on disk (a fake `pcsx2-qt` shell
//! script, a real `PCSX2.ini`, a synthetic but structurally valid PS2 ISO9660
//! image with a `SYSTEM.CNF`/`BOOT2` entry) driven through the exact same
//! real discovery/identity/planning functions production code uses -
//! `discover_pcsx2_profiles`, `resolve_pcsx2_native_launch_binding`,
//! `inspect_catalogued_game_identity`, `build_launch_plan_from_results`,
//! `build_pcsx2_command_plan` - never a shortcut or a mocked plan. Spawn
//! tests genuinely `fork`/`exec` the fake script; no real installed PCSX2 is
//! required anywhere.
//!
//! `roots.explicit_executables` (not real `PATH`) is what lets these tests
//! deterministically control which executable resolves as `Native` -
//! `resolve_pcsx2_native_launch_binding` only ever authorizes `Native`
//! installs, and this suite never touches the real host `PATH`/environment
//! to prove one.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::*;
use crate::dat::firmware_evidence::{FirmwareIdentityRecord, FirmwareSystem};
use crate::dat::model::DatEcosystem;
use crate::identity_source::hashing::Crc32;
use crate::launch::process_spawn::CapturedFileIdentity;
use crate::patch_manager::Pcsx2ProfileDiscoveryRoots;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "archivefs-pcsx2-execution-{label}-{}-{sequence}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn write(&self, relative: &str, contents: &[u8]) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }

    fn write_executable(&self, relative: &str, contents: &[u8]) -> PathBuf {
        let path = self.write(relative, contents);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

const ISO_SECTOR_SIZE: usize = 2_048;

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

/// A structurally valid, minimal PS2 ISO9660 image: a primary volume
/// descriptor, a volume descriptor set terminator, a root directory holding
/// one `SYSTEM.CNF;1` entry, and that file's own `BOOT2 = cdrom0:\...;1`
/// content - the only bytes `inspect_catalogued_game_identity` ever reads
/// to verify a PS2 serial. `serial_from_boot_path` turns `SLUS_123.45` into
/// `SLUS-12345`.
fn ps2_iso_bytes() -> Vec<u8> {
    const SECTORS: usize = 24;
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

    let cnf = b"VER = 1.00\r\nBOOT2 = cdrom0:\\SLUS_123.45;1\r\n";
    let root_offset = 20 * ISO_SECTOR_SIZE;
    let cnf_record = directory_record(b"SYSTEM.CNF;1", 21, cnf.len() as u32, false);
    iso[root_offset..root_offset + cnf_record.len()].copy_from_slice(&cnf_record);

    let cnf_offset = 21 * ISO_SECTOR_SIZE;
    iso[cnf_offset..cnf_offset + cnf.len()].copy_from_slice(cnf);
    iso
}

const PS2_SERIAL: &str = "SLUS-12345";

fn base_roots(fixture: &Fixture) -> Pcsx2ProfileDiscoveryRoots {
    Pcsx2ProfileDiscoveryRoots {
        home: fixture.path("home"),
        xdg_config_home: fixture.path("config"),
        xdg_data_home: fixture.path("data"),
        documents_home: fixture.path("home/Documents"),
        flatpak_system_root: fixture.path("system-flatpak"),
        appimage_directory: None,
        portable_configuration_roots: Vec::new(),
        explicit_executables: Vec::new(),
    }
}

/// A fully wired, genuinely `Ready` native PCSX2 fixture: a fake executable
/// script whitelisted via `roots.explicit_executables`, a real `PCSX2.ini`
/// at PCSX2's own default XDG root, a loose PS2 ISO whose verified serial
/// becomes the request's expected serial/game key - computed via the same
/// `inspect_catalogued_game_identity` the module itself uses, never
/// hand-typed - and a real BIOS file whose bytes genuinely match one
/// [`FirmwareIdentityRecord`] in `firmware_evidence`, so
/// `Pcsx2BiosVerification::Verified` is reached honestly rather than
/// through an unrelated "uncertain" branch.
struct ReadyFixture {
    fixture: Fixture,
    roots: Pcsx2ProfileDiscoveryRoots,
    profile_root: PathBuf,
    request: Pcsx2LaunchRequest,
    firmware_evidence: Vec<FirmwareIdentityRecord>,
}

/// Synthetic BIOS bytes for this suite only - never a real PS2 BIOS dump,
/// never a real Redump-published hash. `record_for_bios_bytes` computes its
/// own record hashes from these bytes with the exact same algorithms
/// `resolve_pcsx2_bios` uses, so the fixture is self-consistent without any
/// hand-typed digest.
const BIOS_BYTES: &[u8] = b"synthetic test BIOS bytes for pcsx2_execution fixtures only";

fn record_for_bios_bytes(name: &str) -> FirmwareIdentityRecord {
    use md5::Md5;
    use sha1::Sha1;
    use sha1::digest::Digest;
    let hex = |bytes: &[u8]| -> String { bytes.iter().map(|byte| format!("{byte:02x}")).collect() };
    FirmwareIdentityRecord {
        system: FirmwareSystem::PlayStation2,
        provider: DatEcosystem::Redump,
        name: name.to_string(),
        description: Some(format!("{name} description")),
        size_bytes: BIOS_BYTES.len() as u64,
        crc32: Crc32::of(BIOS_BYTES),
        md5: hex(&Md5::digest(BIOS_BYTES)),
        sha1: hex(&Sha1::digest(BIOS_BYTES)),
        dat_version: Some("20240101".to_string()),
    }
}

fn write_verified_bios(profile_root: &std::path::Path) {
    fs::create_dir_all(profile_root.join("bios")).unwrap();
    fs::write(profile_root.join("bios/scph-70012.bin"), BIOS_BYTES).unwrap();
}

fn build_ready_fixture(label: &str) -> ReadyFixture {
    let fixture = Fixture::new(label);
    let mut roots = base_roots(&fixture);
    let profile_root = roots.xdg_config_home.join("PCSX2");
    fs::create_dir_all(&profile_root).unwrap();
    fs::write(profile_root.join("PCSX2.ini"), b"[Filenames]\n").unwrap();
    write_verified_bios(&profile_root);
    let executable = fixture.write_executable("bin/pcsx2-qt", b"#!/bin/sh\nexit 0\n");
    roots.explicit_executables.push(executable.clone());
    let content = fixture.write("games/game.iso", &ps2_iso_bytes());

    // `profile_id` is a SHA-256-derived hash the module computes internally
    // (never a plain path format) - read it off a real discovery pass
    // rather than hand-reconstructing it.
    let discovery = discover_pcsx2_profiles(&roots).unwrap();
    let profile_id = discovery
        .profiles
        .iter()
        .find(|profile| profile.configuration_path == profile_root)
        .expect("fixture profile must be discovered")
        .profile_id
        .clone();

    let request = Pcsx2LaunchRequest {
        selected_content_path: content,
        expected_platform_id: "PS2".to_string(),
        expected_game_key: PS2_SERIAL.to_string(),
        expected_ps2_serial: PS2_SERIAL.to_string(),
        profile_id,
        expected_executable: executable,
        expected_user_directory_mode: Pcsx2UserDirectoryMode::DefaultNative,
    };
    ReadyFixture {
        fixture,
        roots,
        profile_root,
        request,
        firmware_evidence: vec![record_for_bios_bytes("Sony PlayStation 2 BIOS fixture")],
    }
}

fn preflight(ready: &ReadyFixture) -> Result<Pcsx2Command, Pcsx2LaunchPreflightError> {
    preflight_pcsx2_launch(&ready.request, &ready.roots, &ready.firmware_evidence)
}

// --- native direct-content Ready candidate passes preflight -----------------

#[test]
fn valid_native_ps2_iso_succeeds() {
    let ready = build_ready_fixture("ready");
    let command = preflight(&ready).expect("a fully wired native fixture must preflight cleanly");
    assert_eq!(command.executable, ready.request.expected_executable);
    assert_eq!(
        command.arguments,
        vec![ready.request.selected_content_path.clone().into_os_string()]
    );
    assert_eq!(command.selection.platform_id, "PS2");
    assert_eq!(command.selection.verified_ps2_serial, PS2_SERIAL);
    assert_eq!(
        command.selection.content_path,
        ready.request.selected_content_path
    );
}

// --- changed content identity is detected by the same mechanism used at spawn time ----------

#[test]
fn content_identity_capture_detects_a_swapped_file() {
    let ready = build_ready_fixture("content-changed");
    let path = ready.request.selected_content_path.clone();
    let original = fs::read(&path).unwrap();
    let before = CapturedFileIdentity::capture(&fs::symlink_metadata(&path).unwrap());
    fs::write(&path, b"different bytes entirely, same path").unwrap();
    let after = CapturedFileIdentity::capture(&fs::symlink_metadata(&path).unwrap());
    assert_ne!(
        before, after,
        "captured identity must change when the file changes"
    );
    fs::write(&path, &original).unwrap();
}

// --- symlink content rejected ------------------------------------------------------------------

#[test]
fn symlink_content_is_rejected() {
    let ready = build_ready_fixture("symlink-content");
    let link = ready.fixture.path("games/link.iso");
    symlink(&ready.request.selected_content_path, &link).unwrap();
    let mut request = ready.request.clone();
    request.selected_content_path = link;
    let error =
        preflight_pcsx2_launch(&request, &ready.roots, &ready.firmware_evidence).unwrap_err();
    assert_eq!(error.kind, Pcsx2LaunchPreflightErrorKind::ContentIsSymlink);
}

// --- unsupported content format rejected ---------------------------------------------------------

#[test]
fn non_iso_extension_is_rejected() {
    let ready = build_ready_fixture("unsupported-format");
    let chd = ready.fixture.write("games/game.chd", &ps2_iso_bytes());
    let mut request = ready.request.clone();
    request.selected_content_path = chd;
    let error =
        preflight_pcsx2_launch(&request, &ready.roots, &ready.firmware_evidence).unwrap_err();
    assert_eq!(
        error.kind,
        Pcsx2LaunchPreflightErrorKind::ContentFormatUnsupported
    );
}

#[test]
fn mount_input_archive_content_is_rejected() {
    let ready = build_ready_fixture("archive-content");
    let zip = ready.fixture.write("games/game.zip", b"pk not a real zip");
    let mut request = ready.request.clone();
    request.selected_content_path = zip;
    let error =
        preflight_pcsx2_launch(&request, &ready.roots, &ready.firmware_evidence).unwrap_err();
    assert_eq!(
        error.kind,
        Pcsx2LaunchPreflightErrorKind::ContentRequiresMount
    );
}

// --- identity mismatch rejected -----------------------------------------------------------------

#[test]
fn wrong_expected_game_key_is_rejected() {
    let mut ready = build_ready_fixture("wrong-game-key");
    ready.request.expected_game_key = "SLUS-99999".to_string();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, Pcsx2LaunchPreflightErrorKind::IdentityMismatch);
}

#[test]
fn wrong_expected_ps2_serial_is_rejected() {
    let mut ready = build_ready_fixture("wrong-serial");
    ready.request.expected_ps2_serial = "SLUS-99999".to_string();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, Pcsx2LaunchPreflightErrorKind::Ps2SerialMismatch);
}

// --- profile disappears rejected ----------------------------------------------------------------

#[test]
fn profile_directory_removed_is_rejected() {
    let ready = build_ready_fixture("profile-removed");
    fs::remove_dir_all(&ready.profile_root).unwrap();
    let error = preflight(&ready).unwrap_err();
    // Unlike Dolphin's discovery, a missing `Native` PCSX2 configuration
    // directory is not even reported as a blocked profile
    // (`report_missing: false` for this candidate) - it is simply absent
    // from fresh discovery entirely, so this surfaces as `ProfileNotFound`
    // rather than a binding refusal.
    assert_eq!(error.kind, Pcsx2LaunchPreflightErrorKind::ProfileNotFound);
}

// --- executable changes/disappears rejected -------------------------------------------------------

#[test]
fn executable_disappearing_is_rejected() {
    let ready = build_ready_fixture("executable-missing");
    fs::remove_file(&ready.request.expected_executable).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        Pcsx2LaunchPreflightErrorKind::BindingUnavailable
    );
}

#[test]
fn executable_replaced_with_a_symlink_is_rejected() {
    let ready = build_ready_fixture("executable-symlink");
    let real = ready
        .fixture
        .write_executable("bin/pcsx2-qt-real", b"#!/bin/sh\nexit 0\n");
    fs::remove_file(&ready.request.expected_executable).unwrap();
    symlink(&real, &ready.request.expected_executable).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        Pcsx2LaunchPreflightErrorKind::BindingUnavailable
    );
}

// --- portable.ini conflict rejected through the binding resolver -----------------------------------

#[test]
fn portable_ini_marker_beside_executable_rejects_default_native() {
    let ready = build_ready_fixture("portable-marker");
    fs::write(
        ready
            .request
            .expected_executable
            .parent()
            .unwrap()
            .join("portable.ini"),
        b"",
    )
    .unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(
        error.kind,
        Pcsx2LaunchPreflightErrorKind::BindingUnavailable
    );
}

// --- profile root drift rejected --------------------------------------------------------------

#[test]
fn profile_root_drift_is_rejected() {
    let ready = build_ready_fixture("root-drift");
    let mut request = ready.request.clone();
    // A stale profile id from a root that no longer matches PCSX2's current
    // default XDG resolution - never substituted with a different profile.
    request.profile_id = format!(
        "pcsx2:{}",
        ready.fixture.path("stale-config/PCSX2").display()
    );
    let error =
        preflight_pcsx2_launch(&request, &ready.roots, &ready.firmware_evidence).unwrap_err();
    assert_eq!(error.kind, Pcsx2LaunchPreflightErrorKind::ProfileNotFound);
}

// --- unsupported install type rejected ----------------------------------------------------------

#[test]
fn unsupported_install_type_is_rejected() {
    let fixture = Fixture::new("unsupported-install-type");
    let mut roots = base_roots(&fixture);
    let portable_root = fixture.path("pcsx2-portable");
    fs::create_dir_all(&portable_root).unwrap();
    fs::write(portable_root.join("PCSX2.ini"), b"[Filenames]\n").unwrap();
    roots
        .portable_configuration_roots
        .push(portable_root.clone());
    let executable = fixture.write_executable("bin/pcsx2-qt", b"#!/bin/sh\nexit 0\n");
    roots.explicit_executables.push(executable.clone());
    let content = fixture.write("games/game.iso", &ps2_iso_bytes());

    let discovery = discover_pcsx2_profiles(&roots).unwrap();
    let profile_id = discovery
        .profiles
        .iter()
        .find(|profile| profile.configuration_path == portable_root)
        .expect("fixture profile must be discovered")
        .profile_id
        .clone();

    let request = Pcsx2LaunchRequest {
        selected_content_path: content,
        expected_platform_id: "PS2".to_string(),
        expected_game_key: PS2_SERIAL.to_string(),
        expected_ps2_serial: PS2_SERIAL.to_string(),
        profile_id,
        expected_executable: executable,
        expected_user_directory_mode: Pcsx2UserDirectoryMode::DefaultNative,
    };
    let error = preflight_pcsx2_launch(&request, &roots, &[]).unwrap_err();
    assert_eq!(
        error.kind,
        Pcsx2LaunchPreflightErrorKind::BindingUnavailable
    );
}

// --- readiness gate --------------------------------------------------------------------------

#[test]
fn ready_with_warnings_candidate_is_rejected() {
    let ready = build_ready_fixture("ready-with-warnings");
    // Replace the base fixture's genuinely-matching BIOS bytes with content
    // that does not match any record in `firmware_evidence`: a real, safely
    // readable `bios/` directory whose single `.bin` candidate resolves
    // unambiguously but hashes to nothing in evidence is
    // `Pcsx2BiosVerification::Unknown`, which is a warning, not a blocker -
    // `ReadyWithWarnings`, not strict `Ready`.
    fs::remove_dir_all(ready.profile_root.join("bios")).unwrap();
    fs::create_dir_all(ready.profile_root.join("bios")).unwrap();
    fs::write(
        ready.profile_root.join("bios/scph-70012.bin"),
        b"not a real bios - does not match any evidence record",
    )
    .unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, Pcsx2LaunchPreflightErrorKind::CandidateNotReady);
}

#[test]
fn blocked_candidate_is_rejected() {
    let ready = build_ready_fixture("blocked-missing-bios");
    // No `bios` entry at all is `Pcsx2BiosVerification::Missing`, which
    // blocks the candidate outright (required firmware missing).
    fs::remove_dir_all(ready.profile_root.join("bios")).unwrap();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, Pcsx2LaunchPreflightErrorKind::CandidateNotReady);
}

#[test]
fn empty_firmware_evidence_never_reaches_strict_ready() {
    // Same genuinely-matching BIOS bytes on disk as `valid_native_ps2_iso_succeeds`,
    // but preflight is given zero firmware evidence records to match
    // against. `Pcsx2BiosVerification::Verified` must only ever be reached
    // when a real evidence record actually matches - it must never be
    // promoted from an absence of evidence - so this must fail the same way
    // as an unverified BIOS, not silently pass.
    let mut ready = build_ready_fixture("empty-evidence");
    ready.firmware_evidence.clear();
    let error = preflight(&ready).unwrap_err();
    assert_eq!(error.kind, Pcsx2LaunchPreflightErrorKind::CandidateNotReady);
}

// --- spawn: fake executable receives exact argv ------------------------------------------------------

#[test]
fn fake_executable_receives_exact_argv() {
    let ready = build_ready_fixture("argv-capture");
    let capture_path = ready.fixture.path("argv-capture.txt");
    ready.fixture.write_executable(
        "bin/pcsx2-qt",
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            capture_path.display()
        )
        .as_bytes(),
    );
    let command = preflight(&ready).unwrap();
    let mut process = spawn_pcsx2(command).expect("the fake script must spawn");
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
    let captured = fs::read_to_string(&capture_path).unwrap();
    let lines: Vec<&str> = captured.lines().collect();
    assert_eq!(
        lines,
        vec![ready.request.selected_content_path.to_str().unwrap()]
    );
}

// --- spawn: successful spawn returns PID, clean exit reported -----------------------------------------

#[test]
fn successful_spawn_returns_a_real_pid_and_clean_exit() {
    let ready = build_ready_fixture("spawn-success");
    let command = preflight(&ready).unwrap();
    let mut process = spawn_pcsx2(command).expect("the fake script must spawn");
    assert!(process.pid > 0);
    assert!(process.is_running() || process.poll().is_some());

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let report = loop {
        if let Some(report) = process.poll() {
            break report;
        }
        if std::time::Instant::now() > deadline {
            panic!("the fake script did not exit in time");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let status = report.status.as_ref().expect("wait() must succeed");
    assert!(status.success(), "the fake script exits 0");
}

// --- spawn: non-zero exit and bounded stderr reported -------------------------------------------------

#[test]
fn non_zero_exit_and_stderr_are_reported() {
    let ready = build_ready_fixture("spawn-nonzero");
    ready.fixture.write_executable(
        "bin/pcsx2-qt",
        b"#!/bin/sh\necho 'synthetic pcsx2 failure' 1>&2\nexit 7\n",
    );
    let command = preflight(&ready).unwrap();
    let mut process = spawn_pcsx2(command).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let report = loop {
        if let Some(report) = process.poll() {
            break report;
        }
        if std::time::Instant::now() > deadline {
            panic!("the fake script did not exit in time");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let status = report.status.as_ref().unwrap();
    assert!(!status.success());
    assert_eq!(status.code(), Some(7));
    assert!(String::from_utf8_lossy(&report.stderr).contains("synthetic pcsx2 failure"));
}

// --- spawn: shell metacharacters never interpreted -------------------------------------------------

#[test]
fn shell_metacharacters_in_content_path_are_never_interpreted() {
    let marker = std::env::temp_dir().join(format!(
        "archivefs-pcsx2-execution-shell-marker-{}",
        std::process::id()
    ));
    let _ = fs::remove_file(&marker);
    let ready = build_ready_fixture("shell-metacharacters");
    let dangerous_name = format!("game; touch {} #.iso", marker.display());
    let dangerous_path = ready
        .fixture
        .write(&format!("games/{dangerous_name}"), &ps2_iso_bytes());
    let mut request = ready.request.clone();
    request.selected_content_path = dangerous_path;
    let command = preflight_pcsx2_launch(&request, &ready.roots, &ready.firmware_evidence).unwrap();
    let mut process = spawn_pcsx2(command).unwrap();
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
    assert!(
        !marker.exists(),
        "shell metacharacters in a content path must never be interpreted"
    );
    let _ = fs::remove_file(&marker);
}

// --- spawn failure reported -----------------------------------------------------------------------

#[test]
fn spawn_failure_is_reported() {
    let ready = build_ready_fixture("spawn-failure");
    let mut command = preflight(&ready).unwrap();
    command.executable = ready.fixture.path("bin/does-not-exist");
    let result = spawn_pcsx2(command);
    assert!(matches!(result, Err(Pcsx2LaunchSpawnError::Spawn(_))));
}
